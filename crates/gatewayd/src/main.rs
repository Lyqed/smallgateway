//! gatewayd: the standalone data plane (Phase 1, milestone 1).
//!
//! One binary, one static YAML config file, no control plane. Routes by
//! path prefix, enforces the route's attribution contract (GB-1/2/3),
//! rejects with the operator's own templates (GB-4), and taps every
//! response stream through the canonical event model + meter without
//! buffering. Bootstrap promoted from `spikes/proxy-pingora/src/main.rs`
//! (Phase 0, Spike B), now config-driven.

mod proxy;

use std::path::PathBuf;
use std::sync::Arc;

use log::info;
use pingora::prelude::*;
use pingora::server::Server;

use gateway_core::config::Config;
use proxy::Gateway;

/// CLI: config path is mandatory (fail fast, no implicit defaults for the
/// thing that defines every governance decision); the listen address is
/// deployment plumbing and may come from flag or env.
struct Cli {
    config: PathBuf,
    listen: String,
}

impl Cli {
    fn parse() -> Self {
        let mut config: Option<PathBuf> = None;
        let mut listen =
            std::env::var("GATEWAYD_LISTEN").unwrap_or_else(|_| "127.0.0.1:6188".to_string());

        let args: Vec<String> = std::env::args().collect();
        let mut i = 1;
        while i + 1 < args.len() {
            match args[i].as_str() {
                "--config" => config = Some(PathBuf::from(&args[i + 1])),
                "--listen" => listen = args[i + 1].clone(),
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
        Cli { config, listen }
    }
}

fn usage(problem: &str) -> ! {
    eprintln!("error: {problem}");
    eprintln!("usage: gatewayd --config <gateway.yaml> [--listen 127.0.0.1:6188]");
    std::process::exit(2);
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();

    // Startup validation is the whole point of the static-config milestone:
    // a bad file never serves a request.
    let cfg = match Config::load(&cli.config) {
        Ok(cfg) => Arc::new(cfg),
        Err(e) => {
            eprintln!("gatewayd: {e}");
            std::process::exit(1);
        }
    };

    info!("gatewayd listening on {} ({} config)", cli.listen, cli.config.display());
    for route in &cfg.routes {
        let p = &cfg.providers[&route.provider];
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

    let mut server = Server::new(None).expect("pingora server init");
    server.bootstrap();

    let mut service = http_proxy_service(&server.configuration, Gateway::new(cfg));
    service.add_tcp(&cli.listen);
    server.add_service(service);
    server.run_forever();
}
