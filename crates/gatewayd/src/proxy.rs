//! The config-driven Pingora proxy: route resolution by path prefix,
//! attribution enforcement (GB-1 required keys, GB-2 proven claims, GB-3
//! assigned pins), operator-defined rejections (GB-4), and the streaming
//! tap — every response chunk flows through the provider's adapter and the
//! meter while the identical bytes stream on to the client, nothing
//! buffered whole.
//!
//! Milestone 2: every request binds one `Arc<Snapshot>` at request start
//! (`new_ctx`) and consults ONLY that snapshot for its whole lifetime,
//! streaming included — no torn reads, and an old version drains out with
//! its last in-flight stream (docs/03-hot-swap.md). Every `[req]` and
//! `[meter]` line carries `cfg=vN`: the published bounded-staleness
//! evidence of exactly which version served which stream.
//!
//! The tap and ctx shape are promoted from `spikes/proxy-pingora/src/main.rs`
//! (Phase 0, Spike B); the governance around them is new in Phase 1.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bytes::Bytes;
use log::info;
use pingora::http::{RequestHeader, ResponseHeader};
use pingora::prelude::*;

use gateway_core::adapters::Adapter;
use gateway_core::attribution::{self, Tag};
use gateway_core::config::{self, Config, ProviderKind, RejectionTemplate, ATTR_HEADER_PREFIX};
use gateway_core::event::Event;
use gateway_core::jwt;
use gateway_core::metering::Meter;
use gateway_core::snapshot::Snapshot;
use gateway_core::template;

use crate::reload::SharedSnapshot;

pub struct Gateway {
    /// The swap cell. Touched exactly once per request, in `new_ctx`;
    /// every later hook reads the request's own pinned snapshot instead.
    shared: SharedSnapshot,
}

impl Gateway {
    pub fn new(shared: SharedSnapshot) -> Self {
        Gateway { shared }
    }
}

/// What the matched route pins down for the rest of the request's life.
struct RouteBinding {
    prefix: String,
    provider: String,
    kind: ProviderKind,
}

/// Per-request state: the pinned config snapshot, the chosen adapter, the
/// running meter, the resolved attribution tags, and summary counters.
/// Deliberately bounded — the tap stores counts, never the body. (Promoted
/// shape from Spike B.)
pub struct ReqCtx {
    /// The snapshot this request bound at start. Held for the request's
    /// whole lifetime — including the full streaming response — so a swap
    /// mid-stream never rebinds it, and the old snapshot stays alive until
    /// the last such holder drops (drain semantics, doc 03 limitation 2).
    snapshot: Arc<Snapshot>,
    route: Option<RouteBinding>,
    adapter: Option<Box<dyn Adapter + Send + Sync>>,
    meter: Meter,
    tags: Vec<Tag>,
    body_bytes: usize,
    body_chunks: usize,
    event_counts: [usize; 6],
}

