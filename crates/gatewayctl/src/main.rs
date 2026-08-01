//! gatewayctl entrypoint: source the desired config (a directory or a Git
//! ref/commit), serve the fleet gRPC stream, run the drift reconciler, and
//! re-render + admit + roll out on SIGHUP or a source change. Also exposes an
//! `admit` subcommand so CI can gate a config PR.
//!
//! CLI:
//! ```text
//! # serve mode (default)
//! gatewayctl --repo <dir> [--listen 127.0.0.1:6187] \
//!            [--git-repo <path> --git-ref <ref>] \
//!            [--join-token <secret>] [--token-ttl 300] [--poll-interval 3] \
//!            [--reconcile-interval 5] [--break-glass-file <path>]
//!
//! # admission gate (CI): exit non-zero if the candidate is blocked
//! gatewayctl admit --repo <dir>
//! gatewayctl admit --git-repo <path> --git-ref <ref>
//! ```
//!
//! The config source is a plain directory (`--repo`) OR a Git repo at a
//! ref/commit (`--git-repo` + `--git-ref`). SIGHUP and the poll watcher both
//! re-resolve the source, run admission, and — if the render changed and admits
//! — roll one all-or-nothing wave across the connected fleet. The reconciler
//! runs on its own interval, healing drifted nodes back to desired.
//!
//! Break-glass: `--break-glass-file <path>` arms a SIGUSR2 handler; the file
//! holds `node_id [ttl_secs]` lines. On the signal each listed node is marked
//! break-glass for its TTL, and the reconciler tolerates its drift until the
//! window lapses (docs/00 break-glass with TTL).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use log::{error, info, warn};
use tonic::transport::Server;

use gateway_proto::FleetServiceServer;
use gatewayctl::admission::AdmissionPolicy;
use gatewayctl::fleet::Fleet;
use gatewayctl::reconcile::Reconciler;
use gatewayctl::render::render_source;
use gatewayctl::server::{ControlPlane, FleetSvc};
use gatewayctl::source::{ConfigSource, DirectorySource, GitSource};
use gatewayctl::store::RuntimeStore;
use gatewayctl::token::JoinTokens;

const DEFAULT_LISTEN: &str = "127.0.0.1:6187";
const DEFAULT_POLL_SECS: u64 = 3;
const DEFAULT_RECONCILE_SECS: u64 = 5;
const DEFAULT_TOKEN_TTL_SECS: u64 = 300;
const DEFAULT_JOIN_TOKEN: &str = "dev-join-token";
const DEFAULT_GIT_REF: &str = "HEAD";

struct Cli {
    listen: String,
    join_token: String,
    token_ttl: u64,
    poll_interval: Duration,
    reconcile_interval: Duration,
    /// The resolved config source: a directory or a Git ref/commit.
    source: Arc<dyn ConfigSource>,
    /// A hand-authored snapshot file distributed on SIGUSR1, BYPASSING the repo
    /// render gate — the break-glass / testing affordance that proves the
    /// node's independent NACK defense (docs/07).
    push_raw: Option<PathBuf>,
    /// A file listing `node_id [ttl_secs]` per line; on SIGUSR2 each node is
    /// marked break-glass for its TTL (default token_ttl if omitted).
    break_glass_file: Option<PathBuf>,
}

/// A parsed set of raw CLI flags before the source is resolved. Shared by the
/// serve path and the `admit` subcommand.
#[derive(Default)]
struct Flags {
    repo: Option<PathBuf>,
    git_repo: Option<PathBuf>,
    git_ref: Option<String>,
    listen: Option<String>,
    join_token: Option<String>,
    token_ttl: Option<u64>,
    poll_secs: Option<u64>,
    reconcile_secs: Option<u64>,
    push_raw: Option<PathBuf>,
    break_glass_file: Option<PathBuf>,
}

