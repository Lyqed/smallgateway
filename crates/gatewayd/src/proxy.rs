//! The config-driven Pingora proxy: route resolution (path prefix + CEL
//! conditions), the scoped attribution contract (GB-1/2/3 + CEL-derived +
//! app-scope), operator rejections (GB-4), Vertex labels (GB-8), STS
//! session-tag credentials + SigV4 (GB-7), tier-2 signed-WASM policy hooks
//! (Phase 4), and the streaming tap — every chunk flows through the adapter
//! and the meter while the identical bytes stream on, nothing buffered whole.
//!
//! Every request binds one `Arc<Snapshot>` at request start (`new_ctx`) and,
//! Phase 4, the WASM module set paired with it, consulting ONLY that binding
//! for its whole life — no torn reads, and the old version drains out with its
//! last in-flight stream (docs/03-hot-swap.md). Every `[req]`/`[meter]` line
//! carries `cfg=vN`. request_filter: normalize path → CEL context → route →
//! resolve attribution (fleet→project→route, then route⊕app) → GB-1 → GB-5
//! admit → GB-8 labels → GB-7 credentials → WASM on_request. The per-request
//! helpers live in `proxy_support`/`proxy_stream`/`proxy_wasm` to keep this
//! module focused on the `ProxyHttp` trait impl.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use log::{error, info};
use pingora::http::RequestHeader;
use pingora::prelude::*;

use gateway_core::adapters::Adapter;
use gateway_core::attribution::Tag;
use gateway_core::aws::{CredentialCache, Credentials};
use gateway_core::budget::CapId;
use gateway_core::config::{ProviderKind, ATTR_HEADER_PREFIX};
use gateway_core::event::Event;
use gateway_core::labels;
use gateway_core::metering::Meter;
use gateway_core::snapshot::Snapshot;

use gateway_wasm::{BoundModules, Hook};

use crate::aws_auth;
use crate::budget::NodeBudgets;
use crate::proxy_support::{
    caller_attrs, eval_ctx, now_unix, respond_rejection, verified_claims,
};
use crate::reload::SharedSnapshot;
use crate::wasm_runtime::WasmRuntime;

pub struct Gateway {
    /// The swap cell. Touched exactly once per request, in `new_ctx`;
    /// every later hook reads the request's own pinned snapshot instead.
    shared: SharedSnapshot,
    /// Tier-2 (Phase 4): the signed-WASM policy runtime. Bound ONCE per request
    /// in `new_ctx`, paired atomically with the snapshot version — config vN and
    /// module set vN together, no torn read (docs/04). `None` -> no runtime.
    wasm: Option<WasmRuntime>,
    /// GB-7: credentials per unique (role, endpoint, tag-set). Lives
    /// across config swaps on purpose — the key carries every input that
    /// changes the minted credentials.
    sts_cache: CredentialCache,
    /// GB-8: SA bearer tokens per (sa, scopes, pool, provider). Same
    /// cross-swap lifetime rationale as the STS cache.
    gcp_tokens: crate::gcp_auth::GcpTokenCache,
    /// GB-5: the node-local budget counters (shared with the control-plane
    /// client, which reports spend and applies share grants). Lives OUTSIDE
    /// the snapshot on purpose — a config swap changes the CAP a request reads
    /// (from its pinned policy), never the running counters (docs/03
    /// limitation 3: a cap tightened mid-stream does not retroactively apply;
    /// the counters carry across swaps).
    budgets: Arc<NodeBudgets>,
}

impl Gateway {
    /// Standalone convenience constructor with a fresh single-node budget set
    /// (used by the proxy unit tests; `main` builds the shared budgets and uses
    /// [`Gateway::with_budgets`] so file and control-plane modes share one set).
    #[allow(dead_code)]
    pub fn new(shared: SharedSnapshot) -> Self {
        Self::with_budgets(
            shared,
            Arc::new(NodeBudgets::new(
                "standalone",
                Box::new(crate::budget::LogWebhookSink),
            )),
        )
    }

