//! Request-scoped helper functions for the proxy: attribution-header
//! collection, the CEL context builder, the GB-4 rejection writer, JWT claim
//! verification, policy resolution, and GB-7 session-tag resolution. Split out
//! of `proxy.rs` to keep that file focused on the `ProxyHttp` trait impl and
//! under the size budget; the behavior is unchanged.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use log::info;
use pingora::http::{RequestHeader, ResponseHeader};
use pingora::prelude::*;

use gateway_core::attribution::{self, Origin, Tag};
use gateway_core::budget::CapId;
use gateway_core::config::{Config, RejectionTemplate, StreamingRejection, StsConfig, ATTR_HEADER_PREFIX};
use gateway_core::expr::EvalCtx;
use gateway_core::jwt;
use gateway_core::scope::{validate_session_tag_value, EffectivePolicy};
use gateway_core::template;

/// Caller-sent attribution headers (`x-attr-<key>`), first value wins.
pub(crate) fn caller_attrs(head: &RequestHeader) -> BTreeMap<String, String> {
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
pub(crate) fn eval_ctx(
    head: &RequestHeader,
    path: &str,
    claims: Option<&serde_json::Map<String, serde_json::Value>>,
) -> EvalCtx {
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

pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// GB-4: the operator's body verbatim — correct status and content type,
/// never a bare 4xx of our own invention.
pub(crate) async fn respond_rejection(
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
    session
        .write_response_body(Some(Bytes::from(body)), true)
        .await?;
    Ok(())
}

/// GB-2: claims from a verified HS256 token, or `None` (absent header,
/// bad signature, expired — each logged). Consulted whenever auth is
/// configured: claim mappings, CEL derivations, and label expressions all
/// read from the SAME verified source. Takes the request's pinned config,
/// never the live cell.
pub(crate) fn verified_claims(
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
pub(crate) fn resolve_policy(
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

/// Resolve a request's attribution in two phases (extracted from the request
/// hook): phase 1 is the composed fleet→project→route policy, deliberately
/// lenient; phase 2 re-resolves under route⊕app when the RESOLVED value of
/// `apps.key` selects an override (which may add/satisfy requirements, override
/// pins/templates/labels). Returns the FINAL policy the hook enforces against
/// and its resolution. `route` outlives the borrow.
pub(crate) fn resolve_with_app_scope<'r>(
    route: &'r gateway_core::config::Route,
    apps: Option<&gateway_core::config::Apps>,
    caller: &BTreeMap<String, String>,
    claims: Option<&serde_json::Map<String, serde_json::Value>>,
    cel_ctx: &EvalCtx,
) -> (&'r EffectivePolicy, attribution::Resolution) {
    let mut policy = route.policy();
    let mut resolution = resolve_policy(policy, caller, claims, cel_ctx, &route.prefix);
    if let Some(apps) = apps {
        let app_value = resolution.value(&apps.key).map(str::to_string);
        if let Some(app_policy) = app_value.as_deref().and_then(|value| route.app_policy(value)) {
            let value = app_value.expect("checked above");
            info!("[app {}] {}={value} -> app override", route.prefix, apps.key);
            policy = app_policy;
            resolution = resolve_policy(policy, caller, claims, cel_ctx, &route.prefix);
        }
    }
    (policy, resolution)
}

/// GB-7: session tags from RESOLVED attribution values. Static config
/// values pass through; `from_attribution` reads the adjudicated tag —
/// and re-checks (defense in depth; config validation already guarantees
/// it) that the value is not caller-origin.
pub(crate) fn resolve_session_tags(
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
                validate_session_tag_value(&tag.value).map_err(|e| (key.clone(), e))?;
                tag.value.clone()
            }
            (None, None) => unreachable!("config validation enforces exactly-one-of"),
        };
        out.push((spec.key.clone(), value));
    }
    Ok(out)
}

/// GB-8: resolve the effective Vertex billing labels for a request — build the
/// attribution map from resolved tags, extend the CEL context with it, and
/// resolve every label (static / from_attribution / CEL). Extracted from the
/// proxy hook so it stays focused; the hook owns the async GB-4 rejection on
/// the `Err` path. Returns the resolved `(key, value)` pairs or the offending
/// label's [`gateway_core::labels::LabelError`].
pub(crate) fn resolve_vertex_labels(
    labels: &[gateway_core::scope::EffectiveLabel],
    tags: &[Tag],
    cel_ctx: &EvalCtx,
) -> std::result::Result<Vec<(String, String)>, gateway_core::labels::LabelError> {
    let attribution: BTreeMap<String, String> = tags
        .iter()
        .map(|t| (t.key.clone(), t.value.clone()))
        .collect();
    let mut label_ctx = cel_ctx.clone();
    label_ctx.attribution = attribution.clone();
    gateway_core::labels::resolve(labels, &attribution, &label_ctx)
}

