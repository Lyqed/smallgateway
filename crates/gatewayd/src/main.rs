//! gatewayd: the standalone data plane (Phase 1, milestones 1 + 2).
//!
//! One binary, one YAML config file, no control plane. Routes by path
//! prefix, enforces the route's attribution contract (GB-1/2/3), rejects
//! with the operator's own templates (GB-4), and taps every response
//! stream through the canonical event model + meter without buffering.
//! Milestone 2 makes the config hot-swappable: versioned snapshots bound
//! atomically per request, reloaded on SIGHUP or by the poll watcher, with
//! NACK-keeps-old semantics — see README.md for the exact promises.
//! Bootstrap promoted from `spikes/proxy-pingora/src/main.rs` (Phase 0,
//! Spike B), now config-driven.

mod aws_auth;
mod budget;
mod client;
mod proxy;
mod reload;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use log::info;
use pingora::prelude::*;
use pingora::server::Server;

use std::sync::Arc as StdArc;

use budget::{LogWebhookSink, NodeBudgets};
use proxy::Gateway;
use reload::{Reloader, SharedSnapshot};

/// Poll watcher default: "a few seconds" of staleness for file-edit-only
/// operators; SIGHUP is the immediate path.
const DEFAULT_POLL_INTERVAL_SECS: u64 = 3;

/// Where a node's config comes from. `File` is the standalone Phase 1 mode
/// (a local YAML, SIGHUP + poll); `ControlPlane` is the Phase 2 fleet mode
/// (dial gatewayctl, receive `RenderedSnapshot`s over the stream). Both bind
/// through the identical `SharedSnapshot` / `Reloader` machinery.
enum ConfigSource {
    File(PathBuf),
    ControlPlane {
        endpoint: String,
        node_id: String,
        join_token: String,
    },
}

/// CLI: exactly one config source is mandatory (fail fast, no implicit
/// defaults for the thing that defines every governance decision); the listen
/// address and watcher interval are deployment plumbing.
struct Cli {
    source: ConfigSource,
    listen: String,
    poll_interval: Duration,
}

impl Cli {
    fn parse() -> Self {
        let mut config: Option<PathBuf> = None;
        let mut control_plane: Option<String> = None;
        let mut node_id: Option<String> = None;
        let mut join_token: Option<String> = None;
        // Kept for symmetry/explicitness; the source is inferred from which of
        // --config / --control-plane is given, and --config-source can pin it.
        let mut config_source: Option<String> = None;
        let mut listen =
            std::env::var("GATEWAYD_LISTEN").unwrap_or_else(|_| "127.0.0.1:6188".to_string());
        let mut poll_secs = DEFAULT_POLL_INTERVAL_SECS;

        let args: Vec<String> = std::env::args().collect();
        let mut i = 1;
        while i + 1 < args.len() {
            match args[i].as_str() {
                "--config" => config = Some(PathBuf::from(&args[i + 1])),
                "--config-source" => config_source = Some(args[i + 1].clone()),
                "--control-plane" => control_plane = Some(args[i + 1].clone()),
                "--node-id" => node_id = Some(args[i + 1].clone()),
                "--join-token" => join_token = Some(args[i + 1].clone()),
                "--listen" => listen = args[i + 1].clone(),
                "--poll-interval" => {
                    poll_secs = args[i + 1]
                        .parse()
                        .unwrap_or_else(|_| usage("--poll-interval takes whole seconds (0 disables)"))
                }
                other => usage(&format!("unknown flag {other}")),
            }
            i += 2;
        }
        if i < args.len() {
            usage(&format!("dangling argument {:?}", args[i]));
        }

        // Resolve the source. --config-source, if given, must agree with the
        // flags present; otherwise the source is inferred.
        let wants_cp = matches!(config_source.as_deref(), Some("control-plane"))
            || (config_source.is_none() && control_plane.is_some());
        let wants_file = matches!(config_source.as_deref(), Some("file"))
            || (config_source.is_none() && control_plane.is_none());
        if let Some(other) = config_source.as_deref() {
            if !matches!(other, "file" | "control-plane") {
                usage("--config-source must be 'file' or 'control-plane'");
            }
        }

        let source = if wants_cp {
            let endpoint = control_plane
                .unwrap_or_else(|| usage("control-plane mode requires --control-plane <endpoint>"));
            let node_id =
                node_id.unwrap_or_else(|| usage("control-plane mode requires --node-id <id>"));
            let join_token = join_token
                .unwrap_or_else(|| usage("control-plane mode requires --join-token <secret>"));
            if config.is_some() {
                usage("--config and --control-plane are mutually exclusive");
            }
            ConfigSource::ControlPlane {
                endpoint,
                node_id,
                join_token,
            }
        } else if wants_file {
            let config = config.unwrap_or_else(|| usage("--config is required in file mode"));
            ConfigSource::File(config)
        } else {
            usage("specify exactly one config source: --config or --control-plane");
        };

        Cli {
            source,
            listen,
            poll_interval: Duration::from_secs(poll_secs),
        }
    }
}