    /// Build with a shared [`NodeBudgets`] — control-plane mode passes the same
    /// `Arc` the client loop reports/rebalances through. No wasm runtime.
    pub fn with_budgets(shared: SharedSnapshot, budgets: Arc<NodeBudgets>) -> Self {
        Gateway {
            shared,
            wasm: None,
            sts_cache: CredentialCache::new(),
            gcp_tokens: crate::gcp_auth::GcpTokenCache::new(),
            budgets,
        }
    }

    /// Attach the tier-2 WASM runtime (Phase 4). `main` builds it from the
    /// bootstrap snapshot and the config dir, then calls this.
    pub fn with_wasm(mut self, wasm: WasmRuntime) -> Self {
        self.wasm = Some(wasm);
        self
    }

}

/// What the matched route pins down for the rest of the request's life.
struct RouteBinding {
    prefix: String,
    provider: String,
    kind: ProviderKind,
}

/// GB-7 material carried from request_filter to upstream_request_filter.
struct AwsSigning {
    creds: Credentials,
    region: String,
}

/// Per-request state: the pinned config snapshot, the chosen adapter, the
/// running meter, the resolved attribution tags, and summary counters.
/// Deliberately bounded — the response tap stores counts, never the body;
/// the request body is buffered ONLY on labeled vertex routes (GB-8 must
/// rewrite it). (Promoted shape from Spike B.)
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
    /// GB-8: resolved operator labels to merge into the request body.
    vertex_labels: Option<Vec<(String, String)>>,
    /// Operator-forced injection (guardrails and friends), resolved per
    /// request: headers applied (and SIGNED, on signing providers) in
    /// `upstream_request_filter`; body fields merged in the buffered path.
    forced: Option<crate::proxy_support::ResolvedInjection>,
    /// Request-body accumulator (labeled vertex routes and forced-body
    /// routes only).
    body_buf: Vec<u8>,
    /// GB-7: credentials + region for SigV4 signing.
    aws: Option<AwsSigning>,
    /// GB-8 auth: the minted SA bearer for vertex providers with an auth
    /// chain; applied as `Authorization: Bearer` in upstream_request_filter.
    gcp_bearer: Option<String>,
    /// Signed-payload providers (bedrock+sts): the COMPLETE request body,
    /// read and forced-injected in request_filter BEFORE Authorization is
    /// computed, so the SigV4 payload hash covers exactly these bytes.
    /// Re-injected as the upstream body in request_body_filter.
    signed_body: Option<Bytes>,
    body_bytes: usize,
    body_chunks: usize,
    event_counts: [usize; 6],
    /// GB-5: the capped spenders this request bills — one per resolved
    /// attribution tag that has a composed cap. `(CapId, cap_tokens)`.
    caps: Vec<(CapId, gateway_core::budget::CapTerms)>,
    /// GB-5: the last estimated-output-token reading fed to the budget, so each
    /// tap computes the INCREMENT since the previous chunk (the Meter's
    /// estimate is cumulative). Reconciled to the authoritative frame at end.
    last_metered_est: u64,
    /// GB-5: set once a mid-stream cut fires. Further chunks are suppressed and
    /// the operator's terminal event is emitted in place of continued content.
    cut: bool,
    /// Phase 4: the WASM module set bound to THIS request, paired atomically
    /// with `snapshot` and held for the request's life so an in-flight stream
    /// keeps its module version until it drains (docs/03 limitation 2). `None`
    /// -> no wasm runtime.
    modules: Option<BoundModules>,
    /// Phase 4: header mutations a WASM `on_request` hook returned, applied in
    /// `upstream_request_filter` after the resolved-tag insertion.
    wasm_header_set: BTreeMap<String, String>,
    wasm_header_remove: Vec<String>,
    /// Phase 4: whether the per-event WASM hook runs (config gate AND a module
    /// implements on_response_event), resolved ONCE at bind — the hot loop does
    /// one bool check, not a lookup per event.
    wasm_per_event: bool,
}

