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

mod proxy;
mod reload;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use log::info;
use pingora::prelude::*;
use pingora::server::Server;

use proxy::Gateway;
use reload::Reloader;

/// Poll watcher default: "a few seconds" of staleness for file-edit-only
/// operators; SIGHUP is the immediate path.
const DEFAULT_POLL_INTERVAL_SECS: u64 = 3;

/// CLI: config path is mandatory (fail fast, no implicit defaults for the
/// thing that defines every governance decision); the listen address and
/// watcher interval are deployment plumbing.
struct Cli {
    config: PathBuf,
    listen: String,
    poll_interval: Duration,
}

impl Cli {
    fn parse() -> Self {
        let mut config: Option<PathBuf> = None;
        let mut listen =
            std::env::var("GATEWAYD_LISTEN").unwrap_or_else(|_| "127.0.0.1:6188".to_string());
        let mut poll_secs = DEFAULT_POLL_INTERVAL_SECS;

        let args: Vec<String> = std::env::args().collect();
        let mut i = 1;
        while i + 1 < args.len() {
            match args[i].as_str() {
                "--config" => config = Some(PathBuf::from(&args[i + 1])),
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
        let Some(config) = config else {
            usage("--config is required");
        };
        Cli {
            config,
            listen,
            poll_interval: Duration::from_secs(poll_secs),
        }
    }
}

fn usage(problem: &str) -> ! {
    eprintln!("error: {problem}");
    eprintln!(
        "usage: gatewayd --config <gateway.yaml> [--listen 127.0.0.1:6188] \
         [--poll-interval 3]"
    );
    std::process::exit(2);
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();

    // Startup validation stays the fail-fast gate from milestone 1: a bad
    // file never serves a request. Milestone 2 routes it through the
    // renderer so the very first config is snapshot v1 like every later one.
    let reloader = match Reloader::bootstrap(cli.config.clone()) {
        Ok(reloader) => Arc::new(reloader),
        Err(e) => {
            eprintln!("gatewayd: {e}");
            std::process::exit(1);
        }
    };
    let shared = reloader.shared();
    let snap = shared.load();

    info!(
        "gatewayd listening on {} ({} config, cfg=v{} hash={})",
        cli.listen,
        cli.config.display(),
        snap.version,
        snap.short_hash(),
    );
    for route in &snap.config.routes {
        let p = &snap.config.providers[&route.provider];
        info!(
            "[route] {} -> {} ({}) upstream {}:{} tls={} | required={:?} pinned={:?} from_claims={:?}",
            route.prefix,
            route.provider,
            p.kind.name(),
            p.upstream.host,
            p.upstream.port,
            p.upstream.tls,
            route.attribution.required_keys,
            route.attribution.pinned.keys().collect::<Vec<_>>(),
            route.attribution.from_claims.keys().collect::<Vec<_>>(),
        );
    }

    // Both reload triggers funnel through Reloader::reload; in-flight
    // requests keep the snapshot they bound at request start.
    reload::spawn_sighup_listener(reloader.clone());
    if cli.poll_interval.is_zero() {
        info!("[reload] poll watcher disabled (--poll-interval 0); SIGHUP still reloads");
    } else {
        reload::spawn_poll_watcher(reloader.clone(), cli.poll_interval);
        info!(
            "[reload] watching {} every {}s (SIGHUP reloads immediately)",
            cli.config.display(),
            cli.poll_interval.as_secs(),
        );
    }

    let mut server = Server::new(None).expect("pingora server init");
    server.bootstrap();

    let mut service = http_proxy_service(&server.configuration, Gateway::new(shared));
    service.add_tcp(&cli.listen);
    server.add_service(service);
    server.run_forever();
}
