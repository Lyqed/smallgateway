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

/// The per-request role identity: the allow-list gate, then `role_arn` and
/// `session_name` templates rendered against ADJUDICATED attribution. A
/// placeholder key must be non-caller-origin unless it is the allow-listed
/// key (the closed set is what makes a caller-picked value safe: the caller
/// chooses WHICH operator-built role, never a new one). Key names are the
/// operator's own; nothing here privileges any particular key.
pub(crate) fn resolve_role_identity(
    sts: &StsConfig,
    tags: &[Tag],
) -> std::result::Result<(String, String), (String, String)> {
    if let Some(allow) = &sts.allow {
        let tag = tags.iter().find(|t| t.key == allow.key).ok_or_else(|| {
            (
                allow.key.clone(),
                format!("attribution key {:?} did not resolve", allow.key),
            )
        })?;
        if !allow.values.iter().any(|v| v == &tag.value) {
            return Err((
                allow.key.clone(),
                format!(
                    "value {:?} is not in the operator's allow-list for {:?}",
                    tag.value, allow.key
                ),
            ));
        }
    }
    let role_arn = render_role_material(&sts.role_arn, "role_arn", sts, tags)?;
    if !role_arn.starts_with("arn:") || !role_arn.contains(":role/") {
        return Err((
            "role_arn".to_string(),
            format!("rendered role ARN {role_arn:?} is not an IAM role ARN"),
        ));
    }
    let session_name = gateway_core::aws::sanitize_session_name(&render_role_material(
        &sts.session_name,
        "session_name",
        sts,
        tags,
    )?);
    Ok((role_arn, session_name))
}

/// Render one role-material template against the adjudicated tag set,
/// enforcing the caller-origin rule per placeholder (the sts allow-list is
/// the one admissible exception). Fail closed on any unresolved key.
fn render_role_material(
    spec: &gateway_core::config::OperatorValueSpec,
    what: &str,
    sts: &StsConfig,
    tags: &[Tag],
) -> std::result::Result<String, (String, String)> {
    render_operator_template(spec, what, tags, sts.allow.as_ref().map(|a| a.key.as_str()))
}

/// Shared operator-template renderer: resolve every `{{key}}` against the
/// adjudicated tags; a Caller-origin key is refused unless it is the
/// `allow_gated_key` (role material's closed-set exception; injection
/// passes `None` — guardrails are never caller-steerable). Fail closed.
fn render_operator_template(
    spec: &gateway_core::config::OperatorValueSpec,
    what: &str,
    tags: &[Tag],
    allow_gated_key: Option<&str>,
) -> std::result::Result<String, (String, String)> {
    let template_text = spec
        .as_template()
        .ok_or_else(|| (what.to_string(), "mis-specified operator value".to_string()))?;
    let keys = template::placeholders(&template_text).map_err(|e| (what.to_string(), e))?;
    let mut vars: Vec<(String, String)> = Vec::with_capacity(keys.len());
    for key in keys {
        let tag = tags.iter().find(|t| t.key == key).ok_or_else(|| {
            (key.clone(), format!("attribution key {key:?} did not resolve"))
        })?;
        let allow_gated = allow_gated_key == Some(key.as_str());
        if tag.origin == Origin::Caller && !allow_gated {
            // Statically unreachable (scope.rs refuses the config); never
            // render caller-raw operator material anyway.
            return Err((
                key.clone(),
                format!("attribution key {key:?} is caller-origin and not allow-gated"),
            ));
        }
        vars.push((key, tag.value.clone()));
    }
    let var_refs: Vec<(&str, &str)> =
        vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let rendered = template::render(&template_text, &var_refs);
    if rendered.contains("{{") {
        return Err((
            what.to_string(),
            "template left unresolved placeholders".to_string(),
        ));
    }
    Ok(rendered)
}

/// Operator-forced injection, resolved per request. Headers are lowercased
/// (they enter the SigV4 signed set on signing providers); body fields keep
/// their dotted paths. Strictly gateway-established keys — no allow-gate.
pub(crate) struct ResolvedInjection {
    pub headers: Vec<(String, String)>,
    pub body: Vec<gateway_core::labels::ResolvedBodyField>,
}

pub(crate) fn resolve_injection(
    inject: &gateway_core::config::Injection,
    tags: &[Tag],
) -> std::result::Result<ResolvedInjection, (String, String)> {
    let mut headers = Vec::with_capacity(inject.headers.len());
    for h in &inject.headers {
        let value = render_operator_template(&h.value, &h.name, tags, None)?;
        headers.push((h.name.to_ascii_lowercase(), value));
    }
    let mut body = Vec::with_capacity(inject.body.len());
    for f in &inject.body {
        body.push(gateway_core::labels::ResolvedBodyField {
            path: f.path.clone(),
            value: render_operator_template(&f.value, &f.path, tags, None)?,
            if_absent: f.if_absent,
        });
    }
    Ok(ResolvedInjection { headers, body })
}