impl ReqCtx {
    fn bound(snapshot: Arc<Snapshot>, modules: Option<BoundModules>, per_event: bool) -> Self {
        // The per-event hot-path gate resolved ONCE: the config must enable
        // per-event hooks AND a bound module must implement on_response_event.
        // Either false -> the tap never touches wasm per event (zero cost).
        let wasm_per_event = per_event
            && modules
                .as_ref()
                .is_some_and(|m| m.wants(Hook::OnResponseEvent));
        ReqCtx {
            snapshot,
            route: None,
            adapter: None,
            meter: Meter::new(),
            tags: Vec::new(),
            vertex_labels: None,
            forced: None,
            body_buf: Vec::new(),
            aws: None,
            gcp_bearer: None,
            signed_body: None,
            body_bytes: 0,
            body_chunks: 0,
            event_counts: [0; 6],
            caps: Vec::new(),
            last_metered_est: 0,
            cut: false,
            modules,
            wasm_header_set: BTreeMap::new(),
            wasm_header_remove: Vec::new(),
            wasm_per_event,
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
#[async_trait]
impl ProxyHttp for Gateway {
    type CTX = ReqCtx;

    fn new_ctx(&self) -> Self::CTX {
        // Atomic per-request binding: ONE load of the current snapshot at
        // request start; every later hook reads ctx.snapshot, so this
        // request can never observe two config versions.
        let snapshot = self.shared.load();
        // Phase 4: bind the module set for EXACTLY this snapshot version. The
        // runtime stored vN's modules before advancing the snapshot cell to vN,
        // so config and modules bind together — no torn read (docs/04).
        let (modules, per_event) = match &self.wasm {
            Some(rt) => (Some(rt.bind(snapshot.version, now_unix())), rt.per_event_enabled()),
            None => (None, false),
        };
        ReqCtx::bound(snapshot, modules, per_event)
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
        // The pinned snapshot, cloned out of ctx (an Arc clone) so the rejection
        // paths below can take &mut ctx state.
        let snap = ctx.snapshot.clone();
        let cfg = &snap.config;
        let v = snap.version;

        let head = session.req_header();
        let method = head.method.clone();
        let raw_path = head.uri.path().to_owned();

        // Dot-segment defense (GB-1/GB-2): the helper normalizes + rewrites the
        // URI so a traversal spelling can never select a weaker contract; the
        // unrepresentable case rejects with the GB-4 unknown_route template.
        let path = match crate::proxy_support::normalize_and_rewrite(session, &raw_path, v) {
            Ok(path) => path,
            Err(()) => {
                info!(
                    "[req] {method} {raw_path} -> unrepresentable after normalization \
                     (rejecting: unknown_route) cfg=v{v}"
                );
                respond_rejection(session, &cfg.rejections.unknown_route, &[("route", raw_path.as_str())])
                    .await?;
                return Ok(true);
            }
        };

        // Verified claims feed claim mappings, CEL conditions/derivations,
        // and label expressions alike.
        let claims = verified_claims(cfg, session.req_header());
        let cel_ctx = eval_ctx(session.req_header(), &path, claims.as_ref());

        // Route resolution: longest prefix + CEL condition; unknown → the
        // fleet-scope GB-4 template (no route matched, so no scoped one).
        let Some(route) = cfg.match_route(&path, &cel_ctx) else {
            info!("[req] {method} {path} -> no route (rejecting: unknown_route) cfg=v{v}");
            respond_rejection(
                session,
                &cfg.rejections.unknown_route,
                &[("route", path.as_str())],
            )
            .await?;
            return Ok(true);
        };
        let caller = caller_attrs(session.req_header());

        // Two-phase resolution (fleet→project→route, then route⊕app if the
        // resolved apps.key value selects an override) — the loop lives in the
        // helper; `policy` is the FINAL policy enforcement runs against.
        let (policy, resolution) = crate::proxy_support::resolve_with_app_scope(
            route,
            cfg.apps.as_ref(),
            &caller,
            claims.as_ref(),
            &cel_ctx,
        );

        // GB-1 enforcement against the FINAL policy: every required key
        // satisfied (assigned, proven, derived, or caller) or the request
        // never reaches the upstream — with the effective scope's template.
        if !resolution.ok() {
            let missing_list = resolution.missing.join(", ");
            info!(
                "[req] {method} {path} -> route={} (rejecting: missing_attribution: {missing_list}) cfg=v{v}",
                route.prefix
            );
            respond_rejection(
                session,
                &policy.missing_attribution,
                &[("key", missing_list.as_str()), ("route", route.prefix.as_str())],
            )
            .await?;
            return Ok(true);
        }
        let tags = resolution.tags;

        // GB-5: admit against the node-local budget for every capped tag before
        // the upstream (loop in the helper); a value at its cap rejects with the
        // GB-4 template, naming the exhausted spender.
        let caps = match crate::proxy_stream::admit_caps(&self.budgets, policy, &tags, &route.prefix, v, now_unix()) {
            Ok(caps) => caps,
            Err(d) => {
                info!(
                    "[gb5 {}] {} DENIED at admission: spent {}/{} tokens \
                     (rejecting: missing_attribution) cfg=v{v}",
                    route.prefix, d.id, d.spent, d.cap
                );
                respond_rejection(
                    session,
                    &policy.missing_attribution,
                    &[
                        ("key", d.id.to_string().as_str()),
                        ("route", route.prefix.as_str()),
                        ("cap", d.cap.to_string().as_str()),
                        ("spend", d.spent.to_string().as_str()),
                    ],
                )
                .await?;
                return Ok(true);
            }
        };

        let kind = cfg.providers[&route.provider].kind;

        // GB-8: resolve the effective labels now — fail closed BEFORE the
        // upstream sees anything. The body merge happens in
        // request_body_filter.
        if kind == ProviderKind::Vertex && !policy.labels.is_empty() {
            match crate::proxy_support::resolve_vertex_labels(&policy.labels, &tags, &cel_ctx) {
                Ok(resolved) => {
                    info!(
                        "[gb8 {}] labels{{{}}} cfg=v{v}",
                        route.prefix,
                        resolved
                            .iter()
                            .map(|(k, val)| format!("{k}={val}"))
                            .collect::<Vec<_>>()
                            .join(","),
                    );
                    ctx.vertex_labels = Some(resolved);
                }
                Err(e) => {
                    info!(
                        "[req] {method} {path} -> route={} (rejecting: unresolvable label: {e}) cfg=v{v}",
                        route.prefix
                    );
                    respond_rejection(
                        session,
                        &policy.missing_attribution,
                        &[("key", e.key.as_str()), ("route", route.prefix.as_str())],
                    )
                    .await?;
                    return Ok(true);
                }
            }
        }

        // GB-7: session-tag credentials for bedrock providers with sts. The
        // resolution + exchange live in the helper; here we map its two
        // fail-closed paths (GB-4 reject vs 502) onto responses.
        if let Some(sts) = &cfg.providers[&route.provider].sts {
            use crate::proxy_support::{resolve_gb7_credentials, Gb7Failure};
            match resolve_gb7_credentials(&self.sts_cache, sts, &tags, now_unix()).await {
                Ok((creds, region, session_tags, cached)) => {
                    info!(
                        "[gb7 {}] session_tags{{{}}} access_key={} cache={} cfg=v{v}",
                        route.prefix,
                        session_tags
                            .iter()
                            .map(|(k, val)| format!("{k}={val}"))
                            .collect::<Vec<_>>()
                            .join(","),
                        creds.access_key_id,
                        if cached { "hit" } else { "miss" },
                    );
                    ctx.aws = Some(AwsSigning { creds, region });
                }
                Err(Gb7Failure::Reject { key, reason }) => {
                    info!(
                        "[req] {method} {path} -> route={} (rejecting: session tag: {reason}) cfg=v{v}",
                        route.prefix
                    );
                    respond_rejection(
                        session,
                        &policy.missing_attribution,
                        &[("key", key.as_str()), ("route", route.prefix.as_str())],
                    )
                    .await?;
                    return Ok(true);
                }
                Err(Gb7Failure::Exchange(e)) => {
                    // A minted-identity failure is a 502 (infra), loudly logged
                    // — never the operator's GB-4 body.
                    error!("[gb7 {}] credential exchange FAILED: {e} cfg=v{v}", route.prefix);
                    return Err(Error::explain(
                        HTTPStatus(502),
                        format!("sts credential exchange failed: {e}"),
                    ));
                }
            }
        }

        // GB-8 auth: mint (or cache-hit) the SA bearer for vertex providers
        // with an auth chain. An exchange failure is a 502 (infra, loud),
        // never the operator's GB-4 body — same posture as GB-7.
        if let Some(auth) = &cfg.providers[&route.provider].auth {
            match crate::gcp_auth::bearer_for(&self.gcp_tokens, auth, now_unix()).await {
                Ok((bearer, cached)) => {
                    info!(
                        "[gb8-auth {}] sa={} cache={} cfg=v{v}",
                        route.prefix,
                        auth.service_account_email,
                        if cached { "hit" } else { "miss" },
                    );
                    ctx.gcp_bearer = Some(bearer);
                }
                Err(e) => {
                    error!("[gb8-auth {}] token mint FAILED: {e} cfg=v{v}", route.prefix);
                    return Err(Error::explain(
                        HTTPStatus(502),
                        format!("gcp token exchange failed: {e}"),
                    ));
                }
            }
        }

        // Operator-forced injection: resolve now, fail closed BEFORE the
        // upstream sees anything. A caller can never pick the values (no
        // allow-gate here; guardrails are pure operator policy).
        if let Some(inject) = &cfg.providers[&route.provider].inject {
            match crate::proxy_support::resolve_injection(inject, &tags) {
                Ok(resolved) => {
                    info!(
                        "[inject {}] {} header(s), {} body field(s) cfg=v{v}",
                        route.prefix,
                        resolved.headers.len(),
                        resolved.body.len(),
                    );
                    ctx.forced = Some(resolved);
                }
                Err((key, reason)) => {
                    info!(
                        "[req] {method} {path} -> route={} (rejecting: injection: {reason}) cfg=v{v}",
                        route.prefix
                    );
                    respond_rejection(
                        session,
                        &policy.missing_attribution,
                        &[("key", key.as_str()), ("route", route.prefix.as_str())],
                    )
                    .await?;
                    return Ok(true);
                }
            }
        }

        // Signed-payload path (bedrock+sts): read the WHOLE request body now
        // and apply forced-body injection BEFORE the Authorization is
        // computed, so the SigV4 payload hash covers the final bytes. Bedrock
        // request bodies are small JSON (the RESPONSE is the stream), so
        // buffering here is bounded and correct.
        if kind == ProviderKind::Bedrock && cfg.providers[&route.provider].sts.is_some() {
            // Retry buffering is pingora's own consumed-body replay: the
            // pump re-sends the buffered body with end-of-body set, which
            // is what routes it through request_body_filter (where the
            // finalized signed bytes replace it). Without this the pump
            // idles forever waiting for a body we already consumed.
            session.enable_retry_buffering();
            let mut buf: Vec<u8> = Vec::new();
            while let Some(chunk) = session.read_request_body().await? {
                buf.extend_from_slice(&chunk);
            }
            let forced_body = ctx
                .forced
                .as_ref()
                .map(|f| f.body.as_slice())
                .unwrap_or(&[]);
            let merged = if forced_body.is_empty() || buf.is_empty() {
                buf
            } else {
                match labels::inject_into_body(&buf, forced_body) {
                    Ok(m) => m,
                    Err(e) => {
                        info!(
                            "[req] {method} {path} -> route={} (rejecting: body: {e}) cfg=v{v}",
                            route.prefix
                        );
                        respond_rejection(
                            session,
                            &policy.missing_attribution,
                            &[("key", "body"), ("route", route.prefix.as_str())],
                        )
                        .await?;
                        return Ok(true);
                    }
                }
            };
            ctx.signed_body = Some(Bytes::from(merged));
        }

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
        ctx.caps = caps;
        info!("[attr {}] {} cfg=v{v}", route.prefix, ctx.tag_summary());

        // Phase 4: tier-2 WASM `on_request` chain (adjudicated request in,
        // continue/mutate/reject out; a fault fails CLOSED to reject). Runs
        // AFTER attribution; the decision lives in the helper, the async GB-4
        // reject stays here.
        let wasm_outcome = ctx.modules.as_ref().and_then(|modules| {
            crate::proxy_wasm::apply_on_request(
                modules,
                method.as_str(),
                &path,
                &cel_ctx.headers,
                &ctx.tags,
                &route.prefix,
                v,
            )
        });
        match wasm_outcome {
            Some(crate::proxy_wasm::RequestOutcome::Proceed { header_set, header_remove }) => {
                ctx.wasm_header_set = header_set;
                ctx.wasm_header_remove = header_remove;
            }
            Some(crate::proxy_wasm::RequestOutcome::Reject { reason }) => {
                respond_rejection(
                    session,
                    &policy.missing_attribution,
                    &[("key", reason.as_str()), ("route", route.prefix.as_str())],
                )
                .await?;
                return Ok(true);
            }
            None => {}
        }
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
        // outside the route's contract (neither required, pinned,
        // claim-mapped, nor derived) never reaches the upstream, so a
        // caller cannot smuggle attribution the gateway never adjudicated.
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

        // Phase 4: apply the WASM `on_request` header transform on the
        // adjudicated request (removals then sets).
        crate::proxy_wasm::apply_header_mutations(
            upstream_request,
            &ctx.wasm_header_set,
            &ctx.wasm_header_remove,
        )?;

        // GB-8 auth: the gateway's own minted bearer replaces whatever
        // Authorization the caller sent — the credential is the operator's,
        // never the caller's.
        if let Some(bearer) = &ctx.gcp_bearer {
            upstream_request.insert_header("authorization", format!("Bearer {bearer}"))?;
        }

        // Operator-forced headers: applied BEFORE signing so the signature
        // covers them (operator wins over any caller value: insert_header
        // overrides).
        if let Some(forced) = &ctx.forced {
            for (name, value) in &forced.headers {
                upstream_request.insert_header(name.clone(), value.clone())?;
            }
        }

        // GB-8 labels / forced body fields on NON-signed providers: the body
        // will be rewritten in request_body_filter, so its length changes —
        // switch the upstream leg to chunked framing. Signed-payload
        // providers already finalized their body in request_filter and skip
        // this entirely.
        let rewrites_body = ctx.signed_body.is_none()
            && (ctx.vertex_labels.is_some()
                || ctx.forced.as_ref().is_some_and(|f| !f.body.is_empty()));
        if rewrites_body {
            upstream_request.remove_header("content-length");
            upstream_request.insert_header("transfer-encoding", "chunked")?;
        }

        // Signed-payload path: the finalized body's length is known — plain
        // content-length framing, and the payload hash below covers it.
        if let Some(body) = &ctx.signed_body {
            upstream_request.remove_header("transfer-encoding");
            upstream_request.insert_header("content-length", body.len().to_string())?;
        }

        // GB-7: SigV4 with the session-tagged credentials, over the REAL
        // payload hash. Forced headers enter the SIGNED set: a stripped or
        // altered guardrail header fails verification instead of silently
        // passing unsigned.
        if let Some(aws) = &ctx.aws {
            let binding = ctx.route.as_ref().expect("route bound in request_filter");
            let up = &ctx.snapshot.config.providers[&binding.provider].upstream;
            let extra_signed: &[(String, String)] = ctx
                .forced
                .as_ref()
                .map(|f| f.headers.as_slice())
                .unwrap_or(&[]);
            let payload_hash = gateway_core::aws::sha256_hex(
                ctx.signed_body.as_deref().unwrap_or(&[]),
            );
            aws_auth::sign_bedrock_request(
                upstream_request,
                up,
                &aws.region,
                &aws.creds,
                now_unix(),
                &payload_hash,
                extra_signed,
            )?;
        }
        Ok(())
    }

    /// GB-8's merge point: on labeled vertex routes the request body is
    /// buffered (these are small JSON `generateContent` requests, and the
    /// operator's labels MUST be inside them), merged, and forwarded as
    /// one chunk. Unlabeled routes stream through untouched. A body that
    /// is not a JSON object is refused with a plain 400 — no spend can
    /// have occurred, and Vertex itself would reject it anyway.
    async fn request_body_filter(
        &self,
        _session: &mut Session,
        body: &mut Option<Bytes>,
        end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        // Signed-payload providers: the downstream body was consumed and
        // finalized in request_filter; re-inject those exact bytes (the
        // ones the signature covers) as the upstream body.
        if ctx.signed_body.is_some() {
            *body = ctx.signed_body.take();
            return Ok(());
        }

        let has_labels = ctx.vertex_labels.is_some();
        let forced_body = ctx
            .forced
            .as_ref()
            .map(|f| f.body.as_slice())
            .unwrap_or(&[]);
        if !has_labels && forced_body.is_empty() {
            return Ok(()); // no body rewrite on this route: pass through
        }
        if let Some(chunk) = body.take() {
            ctx.body_buf.extend_from_slice(&chunk);
        }
        if !end_of_stream {
            return Ok(());
        }
        // GB-8 labels first, then operator-forced fields (injection is
        // provider-level policy and applies last, so it wins on overlap).
        let result = match &ctx.vertex_labels {
            Some(operator_labels) => labels::merge_into_body(&ctx.body_buf, operator_labels),
            None => Ok(ctx.body_buf.clone()),
        }
        .and_then(|merged| labels::inject_into_body(&merged, forced_body));
        match result {
            Ok(merged) => {
                let binding = ctx.route.as_ref().expect("route bound");
                info!(
                    "[inject {}] rewrote request body: {} label(s), {} forced field(s) ({} -> {} bytes)",
                    binding.prefix,
                    ctx.vertex_labels.as_ref().map(Vec::len).unwrap_or(0),
                    forced_body.len(),
                    ctx.body_buf.len(),
                    merged.len(),
                );
                ctx.body_buf.clear();
                *body = Some(Bytes::from(merged));
                Ok(())
            }
            Err(e) => {
                error!("[inject] request body rejected: {e}");
                Err(Error::explain(
                    HTTPStatus(400),
                    format!("request body must be a JSON object: {e}"),
                ))
            }
        }
    }

    /// The tap (promoted from Spike B): feed each downstream body chunk to the
    /// adapter + meter, run the gated per-event WASM hook, and charge/cut GB-5
    /// spenders. A cap tightened mid-stream does NOT retroactively apply — this
    /// meters the version the request bound (docs/03 limitation 2); the live
    /// estimate reconciles to the provider's terminal frame at stream end.
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

        // Once cut, suppress all further upstream content: the client already
        // received the terminal event; nothing else should reach it.
        if ctx.cut && !end_of_stream {
            *body = None;
            return Ok(None);
        }

        // Phase 4: the per-event WASM decision, collected while the adapter
        // borrow is held, applied after. A cut from a module short-circuits
        // exactly like GB-5's mid-stream cut (shared machinery).
        let mut wasm_cut_reason: Option<String> = None;
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

                        // The HOT-PATH WASM hook, DOUBLE-GATED (config
                        // per_event_hooks AND a module implements the hook,
                        // resolved once into `wasm_per_event`). Off -> one bool
                        // check, zero marshalling: the measured budget gate
                        // (docs/04; ~12.7us/event when on, see gateway-wasm README).
                        if ctx.wasm_per_event && wasm_cut_reason.is_none() {
                            if let Some(modules) = &ctx.modules {
                                wasm_cut_reason = crate::proxy_wasm::event_cut_reason(
                                    modules,
                                    &event,
                                    ctx.meter.estimated_output_tokens(),
                                );
                            }
                        }
                    }
                }
                ctx.adapter = adapter;
            }
        }

        // A WASM per-event cut and the GB-5 mid-stream cut both use the SAME
        // GB-4 terminal-event machinery: replace the outgoing chunk with the
        // operator's streaming template and latch `ctx.cut` so later chunks are
        // suppressed. The bound snapshot's streaming template is shared by both.
        let streaming = ctx
            .snapshot
            .config
            .rejections
            .missing_attribution
            .streaming
            .clone();
        let route_prefix = ctx.route.as_ref().map(|b| b.prefix.clone()).unwrap_or_default();
        if let Some(reason) = wasm_cut_reason {
            if !ctx.cut {
                error!(
                    "[wasm {route_prefix}] on_response_event -> CUT ({reason}); GB-4 terminal \
                     event cfg=v{}",
                    ctx.snapshot.version
                );
                *body = Some(crate::proxy_wasm::render_wasm_cut(streaming.as_ref(), &reason, &route_prefix));
                ctx.cut = true;
            }
        }

        // GB-5 mid-stream enforcement: charge the estimated-output-token
        // increment against every capped spender; the first to cross its bound
        // cuts (the loop lives in the helper so this hot method stays focused).
        if !ctx.caps.is_empty() && !ctx.cut {
            let est = ctx.meter.estimated_output_tokens();
            let delta = est.saturating_sub(ctx.last_metered_est);
            ctx.last_metered_est = est;
            let caps = ctx.caps.clone();
            if let Some(cut) = crate::proxy_stream::charge_caps_and_cut(
                &self.budgets, &caps, delta, streaming.as_ref(), &route_prefix, ctx.snapshot.version,
                now_unix(),
            ) {
                *body = Some(cut);
                ctx.cut = true;
            }
        }

        if end_of_stream {
            let binding = ctx.route.as_ref().expect("route bound above");
            // The attribution→spend join: tags and token counts on ONE
            // line, so "who spent what" is a grep, not a correlation.
            // cfg=vN names the version that metered THIS stream — the
            // bounded-staleness evidence during a drain overlap.
            let report = ctx.meter.report();
            crate::proxy_stream::log_meter_report(
                &binding.prefix,
                ctx.snapshot.version,
                &binding.provider,
                binding.kind.name(),
                &ctx.tag_summary(),
                &ctx.event_summary(),
                ctx.body_chunks,
                ctx.body_bytes,
                &report,
            );

            // Phase 4: the `on_response_end` WASM hook — the terminal counts
            // (reconciled) handed to any module that wants them (custom billing
            // emit, audit). Observability, not enforcement; never Continue.
            if let Some(modules) = &ctx.modules {
                crate::proxy_wasm::run_on_response_end(
                    modules,
                    &report,
                    &binding.prefix,
                    ctx.snapshot.version,
                );
            }

            // GB-5: reconcile each capped spender's live estimate for THIS
            // stream to the provider's authoritative terminal frame (docs/01
            // Q3) and log its post-reconcile state — the billing number.
            self.budgets.settle_and_log(
                &ctx.caps,
                ctx.meter.estimated_output_tokens(),
                report.authoritative_output_tokens,
                &binding.prefix,
                ctx.snapshot.version,
                ctx.cut,
                now_unix(),
            );
        }
        Ok(None)
    }
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

    /// The effective policy is what the proxy consults; the raw config is
    /// what the operator wrote. The snapshot carries both, composed once.
    #[test]
    fn bound_snapshot_carries_composed_policies() {
        let path = temp_cfg(&valid_yaml("prod"));
        let reloader = Reloader::bootstrap(path).unwrap();
        let gateway = Gateway::new(reloader.shared());
        let ctx = gateway.new_ctx();
        let policy = ctx.snapshot.config.routes[0].policy();
        assert_eq!(policy.pinned["env"], "prod");
    }
}
