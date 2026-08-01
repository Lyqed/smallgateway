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