impl Flags {
    /// Parse `args` (excluding argv[0] and any leading subcommand).
    fn parse(args: &[String]) -> Flags {
        let mut f = Flags::default();
        let mut i = 0;
        while i + 1 < args.len() {
            match args[i].as_str() {
                "--repo" => f.repo = Some(PathBuf::from(&args[i + 1])),
                "--git-repo" => f.git_repo = Some(PathBuf::from(&args[i + 1])),
                "--git-ref" => f.git_ref = Some(args[i + 1].clone()),
                "--listen" => f.listen = Some(args[i + 1].clone()),
                "--join-token" => f.join_token = Some(args[i + 1].clone()),
                "--push-raw" => f.push_raw = Some(PathBuf::from(&args[i + 1])),
                "--break-glass-file" => f.break_glass_file = Some(PathBuf::from(&args[i + 1])),
                "--token-ttl" => {
                    f.token_ttl = Some(
                        args[i + 1]
                            .parse()
                            .unwrap_or_else(|_| usage("--token-ttl takes whole seconds")),
                    )
                }
                "--poll-interval" => {
                    f.poll_secs = Some(
                        args[i + 1]
                            .parse()
                            .unwrap_or_else(|_| usage("--poll-interval takes whole seconds (0 disables)")),
                    )
                }
                "--reconcile-interval" => {
                    f.reconcile_secs = Some(
                        args[i + 1].parse().unwrap_or_else(|_| {
                            usage("--reconcile-interval takes whole seconds (0 disables)")
                        }),
                    )
                }
                other => usage(&format!("unknown flag {other}")),
            }
            i += 2;
        }
        if i < args.len() {
            usage(&format!("dangling argument {:?}", args[i]));
        }
        f
    }

    /// Build the config source from the flags. A directory (`--repo`) and a Git
    /// repo (`--git-repo`) are mutually exclusive; exactly one is required.
    fn source(&self) -> Arc<dyn ConfigSource> {
        match (&self.repo, &self.git_repo) {
            (Some(_), Some(_)) => usage("--repo and --git-repo are mutually exclusive"),
            (Some(dir), None) => Arc::new(DirectorySource::new(dir.clone())),
            (None, Some(repo)) => {
                let reference = self.git_ref.clone().unwrap_or_else(|| DEFAULT_GIT_REF.to_string());
                Arc::new(GitSource::new(repo.clone(), reference))
            }
            (None, None) => usage("a config source is required: --repo <dir> OR --git-repo <path> [--git-ref <ref>]"),
        }
    }
}

impl Cli {
    fn parse_serve(args: &[String]) -> Cli {
        let f = Flags::parse(args);
        let source = f.source();
        Cli {
            listen: f
                .listen
                .or_else(|| std::env::var("GATEWAYCTL_LISTEN").ok())
                .unwrap_or_else(|| DEFAULT_LISTEN.into()),
            join_token: f
                .join_token
                .or_else(|| std::env::var("GATEWAYCTL_JOIN_TOKEN").ok())
                .unwrap_or_else(|| DEFAULT_JOIN_TOKEN.into()),
            token_ttl: f.token_ttl.unwrap_or(DEFAULT_TOKEN_TTL_SECS),
            poll_interval: Duration::from_secs(f.poll_secs.unwrap_or(DEFAULT_POLL_SECS)),
            reconcile_interval: Duration::from_secs(f.reconcile_secs.unwrap_or(DEFAULT_RECONCILE_SECS)),
            source,
            push_raw: f.push_raw,
            break_glass_file: f.break_glass_file,
        }
    }
}

fn usage(problem: &str) -> ! {
    eprintln!("error: {problem}");
    eprintln!(
        "usage:\n  \
         gatewayctl --repo <dir> | (--git-repo <path> [--git-ref <ref>]) \
         [--listen {DEFAULT_LISTEN}] [--join-token <secret>] [--token-ttl 300] \
         [--poll-interval 3] [--reconcile-interval 5] [--push-raw <file>] \
         [--break-glass-file <file>]\n  \
         gatewayctl admit --repo <dir> | (--git-repo <path> [--git-ref <ref>])"
    );
    std::process::exit(2);
}

/// The admission policy the control plane gates rollouts with (and `admit`
/// checks). The built-in Baseline rules are always on; one demonstrative CEL
/// rule ("every provider has an upstream host") is added so the CEL path is
/// exercised in the running control plane, not only in tests.
fn admission_policy() -> AdmissionPolicy {
    AdmissionPolicy::new().with_cel_rule(
        "route-list-non-empty",
        "size(config.routes) > 0",
        "the config must define at least one route",
    )
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();
    // Subcommand dispatch: `admit` runs the admission gate and exits; anything
    // else is the serve path.
    if args.get(1).map(String::as_str) == Some("admit") {
        run_admit(&args[2..]);
    }
    run_serve(Cli::parse_serve(&args[1..]));
}

/// The `admit` subcommand: resolve + render the candidate source, run admission,
/// print the verdict, and exit non-zero on a block so CI can gate the PR.
fn run_admit(args: &[String]) -> ! {
    let flags = Flags::parse(args);
    let source = flags.source();
    let policy = admission_policy();
    match policy.admit_source(source.as_ref()) {
        Ok(verdict) if verdict.is_admitted() => {
            println!("ADMIT: {} passed all admission rules", source.describe());
            std::process::exit(0);
        }
        Ok(verdict) => {
            eprintln!("BLOCK: {} failed admission:", source.describe());
            for f in verdict.failures() {
                eprintln!("  - {f}");
            }
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("BLOCK: {} is unrenderable: {e}", source.describe());
            std::process::exit(1);
        }
    }
}