fn usage(problem: &str) -> ! {
    eprintln!("error: {problem}");
    eprintln!(
        "usage:\n  \
         gatewayd --config <gateway.yaml> [--listen 127.0.0.1:6188] [--poll-interval 3]\n  \
         gatewayd --control-plane <host:port> --node-id <id> --join-token <secret> \
         [--config-source control-plane] [--listen 127.0.0.1:6188]"
    );
    std::process::exit(2);
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();

    // GB-5: the node-local budget counters, shared between the proxy (which
    // taps and enforces) and — in control-plane mode — the client loop (which
    // reports spend and applies share grants). The node id labels the GB-6
    // alerts; standalone file mode uses a fixed label.
    let node_label = match &cli.source {
        ConfigSource::ControlPlane { node_id, .. } => node_id.clone(),
        ConfigSource::File(_) => "standalone".to_string(),
    };
    let budgets = StdArc::new(NodeBudgets::new(node_label, Box::new(LogWebhookSink)));

    // Obtain the SharedSnapshot pingora binds per request from whichever config
    // source was chosen. In BOTH modes the snapshot is a validated v1 that must
    // exist before serving — a node with no valid config never serves a
    // request (fail-fast, unchanged from milestone 1).
    let shared = match &cli.source {
        ConfigSource::File(path) => {
            let reloader = match Reloader::bootstrap(path.clone()) {
                Ok(reloader) => Arc::new(reloader),
                Err(e) => {
                    eprintln!("gatewayd: {e}");
                    std::process::exit(1);
                }
            };
            let shared = reloader.shared();
            log_active_routes(&shared, &format!("file {}", path.display()));

            // File-mode triggers: SIGHUP + poll, funneling through
            // Reloader::reload; in-flight requests keep their bound snapshot.
            reload::spawn_sighup_listener(reloader.clone());
            if cli.poll_interval.is_zero() {
                info!("[reload] poll watcher disabled (--poll-interval 0); SIGHUP still reloads");
            } else {
                reload::spawn_poll_watcher(reloader.clone(), cli.poll_interval);
                info!(
                    "[reload] watching {} every {}s (SIGHUP reloads immediately)",
                    path.display(),
                    cli.poll_interval.as_secs(),
                );
            }
            shared
        }
        ConfigSource::ControlPlane {
            endpoint,
            node_id,
            join_token,
        } => {
            // Control-plane mode: dial gatewayctl, join, and BLOCK until the
            // first RenderedSnapshot is received and bound. The stream is the
            // reload trigger from here on (no file, no SIGHUP/poll); every
            // subsequent Push binds through the same Reloader path.
            info!(
                "gatewayd control-plane mode: node {node_id:?} dialing {endpoint}"
            );
            let shared = match client::connect_and_bootstrap(
                endpoint.clone(),
                node_id.clone(),
                join_token.clone(),
                budgets.clone(),
            ) {
                Ok(shared) => shared,
                Err(e) => {
                    eprintln!("gatewayd: control-plane bootstrap failed: {e}");
                    std::process::exit(1);
                }
            };
            log_active_routes(&shared, &format!("control-plane {endpoint}"));
            shared
        }
    };

    info!(
        "gatewayd listening on {} (cfg=v{} hash={})",
        cli.listen,
        shared.load().version,
        shared.load().short_hash(),
    );

    let mut server = Server::new(None).expect("pingora server init");
    server.bootstrap();

    let mut service =
        http_proxy_service(&server.configuration, Gateway::with_budgets(shared, budgets));
    service.add_tcp(&cli.listen);
    server.add_service(service);
    server.run_forever();
}

/// Log the composed policy of every active route — identical in both config
/// sources, since the bound `Config` is the same shape however it arrived.
fn log_active_routes(shared: &SharedSnapshot, source: &str) {
    let snap = shared.load();
    info!(
        "gatewayd active config from {source}: cfg=v{} hash={}",
        snap.version,
        snap.short_hash()
    );
    for route in &snap.config.routes {
        let p = &snap.config.providers[&route.provider];
        // The COMPOSED policy (fleet → project → route), not the raw route
        // block: what this route actually enforces.
        let policy = route.policy();
        info!(
            "[route] {} -> {} ({}) upstream {}:{} tls={}{}{} | required={:?} pinned={:?} \
             from_claims={:?} derived={:?} labels={:?}{}",
            route.prefix,
            route.provider,
            p.kind.name(),
            p.upstream.host,
            p.upstream.port,
            p.upstream.tls,
            route.project.as_deref().map(|pr| format!(" project={pr}")).unwrap_or_default(),
            route.condition.as_deref().map(|c| format!(" match={c:?}")).unwrap_or_default(),
            policy.required_keys,
            policy.pinned.keys().collect::<Vec<_>>(),
            policy.from_claims.keys().collect::<Vec<_>>(),
            policy.derived.keys().collect::<Vec<_>>(),
            policy.labels.iter().map(|l| l.key.as_str()).collect::<Vec<_>>(),
            p.sts
                .as_ref()
                .map(|s| format!(
                    " sts[role={} tags={:?}]",
                    s.role_arn,
                    s.tags.iter().map(|t| t.key.as_str()).collect::<Vec<_>>()
                ))
                .unwrap_or_default(),
        );
    }
}
