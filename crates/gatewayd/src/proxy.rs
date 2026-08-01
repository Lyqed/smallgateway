//! The config-driven Pingora proxy: route resolution (path prefix + CEL
//! conditions), the scoped attribution contract (GB-1 required keys, GB-2
//! proven claims, GB-3 assigned pins, CEL-derived values, app-scope
//! overrides), operator-defined rejections (GB-4), Vertex billing-label
//! injection (GB-8), STS session-tag credentials + SigV4 signing for
//! Bedrock (GB-7), and the streaming tap — every response chunk flows
//! through the provider's adapter and the meter while the identical bytes
//! stream on to the client, nothing buffered whole.
//!
//! Milestone 2: every request binds one `Arc<Snapshot>` at request start
//! (`new_ctx`) and consults ONLY that snapshot for its whole lifetime,
//! streaming included — no torn reads, and an old version drains out with
//! its last in-flight stream (docs/03-hot-swap.md). Every `[req]` and
//! `[meter]` line carries `cfg=vN`.
//!
//! Milestone 3 request flow (request_filter):
//! 1. normalize the path, build the CEL context (request meta + verified
//!    claims), select the route (prefix + condition);
//! 2. resolve attribution against the route's composed fleet→project→route
//!    policy (derived CEL values evaluated here; failures leave the key
//!    unresolved — required keys then reject, fail closed);
//! 3. if the resolved value of `apps.key` selects an app override,
//!    re-resolve under the composed route⊕app policy (its templates and
//!    labels included);
//! 4. GB-8 (vertex): resolve the effective labels (static /
//!    from_attribution / CEL); any failure → the effective GB-4
//!    `missing_attribution` rejection. The merge into the request BODY
//!    happens in `request_body_filter` (the body is buffered for labeled
//!    vertex routes only — a malformed JSON body there is refused with a
//!    plain 400 before any spend can occur);
//! 5. GB-7 (bedrock + sts): resolve session tags from ATTRIBUTION values
//!    (never caller-raw — enforced statically at config load and
//!    re-checked here), get credentials (per-tag-set cache, else one
//!    AssumeRole exchange), then SigV4-sign the upstream request in
//!    `upstream_request_filter`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bytes::Bytes;
use log::{error, info};
use pingora::http::{RequestHeader, ResponseHeader};
use pingora::prelude::*;

use gateway_core::adapters::Adapter;
use gateway_core::attribution::{self, Origin, Tag};
use gateway_core::aws::{CredentialCache, Credentials};
use gateway_core::budget::{CapId, Verdict};
use gateway_core::config::{self, Config, ProviderKind, RejectionTemplate, StsConfig, ATTR_HEADER_PREFIX};
use gateway_core::event::Event;
use gateway_core::expr::EvalCtx;
use gateway_core::jwt;
use gateway_core::labels;
use gateway_core::metering::Meter;
use gateway_core::scope::{validate_session_tag_value, EffectivePolicy};
use gateway_core::snapshot::Snapshot;
use gateway_core::template;

use crate::aws_auth;
use crate::budget::{MeterOutcome, NodeBudgets};
use crate::reload::SharedSnapshot;

pub struct Gateway {
    /// The swap cell. Touched exactly once per request, in `new_ctx`;
    /// every later hook reads the request's own pinned snapshot instead.
    shared: SharedSnapshot,
    /// GB-7: credentials per unique (role, endpoint, tag-set). Lives
    /// across config swaps on purpose — the key carries every input that
    /// changes the minted credentials.
    sts_cache: CredentialCache,
    /// GB-5: the node-local budget counters (shared with the control-plane
    /// client, which reports spend and applies share grants). Lives OUTSIDE
    /// the snapshot on purpose — a config swap changes the CAP a request reads
    /// (from its pinned policy), never the running counters (docs/03
    /// limitation 3: a cap tightened mid-stream does not retroactively apply;
    /// the counters carry across swaps).
    budgets: Arc<NodeBudgets>,
}

impl Gateway {
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
    /// `Arc` the client loop reports/rebalances through.
    pub fn with_budgets(shared: SharedSnapshot, budgets: Arc<NodeBudgets>) -> Self {
        Gateway {
            shared,
            sts_cache: CredentialCache::new(),
            budgets,
        }
    }

