//! Spike B, candidate 1: minimal streaming proxy on Pingora.
//!
//! Forwards HTTP to a configurable upstream and taps the response body in
//! `response_body_filter` — each chunk is fed through the per-request
//! spike-event-model adapter and metered, while the unmodified bytes stream
//! on to the client. Nothing buffers the whole response: the adapters hold
//! only the current partial frame, and the filter never accumulates body
//! bytes.

mod provider;

use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use log::info;
use pingora::prelude::*;
use pingora::server::Server;

use provider::Provider;
use spike_event_model::event::Event;
use spike_event_model::metering::Meter;

/// Upstream + listener config, from CLI flags with env-var fallback.
#[derive(Debug, Clone)]
struct SpikeConfig {
    listen: String,
    upstream_host: String,
    upstream_port: u16,
    upstream_tls: bool,
    sni: String,
}

impl SpikeConfig {
    fn load() -> Self {
        let mut listen = env_or("SPIKE_LISTEN", "127.0.0.1:6188");
        let mut host = env_or("SPIKE_UPSTREAM_HOST", "127.0.0.1");
        let mut port = env_or("SPIKE_UPSTREAM_PORT", "6190");
        let mut tls = env_or("SPIKE_UPSTREAM_TLS", "false");
        let mut sni = std::env::var("SPIKE_UPSTREAM_SNI").ok();

        let args: Vec<String> = std::env::args().collect();
        let mut i = 1;
        while i + 1 < args.len() {
            match args[i].as_str() {
                "--listen" => listen = args[i + 1].clone(),
                "--upstream-host" => host = args[i + 1].clone(),
                "--upstream-port" => port = args[i + 1].clone(),
                "--upstream-tls" => tls = args[i + 1].clone(),
                "--sni" => sni = Some(args[i + 1].clone()),
                other => {
                    eprintln!("unknown flag {other}");
                    std::process::exit(2);
                }
            }
            i += 2;
        }

        let upstream_port: u16 = port.parse().unwrap_or_else(|_| {
            eprintln!("invalid upstream port {port:?}");
            std::process::exit(2);
        });
        let upstream_tls = matches!(tls.as_str(), "true" | "on" | "1" | "yes");
        SpikeConfig {
            listen,
            sni: sni.unwrap_or_else(|| host.clone()),
            upstream_host: host,
            upstream_port,
            upstream_tls,
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Per-request state: the chosen adapter, the running meter, and summary
/// counters. Deliberately bounded — the tap stores counts, never the body.
struct SpikeCtx {
    provider: Provider,
    adapter: Box<dyn spike_event_model::adapters::Adapter + Send + Sync>,
    meter: Meter,
    body_bytes: usize,
    body_chunks: usize,
    event_counts: [usize; 6],
}

impl SpikeCtx {
    fn new(provider: Provider) -> Self {
        SpikeCtx {
            provider,
            adapter: provider.new_adapter(),
            meter: Meter::new(),
            body_bytes: 0,
            body_chunks: 0,
            event_counts: [0; 6],
        }
    }

    fn count(&mut self, event: &Event) {
        let idx = match event {
            Event::MessageStart { .. } => 0,
            Event::ContentDelta { .. } => 1,
            Event::ToolCallDelta { .. } => 2,
            Event::UsageDelta { .. } => 3,
            Event::MessageEnd { .. } => 4,
            Event::Error { .. } => 5,
        };
        self.event_counts[idx] += 1;
    }

    fn summary(&self) -> String {
        const NAMES: [&str; 6] = [
            "MessageStart",
            "ContentDelta",
            "ToolCallDelta",
            "UsageDelta",
            "MessageEnd",
            "Error",
        ];
        NAMES
            .iter()
            .zip(self.event_counts.iter())
            .filter(|(_, n)| **n > 0)
            .map(|(name, n)| format!("{name}={n}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

struct SpikeGateway {
    cfg: SpikeConfig,
}

#[async_trait]
impl ProxyHttp for SpikeGateway {
    type CTX = SpikeCtx;

    fn new_ctx(&self) -> Self::CTX {
        // Provider is unknown until the request headers are visible; the
        // default matches the spec and request_filter re-initializes.
        SpikeCtx::new(Provider::OpenAi)
    }

    async fn request_filter(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<bool> {
        let header = session
            .req_header()
            .headers
            .get("x-spike-provider")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_owned());
        let path = session.req_header().uri.path().to_owned();
        let provider = Provider::select(header.as_deref(), &path);
        *ctx = SpikeCtx::new(provider);
        info!(
            "[req] {} {} -> provider={}",
            session.req_header().method,
            path,
            provider.name()
        );
        Ok(false)
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        _ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        let addr = format!("{}:{}", self.cfg.upstream_host, self.cfg.upstream_port);
        let peer = HttpPeer::new(addr, self.cfg.upstream_tls, self.cfg.sni.clone());
        Ok(Box::new(peer))
    }

    /// The tap. Pingora hands each body chunk as `&mut Option<Bytes>` on its
    /// way downstream; we feed a copy of the bytes to the adapter and leave
    /// the option untouched, so the client receives the identical stream.
    fn response_body_filter(
        &self,
        _session: &mut Session,
        body: &mut Option<Bytes>,
        end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> Result<Option<Duration>> {
        if let Some(chunk) = body.as_ref() {
            if !chunk.is_empty() {
                ctx.body_bytes += chunk.len();
                ctx.body_chunks += 1;
                for event in ctx.adapter.feed(chunk) {
                    ctx.meter.observe(&event);
                    ctx.count(&event);
                    info!("[tap {}] {:?}", ctx.provider.name(), event);
                }
            }
        }
        if end_of_stream {
            info!(
                "[tap {}] end-of-stream: {} body bytes in {} chunks; events: {}",
                ctx.provider.name(),
                ctx.body_bytes,
                ctx.body_chunks,
                ctx.summary()
            );
            for line in ctx.meter.report().to_string().lines() {
                info!("[tap {}] {}", ctx.provider.name(), line);
            }
        }
        Ok(None)
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cfg = SpikeConfig::load();
    info!("spike-proxy-pingora listening on {} -> upstream {}:{} (tls={}, sni={})",
        cfg.listen, cfg.upstream_host, cfg.upstream_port, cfg.upstream_tls, cfg.sni);

    let mut server = Server::new(None).expect("pingora server init");
    server.bootstrap();

    let listen = cfg.listen.clone();
    let mut proxy = http_proxy_service(&server.configuration, SpikeGateway { cfg });
    proxy.add_tcp(&listen);
    server.add_service(proxy);
    server.run_forever();
}
