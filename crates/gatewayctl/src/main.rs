//! gatewayctl entrypoint: load the config repo, serve the fleet gRPC stream,
//! and re-render + roll out on SIGHUP or a repo change.
//!
//! CLI:
//! ```text
//! gatewayctl --repo <dir> --listen 127.0.0.1:6187 \
//!            [--join-token <secret>] [--token-ttl 300] [--poll-interval 3]
//! ```
//!
//! The join token is minted at startup (M1 convenience: one operator-supplied
//! token authorizing a join with no extra labels; a real deployment mints
//! per-join, label-scoped tokens via an admin surface). SIGHUP and the poll
//! watcher both re-render the repo and, if the render changed, run one
//! all-or-nothing wave across the connected fleet — the fleet analog of the
//! data plane's single reload path.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use log::{error, info};
use tonic::transport::Server;

use gateway_proto::FleetServiceServer;
use gatewayctl::fleet::Fleet;
use gatewayctl::render::render_repo;
use gatewayctl::server::{ControlPlane, FleetSvc};
use gatewayctl::store::RuntimeStore;
use gatewayctl::token::JoinTokens;

const DEFAULT_LISTEN: &str = "127.0.0.1:6187";
const DEFAULT_POLL_SECS: u64 = 3;
const DEFAULT_TOKEN_TTL_SECS: u64 = 300;
const DEFAULT_JOIN_TOKEN: &str = "dev-join-token";

struct Cli {
    repo: PathBuf,
    listen: String,
    join_token: String,
    token_ttl: u64,
    poll_interval: Duration,
    /// A hand-authored snapshot file distributed on SIGUSR1, BYPASSING the repo
    /// render gate — the break-glass / testing affordance that proves the
    /// node's independent NACK defense (docs/07). The demo points this at a
    /// deliberately-invalid config to show both nodes NACK while keeping their
    /// good version.
    push_raw: Option<PathBuf>,
}

impl Cli {
    fn parse() -> Self {
        let mut repo: Option<PathBuf> = None;
        let mut listen = std::env::var("GATEWAYCTL_LISTEN").unwrap_or_else(|_| DEFAULT_LISTEN.into());
        let mut join_token =
            std::env::var("GATEWAYCTL_JOIN_TOKEN").unwrap_or_else(|_| DEFAULT_JOIN_TOKEN.into());
        let mut token_ttl = DEFAULT_TOKEN_TTL_SECS;
        let mut poll_secs = DEFAULT_POLL_SECS;
        let mut push_raw: Option<PathBuf> = None;

        let args: Vec<String> = std::env::args().collect();
        let mut i = 1;
        while i + 1 < args.len() {
            match args[i].as_str() {
                "--repo" => repo = Some(PathBuf::from(&args[i + 1])),
                "--listen" => listen = args[i + 1].clone(),
                "--join-token" => join_token = args[i + 1].clone(),
                "--push-raw" => push_raw = Some(PathBuf::from(&args[i + 1])),
                "--token-ttl" => {
                    token_ttl = args[i + 1]
                        .parse()
                        .unwrap_or_else(|_| usage("--token-ttl takes whole seconds"))
                }
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
        let Some(repo) = repo else {
            usage("--repo <config-repo-dir> is required");
        };
        Cli {
            repo,
            listen,
            join_token,
            token_ttl,
            poll_interval: Duration::from_secs(poll_secs),
            push_raw,
        }
    }
}

fn usage(problem: &str) -> ! {
    eprintln!("error: {problem}");
    eprintln!(
        "usage: gatewayctl --repo <config-repo-dir> [--listen {DEFAULT_LISTEN}] \
         [--join-token <secret>] [--token-ttl 300] [--poll-interval 3] \
         [--push-raw <snapshot-file>]"
    );
    std::process::exit(2);
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();

    // Fail-fast startup render: a broken repo never serves a fleet, exactly as
    // a bad file never serves the single node.
    let rendered = match render_repo(&cli.repo) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("gatewayctl: {e}");
            std::process::exit(1);
        }
    };
    info!(
        "gatewayctl compiled repo {} -> render_hash={} source_commit={}",
        cli.repo.display(),
        &rendered.render_hash[..12],
        rendered.source_commit,
    );