    /// The node's budget counters — for the control-plane client to report and
    /// rebalance, and for tests.
    pub fn budgets(&self) -> Arc<NodeBudgets> {
        self.budgets.clone()
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
    /// GB-8: request-body accumulator (labeled vertex routes only).
    body_buf: Vec<u8>,
    /// GB-7: credentials + region for SigV4 signing.
    aws: Option<AwsSigning>,
    body_bytes: usize,
    body_chunks: usize,
    event_counts: [usize; 6],
    /// GB-5: the capped spenders this request bills — one per resolved
    /// attribution tag that has a composed cap. `(CapId, cap_tokens)`.
    caps: Vec<(CapId, u64)>,
    /// GB-5: the last estimated-output-token reading fed to the budget, so each
    /// tap computes the INCREMENT since the previous chunk (the Meter's
    /// estimate is cumulative). Reconciled to the authoritative frame at end.
    last_metered_est: u64,
    /// GB-5: set once a mid-stream cut fires. Further chunks are suppressed and
    /// the operator's terminal event is emitted in place of continued content.
    cut: bool,
}

impl ReqCtx {
    fn bound(snapshot: Arc<Snapshot>) -> Self {
        ReqCtx {
            snapshot,
            route: None,
            adapter: None,
            meter: Meter::new(),
            tags: Vec::new(),
            vertex_labels: None,
            body_buf: Vec::new(),
            aws: None,
            body_bytes: 0,
            body_chunks: 0,
            event_counts: [0; 6],
            caps: Vec::new(),
            last_metered_est: 0,
            cut: false,
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

/// The documented CEL context: request meta (+ claims); `attribution` is
/// filled in later for label expressions.
fn eval_ctx(head: &RequestHeader, path: &str, claims: Option<&serde_json::Map<String, serde_json::Value>>) -> EvalCtx {
    let mut headers = BTreeMap::new();
    for (name, value) in head.headers.iter() {
        if let Ok(v) = value.to_str() {
            headers
                .entry(name.as_str().to_ascii_lowercase())
                .or_insert_with(|| v.to_string());
        }
    }
    EvalCtx {
        method: head.method.as_str().to_string(),
        path: path.to_string(),
        headers,
        claims: claims.map(|c| serde_json::Value::Object(c.clone())),
        attribution: BTreeMap::new(),
    }
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
    // The GB-5 additions {{cap}}/{{spend}} are optional and only supplied by
    // the budget rejection path. Default them to "-" for every OTHER reason
    // (missing attribution, unresolvable label, session tag) so an operator
    // body that uses them never leaks a literal placeholder on a non-budget
    // rejection.
    let mut all: Vec<(&str, &str)> = Vec::with_capacity(vars.len() + 2);
    all.extend_from_slice(vars);
    if !all.iter().any(|(k, _)| *k == "cap") {
        all.push(("cap", "-"));
    }
    if !all.iter().any(|(k, _)| *k == "spend") {
        all.push(("spend", "-"));
    }
    let body = template::render(&t.body, &all);
    let mut header = ResponseHeader::build(t.status, Some(2))?;
    header.insert_header("content-type", t.content_type.clone())?;
    header.insert_header("content-length", body.len().to_string())?;
    session.write_response_header(Box::new(header), false).await?;
    session.write_response_body(Some(Bytes::from(body)), true).await?;
    Ok(())
}

/// GB-2: claims from a verified HS256 token, or `None` (absent header,
/// bad signature, expired — each logged). Consulted whenever auth is
/// configured: claim mappings, CEL derivations, and label expressions all
/// read from the SAME verified source. Takes the request's pinned config,
/// never the live cell.
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

/// Resolve one policy's contract: derived CEL values evaluated against the
/// request context (an eval error is logged and leaves the key unresolved
/// — required keys then report missing, fail closed).
fn resolve_policy(
    policy: &EffectivePolicy,
    caller: &BTreeMap<String, String>,
    claims: Option<&serde_json::Map<String, serde_json::Value>>,
    ctx: &EvalCtx,
    prefix: &str,
) -> attribution::Resolution {
    attribution::resolve(
        policy,
        |key| caller.get(key).cloned(),
        claims,
        |key| match policy.derived.get(key) {
            None => None,
            Some(expr) => match expr.eval_string(ctx) {
                Ok(v) => Some(v),
                Err(e) => {
                    info!("[attr {prefix}] derived {key:?} failed: {e}");
                    None
                }
            },
        },
    )
}

/// GB-7: session tags from RESOLVED attribution values. Static config
/// values pass through; `from_attribution` reads the adjudicated tag —
/// and re-checks (defense in depth; config validation already guarantees
/// it) that the value is not caller-origin.
fn resolve_session_tags(
    sts: &StsConfig,
    tags: &[Tag],
) -> std::result::Result<Vec<(String, String)>, (String, String)> {
    let mut out = Vec::with_capacity(sts.tags.len());
    for spec in &sts.tags {
        let value = match (&spec.value, &spec.from_attribution) {
            (Some(v), _) => v.clone(),
            (None, Some(key)) => {
                let tag = tags.iter().find(|t| &t.key == key).ok_or_else(|| {
                    (key.clone(), format!("attribution key {key:?} did not resolve"))
                })?;
                if tag.origin == Origin::Caller {
                    // Statically unreachable; never sign caller-raw anyway.
                    return Err((key.clone(), format!("attribution key {key:?} is caller-origin")));
                }
                validate_session_tag_value(&tag.value)
                    .map_err(|e| (key.clone(), e))?;
                tag.value.clone()
            }
            (None, None) => unreachable!("config validation enforces exactly-one-of"),
        };
        out.push((spec.key.clone(), value));
    }
    Ok(out)
}

#[async_trait]
impl ProxyHttp for Gateway {
    type CTX = ReqCtx;

    fn new_ctx(&self) -> Self::CTX {
        // Atomic per-request binding: ONE load of the current snapshot at
        // request start; every later hook reads ctx.snapshot, so this
        // request can never observe two config versions.
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

        // Phase 1 of resolution: the composed fleet→project→route policy.
        // Deliberately LENIENT — enforcement waits for the final policy of
        // the chain, because the app scope (selected by a RESOLVED value)
        // may itself satisfy a still-missing requirement (e.g. pin it).
        let mut policy = route.policy();
        let mut resolution =
            resolve_policy(policy, &caller, claims.as_ref(), &cel_ctx, &route.prefix);

        // Phase 2: the app scope. The RESOLVED value of apps.key selects
        // the override; the request re-resolves under route⊕app (which may
        // add requirements, satisfy them, override pins, templates, labels).
        if let Some(apps) = &cfg.apps {
            let app_value = resolution.value(&apps.key).map(str::to_string);
            if let Some(app_policy) = app_value.as_deref().and_then(|value| route.app_policy(value)) {
                let value = app_value.expect("checked above");
                info!("[app {}] {}={value} -> app override cfg=v{v}", route.prefix, apps.key);
                policy = app_policy;
                resolution =
                    resolve_policy(policy, &caller, claims.as_ref(), &cel_ctx, &route.prefix);
            }
        }

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

        // GB-5: for each resolved tag that this policy caps, admit the request
        // against the node-local budget BEFORE it reaches the upstream. A value
        // already at its cap is rejected with the effective GB-4
        // `missing_attribution` template (the same operator body that makes the
        // hard cap livable, per GB-4), naming the exhausted spender. The common
        // path is one in-memory check per capped tag — no control-plane hop.
        let mut caps: Vec<(CapId, u64)> = Vec::new();
        for tag in &tags {
            if let Some(cap) = policy.cap_for(&tag.key, &tag.value) {
                let id = CapId::new(&tag.key, &tag.value);
                match self.budgets.admit(&id, Some(cap)) {
                    Verdict::Deny { cap } => {
                        let spent = self
                            .budgets
                            .snapshot(&id)
                            .map(|(_, _, s)| s)
                            .unwrap_or(cap);
                        info!(
                            "[gb5 {}] {id} DENIED at admission: spent {spent}/{cap} tokens \
                             (rejecting: missing_attribution) cfg=v{v}",
                            route.prefix
                        );
                        respond_rejection(
                            session,
                            &policy.missing_attribution,
                            &[
                                ("key", id.to_string().as_str()),
                                ("route", route.prefix.as_str()),
                                ("cap", cap.to_string().as_str()),
                                ("spend", spent.to_string().as_str()),
                            ],
                        )
                        .await?;
                        return Ok(true);
                    }
                    Verdict::Escalate => {
                        // Near the local-share limit: keep serving this request,
                        // but flag it so the control-plane client escalates to a
                        // synchronous check before the next spend crosses the cap.
                        info!(
                            "[gb5 {}] {id} at/above ~90% of local share; will escalate cfg=v{v}",
                            route.prefix
                        );
                        caps.push((id, cap));
                    }
                    Verdict::Allow => caps.push((id, cap)),
                }
            }
        }

        let kind = cfg.providers[&route.provider].kind;

        // GB-8: resolve the effective labels now — fail closed BEFORE the
        // upstream sees anything. The body merge happens in
        // request_body_filter.
        if kind == ProviderKind::Vertex && !policy.labels.is_empty() {
            let attribution = tags
                .iter()
                .map(|t| (t.key.clone(), t.value.clone()))
                .collect::<BTreeMap<_, _>>();
            let mut label_ctx = cel_ctx.clone();
            label_ctx.attribution = attribution.clone();
            match labels::resolve(&policy.labels, &attribution, &label_ctx) {
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

        // GB-7: session-tag credentials for bedrock providers with sts.
        if let Some(sts) = &cfg.providers[&route.provider].sts {
            let session_tags = match resolve_session_tags(sts, &tags) {
                Ok(t) => t,
                Err((key, reason)) => {
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
            };
            match aws_auth::credentials_for(&self.sts_cache, sts, &session_tags, now_unix()).await
            {
                Ok((creds, cached)) => {
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
                    ctx.aws = Some(AwsSigning { creds, region: sts.region.clone() });
                }
                Err(e) => {
                    // Fail closed: a request whose invoice-grade identity
                    // cannot be minted never reaches Bedrock. This is an
                    // exchange failure, not an attribution failure, so it
                    // is a 502, loudly logged — not a GB-4 template.
                    error!("[gb7 {}] credential exchange FAILED: {e} cfg=v{v}", route.prefix);
                    return Err(Error::explain(
                        HTTPStatus(502),
                        format!("sts credential exchange failed: {e}"),
                    ));
                }
            }
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

        // GB-8: the body will be rewritten in request_body_filter, so its
        // length changes — switch the upstream leg to chunked framing.
        if ctx.vertex_labels.is_some() {
            upstream_request.remove_header("content-length");
            upstream_request.insert_header("transfer-encoding", "chunked")?;
        }

        // GB-7: SigV4 with the session-tagged credentials.
        if let Some(aws) = &ctx.aws {
            let binding = ctx.route.as_ref().expect("route bound in request_filter");
            let up = &ctx.snapshot.config.providers[&binding.provider].upstream;
            aws_auth::sign_bedrock_request(upstream_request, up, &aws.region, &aws.creds, now_unix())?;
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
        let Some(operator_labels) = &ctx.vertex_labels else {
            return Ok(()); // not a labeled vertex route: pass through
        };
        if let Some(chunk) = body.take() {
            ctx.body_buf.extend_from_slice(&chunk);
        }
        if !end_of_stream {
            return Ok(());
        }
        match labels::merge_into_body(&ctx.body_buf, operator_labels) {
            Ok(merged) => {
                let binding = ctx.route.as_ref().expect("route bound");
                info!(
                    "[gb8 {}] merged {} operator label(s) into request body ({} -> {} bytes)",
                    binding.prefix,
                    operator_labels.len(),
                    ctx.body_buf.len(),
                    merged.len(),
                );
                ctx.body_buf.clear();
                *body = Some(Bytes::from(merged));
                Ok(())
            }
            Err(e) => {
                error!("[gb8] request body rejected: {e}");
                Err(Error::explain(
                    HTTPStatus(400),
                    format!("vertex request body must be a JSON object: {e}"),
                ))
            }
        }
    }

    /// The tap (promoted from Spike B), now the GB-5 mid-stream enforcement
    /// point too. Pingora hands each body chunk as `&mut Option<Bytes>` on its
    /// way downstream; we feed a copy of the bytes to the adapter and the meter,
    /// then charge the INCREMENT of estimated output tokens against every capped
    /// spender for this request. When a spender's running tally crosses the
    /// bound (the cap, or the held share under partition) the stream is CUT: the
    /// operator's GB-4 terminal event (the typed streaming template) replaces
    /// the outgoing content and every later chunk is suppressed. A cap tightened
    /// mid-stream does NOT retroactively apply — this meters the version the
    /// request bound (docs/03 limitation 2); the live estimate is reconciled to
    /// the provider's terminal usage frame at stream end.
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

        // GB-5 mid-stream enforcement: charge the estimated-output-token
        // INCREMENT since the last tap against every capped spender, and cut on
        // the first that crosses its bound.
        if !ctx.caps.is_empty() && !ctx.cut {
            let est = ctx.meter.estimated_output_tokens();
            let delta = est.saturating_sub(ctx.last_metered_est);
            ctx.last_metered_est = est;
            if delta > 0 {
                let route_prefix =
                    ctx.route.as_ref().map(|b| b.prefix.clone()).unwrap_or_default();
                // Clone the caps out so the &mut ctx borrow for `cut_stream`
                // below is free of the immutable caps borrow.
                let caps = ctx.caps.clone();
                for (id, cap) in &caps {
                    match self.budgets.meter(id, Some(*cap), delta) {
                        MeterOutcome::Cut { id, cap } => {
                            let spent = self
                                .budgets
                                .snapshot(&id)
                                .map(|(_, _, s)| s)
                                .unwrap_or(cap);
                            error!(
                                "[gb5 {route_prefix}] {id} EXCEEDED mid-stream: spent \
                                 {spent}/{cap} tokens; CUTTING the stream with the GB-4 \
                                 terminal event cfg=v{}",
                                ctx.snapshot.version
                            );
                            self.cut_stream(body, ctx, &id, cap, spent);
                            break;
                        }
                        MeterOutcome::Continue => {}
                    }
                }
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

            // GB-5: reconcile the live estimate for THIS stream to the
            // provider's authoritative terminal frame (docs/01 Q3), then log
            // each capped spender's post-reconcile state. The authoritative
            // output count is the billing number; the estimate was the
            // mid-stream enforcement proxy for it.
            let est = ctx.meter.estimated_output_tokens();
            if let Some(auth) = report.authoritative_output_tokens {
                for (id, cap) in &ctx.caps {
                    self.budgets.settle(id, est, auth);
                    if let Some((_, share, spent)) = self.budgets.snapshot(id) {
                        info!(
                            "[budget {}] {id} reconciled est={est}->auth={auth}; spent={spent}/{cap} \
                             tokens (share={share}){} cfg=v{}",
                            binding.prefix,
                            if ctx.cut { " [CUT]" } else { "" },
                            ctx.snapshot.version,
                        );
                    }
                }
            } else if !ctx.caps.is_empty() {
                // No terminal usage frame: the estimate stands as the charge
                // (its error bound is the published Q3 number).
                for (id, cap) in &ctx.caps {
                    if let Some((_, share, spent)) = self.budgets.snapshot(id) {
                        info!(
                            "[budget {}] {id} no usage frame; spent={spent}/{cap} tokens \
                             (share={share}, estimate stands){} cfg=v{}",
                            binding.prefix,
                            if ctx.cut { " [CUT]" } else { "" },
                            ctx.snapshot.version,
                        );
                    }
                }
            }
        }
        Ok(None)
    }
}

impl Gateway {
    /// Cut an in-flight stream: replace the current outgoing chunk with the
    /// operator's GB-4 streaming terminal event (the typed
    /// [`StreamingRejection`] from the request's bound policy), and latch
    /// `ctx.cut` so every later chunk is suppressed. Falls back to a bare data
    /// frame if the operator defined no `streaming` block — the cut still fires,
    /// the stream still stops, only the payload is a minimal default.
    ///
    /// [`StreamingRejection`]: gateway_core::config::StreamingRejection
    fn cut_stream(
        &self,
        body: &mut Option<Bytes>,
        ctx: &mut ReqCtx,
        id: &CapId,
        cap: u64,
        spent: u64,
    ) {
        ctx.cut = true;
        let cap_s = cap.to_string();
        let spent_s = spent.to_string();
        let key_s = id.to_string();
        let route = ctx
            .route
            .as_ref()
            .map(|b| b.prefix.clone())
            .unwrap_or_default();
        let vars: [(&str, &str); 4] = [
            ("key", key_s.as_str()),
            ("route", route.as_str()),
            ("cap", cap_s.as_str()),
            ("spend", spent_s.as_str()),
        ];
        let rendered = match &ctx.snapshot.config.rejections.missing_attribution.streaming {
            Some(streaming) => template::render_terminal_event(streaming, &vars),
            None => format!(
                "data: {{\"error\":\"budget exhausted for {key_s}\",\"cap\":{cap},\"spend\":{spent}}}\n\n"
            ),
        };
        *body = Some(Bytes::from(rendered));
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