/// GB-7: resolve the role identity and session tags from ATTRIBUTION values
/// and mint (or cache-hit) the session-tagged credentials. Extracted from the
/// proxy hook so it stays focused; the hook maps [`Gb7Failure`] onto its
/// rejection/502 responses. On success returns
/// `(creds, region, session_tags, cache_hit)` for the caller to log and carry
/// to `upstream_request_filter`.
pub(crate) async fn resolve_gb7_credentials(
    cache: &gateway_core::aws::CredentialCache,
    sts: &StsConfig,
    tags: &[Tag],
    now: u64,
) -> std::result::Result<(gateway_core::aws::Credentials, String, Vec<(String, String)>, bool), Gb7Failure>
{
    let (role_arn, session_name) =
        resolve_role_identity(sts, tags).map_err(|(key, reason)| Gb7Failure::Reject { key, reason })?;
    let session_tags = resolve_session_tags(sts, tags)
        .map_err(|(key, reason)| Gb7Failure::Reject { key, reason })?;
    info!(
        "[gb7] role identity: role={role_arn} session_name={session_name}"
    );
    let (creds, cached) = crate::aws_auth::credentials_for(
        cache,
        sts,
        &role_arn,
        &session_name,
        &session_tags,
        now,
    )
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

#[cfg(test)]
mod role_identity_tests {
    use super::*;
    use gateway_core::config::{AllowList, OperatorValueSpec, SessionTag, StsConfig, Upstream};

    fn sts(role: &str, session: &str, allow: Option<AllowList>) -> StsConfig {
        StsConfig {
            endpoint: Upstream { host: "127.0.0.1".into(), port: 6199, tls: false, sni: None },
            role_arn: OperatorValueSpec::Bare(role.to_string()),
            session_name: OperatorValueSpec::Bare(session.to_string()),
            region: "us-east-1".into(),
            duration_secs: 900,
            tags: vec![SessionTag {
                key: "cc".into(),
                value: None,
                from_attribution: Some("cost_center".into()),
            }],
            allow,
            base: None,
        }
    }

    fn tag(key: &str, value: &str, origin: Origin) -> Tag {
        Tag { key: key.into(), value: value.into(), origin }
    }

    #[test]
    fn templates_resolve_and_sanitize_with_operator_chosen_keys() {
        // No key name is privileged: cost_center/tenant/workload, all
        // gateway-established, feed role and session material.
        let sts = sts(
            "arn:aws:iam::1:role/bedrock-{{cost_center}}",
            "{{tenant}}-{{workload}}",
            None,
        );
        let tags = [
            tag("cost_center", "research", Origin::Assigned),
            tag("tenant", "acme ward", Origin::Proven),
            tag("workload", "batch", Origin::Derived),
        ];
        let (role, session) = resolve_role_identity(&sts, &tags).unwrap();
        assert_eq!(role, "arn:aws:iam::1:role/bedrock-research");
        // The space sanitizes to '-' per AWS's RoleSessionName charset.
        assert_eq!(session, "acme-ward-batch");
    }

    #[test]
    fn allow_list_rejects_values_outside_the_closed_set() {
        let sts = sts(
            "arn:aws:iam::1:role/gw",
            "gatewayd",
            Some(AllowList { key: "team".into(), values: vec!["ml".into()] }),
        );
        let tags = [tag("team", "intruder", Origin::Caller)];
        let err = resolve_role_identity(&sts, &tags).unwrap_err();
        assert_eq!(err.0, "team");
        assert!(err.1.contains("allow-list"), "{}", err.1);
    }

    #[test]
    fn allow_gated_caller_key_may_select_the_role() {
        // The APIM-parity affordance: caller-picked, operator-closed.
        let sts = sts(
            "arn:aws:iam::1:role/bedrock-{{team}}",
            "gatewayd",
            Some(AllowList { key: "team".into(), values: vec!["ml".into(), "web".into()] }),
        );
        let tags = [tag("team", "ml", Origin::Caller)];
        let (role, _) = resolve_role_identity(&sts, &tags).unwrap();
        assert_eq!(role, "arn:aws:iam::1:role/bedrock-ml");
    }

    #[test]
    fn caller_origin_key_without_allow_gate_is_refused_at_runtime() {
        // Statically unreachable (scope.rs refuses the config), but the
        // runtime defense holds on its own.
        let sts = sts("arn:aws:iam::1:role/bedrock-{{team}}", "gatewayd", None);
        let tags = [tag("team", "ml", Origin::Caller)];
        let err = resolve_role_identity(&sts, &tags).unwrap_err();
        assert!(err.1.contains("caller-origin"), "{}", err.1);
    }

    #[test]
    fn unresolved_template_key_fails_closed() {
        let sts = sts("arn:aws:iam::1:role/gw-{{ghost}}", "gatewayd", None);
        let tags = [tag("cost_center", "research", Origin::Assigned)];
        let err = resolve_role_identity(&sts, &tags).unwrap_err();
        assert_eq!(err.0, "ghost");
        assert!(err.1.contains("did not resolve"), "{}", err.1);
    }

    #[test]
    fn rendered_role_must_still_be_an_arn() {
        let sts = sts("{{cost_center}}", "gatewayd", None);
        let tags = [tag("cost_center", "research", Origin::Assigned)];
        let err = resolve_role_identity(&sts, &tags).unwrap_err();
        assert!(err.1.contains("not an IAM role ARN"), "{}", err.1);
    }

    #[test]
    fn missing_allow_key_fails_closed_before_any_credential_work() {
        let sts = sts(
            "arn:aws:iam::1:role/gw",
            "gatewayd",
            Some(AllowList { key: "team".into(), values: vec!["ml".into()] }),
        );
        let err = resolve_role_identity(&sts, &[]).unwrap_err();
        assert_eq!(err.0, "team");
        assert!(err.1.contains("did not resolve"), "{}", err.1);
    }
}