    let fleet = Arc::new(Fleet::new(rendered));
    let store = Arc::new(RuntimeStore::new());
    let tokens = Arc::new(JoinTokens::new(cli.token_ttl));
    // M1: mint the operator-supplied join token so joining nodes can bootstrap.
    // Single-use enforcement means a real fleet mints one per node; for the
    // demo we mint a small pool so several nodes can join with predictable
    // secrets (`<token>`, `<token>-2`, `<token>-3`).
    tokens.mint(&cli.join_token, Default::default());
    tokens.mint(&format!("{}-2", cli.join_token), Default::default());
    tokens.mint(&format!("{}-3", cli.join_token), Default::default());
    info!(
        "gatewayctl minted join token(s) (ttl={}s); nodes present --join-token to bootstrap",
        cli.token_ttl
    );

    let cp = ControlPlane::new(fleet.clone(), store.clone(), tokens.clone());

    let listen: SocketAddr = cli
        .listen
        .parse()
        .unwrap_or_else(|e| usage(&format!("--listen must be host:port: {e}")));

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    runtime.block_on(async move {
        // Reload triggers: SIGHUP (immediate) + poll watcher, both re-render
        // the repo and roll out a wave if the render changed.
        spawn_sighup(cp.clone(), cli.repo.clone());
        // Break-glass / testing: SIGUSR1 injects the --push-raw file's bytes as
        // a raw snapshot, bypassing the render gate, to exercise the node NACK
        // defense (docs/07). No-op if --push-raw was not given.
        if let Some(raw) = cli.push_raw.clone() {
            spawn_sigusr1(cp.clone(), raw);
        }
        if cli.poll_interval.is_zero() {
            info!("[reload] poll watcher disabled (--poll-interval 0); SIGHUP still reloads");
        } else {
            spawn_poll(cp.clone(), cli.repo.clone(), cli.poll_interval);
            info!(
                "[reload] watching {} every {}s (SIGHUP reloads immediately)",
                cli.repo.display(),
                cli.poll_interval.as_secs()
            );
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
}

/// Re-render the repo and, if the render changed, roll out one wave. Shared by
/// both triggers — the fleet analog of `Reloader::reload`.
async fn reload_and_roll(cp: &Arc<ControlPlane>, repo: &std::path::Path, trigger: &str) {
    match render_repo(repo) {
        Ok(next) => {
            let hash = next.render_hash.clone();
            if cp.fleet.set_applied(next) {
                info!(
                    "[reload] trigger={trigger} repo re-rendered -> render_hash={}; rolling out",
                    &hash[..12]
                );
                cp.roll_out(trigger).await;
            } else {
                info!(
                    "[reload] trigger={trigger} repo re-rendered identical (render_hash={} unchanged); no-op",
                    &hash[..12]
                );
            }
        }
        Err(e) => {
            // A broken repo edit is REJECTED loudly; the previously-applied
            // render keeps being the fleet's desired state (never silent).
            error!(
                "[reload] trigger={trigger} REJECTED: {e}; keeping the last good render \
                 (render_hash={})",
                &cp.fleet.applied().render_hash[..12]
            );
        }
    }
}

fn spawn_sighup(cp: Arc<ControlPlane>, repo: PathBuf) {
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
            reload_and_roll(&cp, &repo, "sighup").await;
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

fn spawn_poll(cp: Arc<ControlPlane>, repo: PathBuf, interval: Duration) {
    tokio::spawn(async move {
        let mut last = repo_fingerprint(&repo);
        loop {
            tokio::time::sleep(interval).await;
            let now = repo_fingerprint(&repo);
            if now != last {
                last = now;
                reload_and_roll(&cp, &repo, "poll").await;
            }
        }
    });
}

/// A cheap change signal for the repo: the render_hash if it renders, else a
/// marker so a transiently-broken repo re-triggers once it is fixed. Rendering
/// on every poll tick is fine at M1 scale (docs/07: O(nodes) work, no
/// per-request involvement); a real deployment swaps this for a Git commit
/// watch or webhook (deferred).
fn repo_fingerprint(repo: &std::path::Path) -> String {
    match render_repo(repo) {
        Ok(r) => r.render_hash,
        Err(_) => "<<unrenderable>>".to_string(),
    }
}