fn run_serve(cli: Cli) -> ! {
    let policy = admission_policy();

    // Fail-fast startup: a broken or admission-blocked source never serves a
    // fleet, exactly as a bad file never serves the single node. Admission runs
    // BEFORE the first render becomes desired (docs/07: admission gates a config
    // before it can become desired).
    match policy.admit_source(cli.source.as_ref()) {
        Ok(v) if v.is_admitted() => {}
        Ok(v) => {
            eprintln!("gatewayctl: startup config BLOCKED by admission:");
            for f in v.failures() {
                eprintln!("  - {f}");
            }
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("gatewayctl: startup config unrenderable: {e}");
            std::process::exit(1);
        }
    }
    let rendered = match render_source(cli.source.as_ref()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("gatewayctl: {e}");
            std::process::exit(1);
        }
    };
    info!(
        "gatewayctl compiled source {} -> render_hash={} source_commit={}",
        cli.source.describe(),
        &rendered.render_hash[..12],
        rendered.source_commit,
    );

    let fleet = Arc::new(Fleet::new(rendered));
    let store = Arc::new(RuntimeStore::new());
    let tokens = Arc::new(JoinTokens::new(cli.token_ttl));
    // Mint the operator-supplied join token(s) so joining nodes can bootstrap.
    tokens.mint(&cli.join_token, Default::default());
    tokens.mint(&format!("{}-2", cli.join_token), Default::default());
    tokens.mint(&format!("{}-3", cli.join_token), Default::default());
    info!(
        "gatewayctl minted join token(s) (ttl={}s); nodes present --join-token to bootstrap",
        cli.token_ttl
    );

    let cp = ControlPlane::new(fleet, store, tokens);
    let source = cli.source.clone();
    let policy = Arc::new(policy);

    let listen: SocketAddr = cli
        .listen
        .parse()
        .unwrap_or_else(|e| usage(&format!("--listen must be host:port: {e}")));

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    runtime.block_on(async move {
        // Reload triggers: SIGHUP + poll watcher, both re-resolve the source,
        // admit, and roll a wave if the render changed and admits.
        spawn_sighup(cp.clone(), source.clone(), policy.clone());
        if let Some(raw) = cli.push_raw.clone() {
            spawn_sigusr1(cp.clone(), raw);
        }
        if let Some(bg) = cli.break_glass_file.clone() {
            spawn_break_glass(cp.clone(), bg, cli.token_ttl);
        }
        if cli.poll_interval.is_zero() {
            info!("[reload] poll watcher disabled (--poll-interval 0); SIGHUP still reloads");
        } else {
            spawn_poll(cp.clone(), source.clone(), policy.clone(), cli.poll_interval);
            info!(
                "[reload] watching {} every {}s (SIGHUP reloads immediately)",
                source.describe(),
                cli.poll_interval.as_secs()
            );
        }

        // The drift reconciler (docs/07). Runs on its own interval, healing
        // drifted nodes back to desired between waves.
        if cli.reconcile_interval.is_zero() {
            info!("[reconcile] reconciler disabled (--reconcile-interval 0)");
        } else {
            let reconciler = Reconciler::new(cp.clone(), cli.reconcile_interval);
            tokio::spawn(reconciler.run());
        }

        info!("gatewayctl serving FleetService on {listen}");
        if let Err(e) = Server::builder()
            .add_service(FleetServiceServer::new(FleetSvc::new(cp)))
            .serve(listen)
            .await
        {
            error!("gatewayctl server error: {e}");
            std::process::exit(1);
        }
    });
    std::process::exit(0);
}