impl ReqCtx {
    fn bound(snapshot: Arc<Snapshot>) -> Self {
        ReqCtx {
            snapshot,
            route: None,
            adapter: None,
            meter: Meter::new(),
            tags: Vec::new(),
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

    fn event_summary(&self) -> String {
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

    /// `key=value(origin)` pairs — the attribution half of the spend join.
    fn tag_summary(&self) -> String {
        self.tags
            .iter()
            .map(|t| format!("{}={}({})", t.key, t.value, t.origin.label()))
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Caller-sent attribution headers (`x-attr-<key>`), first value wins.
fn caller_attrs(head: &RequestHeader) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (name, value) in head.headers.iter() {
        if let Some(key) = name.as_str().strip_prefix(ATTR_HEADER_PREFIX) {
            if let Ok(v) = value.to_str() {
                out.entry(key.to_string()).or_insert_with(|| v.to_string());
            }
        }
    }
    out
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// GB-4: the operator's body verbatim — correct status and content type,
/// never a bare 4xx of our own invention.
async fn respond_rejection(
    session: &mut Session,
    t: &RejectionTemplate,
    vars: &[(&str, &str)],
) -> Result<()> {
    let body = template::render(&t.body, vars);
    let mut header = ResponseHeader::build(t.status, Some(2))?;
    header.insert_header("content-type", t.content_type.clone())?;
    header.insert_header("content-length", body.len().to_string())?;
    session.write_response_header(Box::new(header), false).await?;
    session.write_response_body(Some(Bytes::from(body)), true).await?;
    Ok(())
}

/// GB-2: claims from a verified HS256 token, or `None` (absent header,
/// bad signature, expired — each logged). Only consulted on routes with
/// claim mappings; config validation guarantees `auth` exists for them.
/// Takes the request's pinned config, never the live cell.
fn verified_claims(
    cfg: &Config,
    head: &RequestHeader,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let auth = cfg.auth.as_ref()?;
    let raw = head.headers.get(auth.jwt.header.as_str())?.to_str().ok()?;
    let token = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))?
        .trim();
    match jwt::verify_hs256(token, auth.jwt.hs256_secret.as_bytes(), now_unix()) {
        Ok(claims) => Some(claims),
        Err(e) => {
            info!("[auth] jwt rejected: {e}");
            None
        }
    }
}

#[async_trait]
impl ProxyHttp for Gateway {
    type CTX = ReqCtx;

    fn new_ctx(&self) -> Self::CTX {
        // Atomic per-request binding: ONE load of the current snapshot at
        // request start; every later hook reads ctx.snapshot, so this
        // request can never observe two config versions. The route itself
        // is unknown until the request headers are visible; request_filter
        // fills the binding (same dance as the spike).
        ReqCtx::bound(self.shared.load())
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
        // The pinned snapshot, cloned out of ctx so the borrow checker lets
        // the rejection paths below take &mut ctx state (an Arc clone: same
        // snapshot, refcount bump).
        let snap = ctx.snapshot.clone();
        let cfg = &snap.config;
        let v = snap.version;

        let head = session.req_header();
        let method = head.method.clone();
        let raw_path = head.uri.path().to_owned();

        // Dot-segment defense (GB-1/GB-2): resolve `.`/`..` (literal and
        // %2e-encoded) and merge duplicate slashes BEFORE choosing a route,
        // then rewrite the request so the upstream sees the same resolved
        // path. Otherwise `/openai/../claims/...` matches the weaker
        // /openai contract while an upstream that collapses dot-segments
        // serves /claims/... — a caller-chosen contract downgrade that
        // smuggles forged x-attr-* tags past the /claims route's rules.
        let path = config::normalize_path(&raw_path);
        if path != raw_path {
            let query = session.req_header().uri.query().map(str::to_owned);
            let target = match &query {
                Some(q) => format!("{path}?{q}"),
                None => path.clone(),
            };
            match http::Uri::try_from(target.as_str()) {
                Ok(uri) => {
                    info!("[req] path normalized: {raw_path} -> {path} cfg=v{v}");
                    session.req_header_mut().set_uri(uri);
                }
                Err(e) => {
                    // Unreachable for a path assembled from valid path
                    // bytes, but a request-path panic is never acceptable:
                    // refuse to route what we cannot represent (GB-4
                    // template, operator's body).
                    info!(
                        "[req] {method} {raw_path} -> unrepresentable after normalization ({e}) \
                         (rejecting: unknown_route) cfg=v{v}"
                    );
                    respond_rejection(
                        session,
                        &cfg.rejections.unknown_route,
                        &[("route", raw_path.as_str())],
                    )
                    .await?;
                    return Ok(true);
                }
            }
        }

        // Route resolution by longest path prefix; unknown → GB-4 template.
        let Some(route) = cfg.match_route(&path) else {
            info!("[req] {method} {path} -> no route (rejecting: unknown_route) cfg=v{v}");
            respond_rejection(
                session,
                &cfg.rejections.unknown_route,
                &[("route", path.as_str())],
            )
            .await?;
            return Ok(true);
        };

        let claims = if route.attribution.from_claims.is_empty() {
            None
        } else {
            verified_claims(cfg, session.req_header())
        };
        let caller = caller_attrs(session.req_header());

        // GB-1: every required key satisfied (assigned, proven, or caller)
        // or the request never reaches the upstream.
        let tags = match attribution::resolve(
            &route.attribution,
            |key| caller.get(key).cloned(),
            claims.as_ref(),
        ) {
            Ok(tags) => tags,
            Err(missing) => {
                let missing_list = missing.join(", ");
                info!(
                    "[req] {method} {path} -> route={} (rejecting: missing_attribution: {missing_list}) cfg=v{v}",
                    route.prefix
                );
                respond_rejection(
                    session,
                    &cfg.rejections.missing_attribution,
                    &[("key", missing_list.as_str()), ("route", route.prefix.as_str())],
                )
                .await?;
                return Ok(true);
            }
        };

        let kind = cfg.providers[&route.provider].kind;
        info!(
            "[req] {method} {path} -> route={} provider={}({}) cfg=v{v}",
            route.prefix,
            route.provider,
            kind.name()
        );
        // Attribution values logged per request, origin included — the
        // "who is spending" half of the join, before a single token flows.
        ctx.route = Some(RouteBinding {
            prefix: route.prefix.clone(),
            provider: route.provider.clone(),
            kind,
        });
        ctx.adapter = Some(kind.new_adapter());
        ctx.tags = tags;
        info!("[attr {}] {} cfg=v{v}", route.prefix, ctx.tag_summary());
        Ok(false)
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        let binding = ctx
            .route
            .as_ref()
            .ok_or_else(|| Error::explain(InternalError, "upstream_peer without a bound route"))?;
        // The request's pinned snapshot, never the live cell: a swap between
        // request_filter and here must not retarget this request.
        let up = &ctx.snapshot.config.providers[&binding.provider].upstream;
        let peer = HttpPeer::new(
            format!("{}:{}", up.host, up.port),
            up.tls,
            up.sni().to_string(),
        );
        Ok(Box::new(peer))
    }

    async fn upstream_request_filter(
        &self,
        _session: &mut Session,
        upstream_request: &mut RequestHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        // Only the resolved contract crosses this boundary: strip EVERY
        // caller-sent x-attr-* header, then insert the resolved tags. A key
        // outside the route's contract (neither required, pinned, nor
        // claim-mapped) never reaches the upstream, so a caller cannot
        // smuggle attribution the gateway never adjudicated.
        let stray: Vec<_> = upstream_request
            .headers
            .keys()
            .filter(|name| name.as_str().starts_with(ATTR_HEADER_PREFIX))
            .cloned()
            .collect();
        for name in stray {
            upstream_request.remove_header(&name);
        }
        // GB-3: the gateway ASSIGNS. Everything x-attr-* was removed above,
        // so a pinned or proven tag can never be spoofed past this point —
        // assigned, not believed.
        for tag in &ctx.tags {
            upstream_request
                .insert_header(format!("{ATTR_HEADER_PREFIX}{}", tag.key), tag.value.clone())?;
        }
        Ok(())
    }

    /// The tap (promoted from Spike B). Pingora hands each body chunk as
    /// `&mut Option<Bytes>` on its way downstream; we feed a copy of the
    /// bytes to the adapter and leave the option untouched, so the client
    /// receives the identical stream at the identical cadence.
    fn response_body_filter(
        &self,
        _session: &mut Session,
        body: &mut Option<Bytes>,
        end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> Result<Option<Duration>> {
        // `kind` is Copy; taking it up front keeps the borrow checker happy
        // while the counters below take `&mut ctx`.
        let Some(kind) = ctx.route.as_ref().map(|b| b.kind) else {
            return Ok(None); // rejected before proxying; nothing to tap
        };
        if let Some(chunk) = body.as_ref() {
            if !chunk.is_empty() {
                ctx.body_bytes += chunk.len();
                ctx.body_chunks += 1;
                let mut adapter = ctx.adapter.take();
                if let Some(adapter) = adapter.as_mut() {
                    for event in adapter.feed(chunk) {
                        ctx.meter.observe(&event);
                        ctx.count(&event);
                        info!("[tap {}] {:?}", kind.name(), event);
                    }
                }
                ctx.adapter = adapter;
            }
        }
        if end_of_stream {
            let binding = ctx.route.as_ref().expect("route bound above");
            // The attribution→spend join: tags and token counts on ONE
            // line, so "who spent what" is a grep, not a correlation.
            // cfg=vN names the version that metered THIS stream — the
            // bounded-staleness evidence during a drain overlap.
            let report = ctx.meter.report();
            info!(
                "[meter {}] cfg=v{} provider={}({}) attribution{{{}}} events{{{}}} chunks={} bytes={} \
                 est_output_tokens={} auth_input_tokens={} auth_output_tokens={} est_err={}",
                binding.prefix,
                ctx.snapshot.version,
                binding.provider,
                binding.kind.name(),
                ctx.tag_summary(),
                ctx.event_summary(),
                ctx.body_chunks,
                ctx.body_bytes,
                report.estimated_output_tokens,
                opt(report.authoritative_input_tokens),
                opt(report.authoritative_output_tokens),
                report
                    .error_pct
                    .map(|p| format!("{p:+.1}%"))
                    .unwrap_or_else(|| "n/a".to_string()),
            );
        }
        Ok(None)
    }
}

fn opt(n: Option<u64>) -> String {
    n.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reload::testutil::{temp_cfg, valid_yaml};
    use crate::reload::{ReloadOutcome, Reloader};

    /// The proxy-level half of the drain contract: `new_ctx` is the ONLY
    /// place a request touches the live cell, so a ctx created before a
    /// swap keeps its version for its whole lifetime while a ctx created
    /// after binds the new one — two versions live side by side.
    #[test]
    fn request_ctx_pins_the_snapshot_bound_at_request_start() {
        let path = temp_cfg(&valid_yaml("prod"));
        let reloader = Reloader::bootstrap(path.clone()).unwrap();
        let gateway = Gateway::new(reloader.shared());

        // Request A starts under v1...
        let ctx_a = gateway.new_ctx();
        assert_eq!(ctx_a.snapshot.version, 1);

        // ...the operator swaps to v2 while A is still streaming...
        std::fs::write(&path, valid_yaml("canary")).unwrap();
        assert_eq!(reloader.reload("test"), ReloadOutcome::Swapped { old: 1, new: 2 });

        // ...and a concurrent request B binds v2 while A still sees v1,
        // config content included: no torn read, no mid-stream rebind.
        let ctx_b = gateway.new_ctx();
        assert_eq!(ctx_a.snapshot.version, 1);
        assert_eq!(ctx_a.snapshot.config.routes[0].attribution.pinned["env"], "prod");
        assert_eq!(ctx_b.snapshot.version, 2);
        assert_eq!(ctx_b.snapshot.config.routes[0].attribution.pinned["env"], "canary");
    }
}