/// Dot-segment defense (GB-1/GB-2): resolve `.`/`..` (literal and %2e-encoded)
/// and merge duplicate slashes, then REWRITE the request URI so the upstream
/// sees the same resolved path. Returns the normalized path on success, or the
/// unrepresentable target string on the (statically unreachable) failure the
/// hook rejects with the GB-4 `unknown_route` template. Extracted from the
/// request hook so it stays focused; a traversal spelling can never select a
/// weaker attribution contract because gateway and upstream agree on the path.
pub(crate) fn normalize_and_rewrite(
    session: &mut Session,
    raw_path: &str,
    cfg_version: u64,
) -> std::result::Result<String, ()> {
    let path = gateway_core::config::normalize_path(raw_path);
    if path == raw_path {
        return Ok(path);
    }
    let query = session.req_header().uri.query().map(str::to_owned);
    let target = match &query {
        Some(q) => format!("{path}?{q}"),
        None => path.clone(),
    };
    match http::Uri::try_from(target.as_str()) {
        Ok(uri) => {
            info!("[req] path normalized: {raw_path} -> {path} cfg=v{cfg_version}");
            session.req_header_mut().set_uri(uri);
            Ok(path)
        }
        Err(_) => Err(()),
    }
}

/// A GB-7 credential-resolution failure, distinguishing the two fail-closed
/// paths: an attribution/session-tag problem is a GB-4 rejection (the operator
/// template), a credential EXCHANGE failure is a 502 (an infrastructure fault,
/// loudly logged — not the operator's body).
pub(crate) enum Gb7Failure {
    /// A session tag could not be resolved: reject with the GB-4 template,
    /// naming `key`.
    Reject { key: String, reason: String },
    /// The AssumeRole exchange itself failed: a 502.
    Exchange(String),
}

/// GB-7: resolve session tags from ATTRIBUTION values and mint (or cache-hit)
/// the session-tagged credentials. Extracted from the proxy hook so it stays
/// focused; the hook maps [`Gb7Failure`] onto its rejection/502 responses. On
/// success returns `(creds, region, session_tags, cache_hit)` for the caller to
/// log and carry to `upstream_request_filter`.
pub(crate) async fn resolve_gb7_credentials(
    cache: &gateway_core::aws::CredentialCache,
    sts: &StsConfig,
    tags: &[Tag],
    now: u64,
) -> std::result::Result<(gateway_core::aws::Credentials, String, Vec<(String, String)>, bool), Gb7Failure>
{
    let session_tags = resolve_session_tags(sts, tags)
        .map_err(|(key, reason)| Gb7Failure::Reject { key, reason })?;
    let (creds, cached) = crate::aws_auth::credentials_for(cache, sts, &session_tags, now)
        .await
        .map_err(|e| Gb7Failure::Exchange(e.to_string()))?;
    Ok((creds, sts.region.clone(), session_tags, cached))
}

/// Render an `Option<u64>` token count for a log line (`-` when absent).
pub(crate) fn opt(n: Option<u64>) -> String {
    n.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string())
}

/// GB-5 mid-stream cut: render the operator's GB-4 streaming terminal event
/// (the typed [`StreamingRejection`] from the request's bound policy) into the
/// bytes that replace the outgoing chunk. `streaming` is the operator's block,
/// or `None` — in which case a minimal bare-data frame is used, so the cut
/// still fires and the stream still stops, only the payload is a default.
///
/// [`StreamingRejection`]: gateway_core::config::StreamingRejection
pub(crate) fn render_cut_event(
    streaming: Option<&StreamingRejection>,
    id: &CapId,
    cap: u64,
    spent: u64,
    route: &str,
) -> Bytes {
    let cap_s = cap.to_string();
    let spent_s = spent.to_string();
    let key_s = id.to_string();
    let vars: [(&str, &str); 4] = [
        ("key", key_s.as_str()),
        ("route", route),
        ("cap", cap_s.as_str()),
        ("spend", spent_s.as_str()),
    ];
    let rendered = match streaming {
        Some(streaming) => template::render_terminal_event(streaming, &vars),
        None => format!(
            "data: {{\"error\":\"budget exhausted for {key_s}\",\"cap\":{cap},\"spend\":{spent}}}\n\n"
        ),
    };
    Bytes::from(rendered)
}