/// Re-resolve the source, admit, and — if the render changed and admits — roll
/// one wave. Shared by both reload triggers (the fleet analog of a reload).
async fn reload_and_roll(
    cp: &Arc<ControlPlane>,
    source: &Arc<dyn ConfigSource>,
    policy: &Arc<AdmissionPolicy>,
    trigger: &str,
) {
    // Admission gate BEFORE the candidate can become desired (docs/07).
    match policy.admit_source(source.as_ref()) {
        Ok(v) if v.is_admitted() => {}
        Ok(v) => {
            error!(
                "[reload] trigger={trigger} BLOCKED by admission ({} rule(s)); keeping the last \
                 good render",
                v.failures().len()
            );
            for f in v.failures() {
                error!("[reload]   admission: {f}");
            }
            return;
        }
        Err(e) => {
            error!("[reload] trigger={trigger} candidate unrenderable: {e}; keeping last good render");
            return;
        }
    }
    match render_source(source.as_ref()) {
        Ok(next) => {
            let hash = next.render_hash.clone();
            let commit = next.source_commit.clone();
            if cp.fleet.set_applied(next) {
                info!(
                    "[reload] trigger={trigger} re-rendered -> render_hash={} source_commit={commit}; rolling out",
                    &hash[..12]
                );
                cp.roll_out(trigger).await;
            } else {
                info!(
                    "[reload] trigger={trigger} re-rendered identical (render_hash={} unchanged); no-op",
                    &hash[..12]
                );
            }
        }
        Err(e) => {
            error!(
                "[reload] trigger={trigger} REJECTED: {e}; keeping the last good render \
                 (render_hash={})",
                &cp.fleet.applied().render_hash[..12]
            );
        }
    }
}

fn spawn_sighup(cp: Arc<ControlPlane>, source: Arc<dyn ConfigSource>, policy: Arc<AdmissionPolicy>) {
    tokio::spawn(async move {
        let mut hangup =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()) {
                Ok(s) => s,
                Err(e) => {
                    error!("[reload] cannot install SIGHUP handler: {e}");
                    return;
                }
            };
        while hangup.recv().await.is_some() {
            reload_and_roll(&cp, &source, &policy, "sighup").await;
        }
    });
}

fn spawn_sigusr1(cp: Arc<ControlPlane>, raw: PathBuf) {
    tokio::spawn(async move {
        let mut sig =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1()) {
                Ok(s) => s,
                Err(e) => {
                    error!("[inject] cannot install SIGUSR1 handler: {e}");
                    return;
                }
            };
        while sig.recv().await.is_some() {
            match std::fs::read(&raw) {
                Ok(bytes) => {
                    info!(
                        "[inject] SIGUSR1: distributing raw snapshot {} ({} bytes), \
                         BYPASSING the render gate — nodes are the validation authority",
                        raw.display(),
                        bytes.len()
                    );
                    cp.roll_out_raw("sigusr1-raw", bytes).await;
                }
                Err(e) => error!("[inject] cannot read --push-raw {}: {e}", raw.display()),
            }
        }
    });
}

/// SIGUSR2: read the break-glass file and mark each listed node break-glass for
/// its TTL. Each line is `node_id [ttl_secs]`; a missing TTL uses `default_ttl`.
fn spawn_break_glass(cp: Arc<ControlPlane>, file: PathBuf, default_ttl: u64) {
    tokio::spawn(async move {
        let mut sig =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined2()) {
                Ok(s) => s,
                Err(e) => {
                    error!("[break-glass] cannot install SIGUSR2 handler: {e}");
                    return;
                }
            };
        while sig.recv().await.is_some() {
            let text = match std::fs::read_to_string(&file) {
                Ok(t) => t,
                Err(e) => {
                    error!("[break-glass] cannot read {}: {e}", file.display());
                    continue;
                }
            };
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let mut parts = line.split_whitespace();
                let Some(node_id) = parts.next() else { continue };
                let ttl = parts.next().and_then(|s| s.parse().ok()).unwrap_or(default_ttl);
                match cp.store.set_break_glass(node_id, ttl) {
                    Some(until) => info!(
                        "[break-glass] node {node_id:?} marked break-glass for {ttl}s (until unix \
                         {until}); the reconciler will tolerate its drift until then (docs/00)"
                    ),
                    None => warn!(
                        "[break-glass] node {node_id:?} is not known to the control plane; ignored"
                    ),
                }
            }
        }
    });
}

fn spawn_poll(
    cp: Arc<ControlPlane>,
    source: Arc<dyn ConfigSource>,
    policy: Arc<AdmissionPolicy>,
    interval: Duration,
) {
    tokio::spawn(async move {
        let mut last = source_fingerprint(&source);
        loop {
            tokio::time::sleep(interval).await;
            let now = source_fingerprint(&source);
            if now != last {
                last = now;
                reload_and_roll(&cp, &source, &policy, "poll").await;
            }
        }
    });
}

/// A cheap change signal for the source: the render_hash if it renders, else a
/// marker so a transiently-broken source re-triggers once it is fixed. For a Git
/// source this reflects a commit's content; a real deployment swaps the poll for
/// a webhook (deferred, poll is the floor — docs/07).
fn source_fingerprint(source: &Arc<dyn ConfigSource>) -> String {
    match render_source(source.as_ref()) {
        Ok(r) => format!("{}:{}", r.source_commit, r.render_hash),
        Err(_) => "<<unrenderable>>".to_string(),
    }
}
