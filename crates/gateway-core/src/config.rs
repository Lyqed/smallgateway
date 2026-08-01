//! The static config file (Phase 1: Baseline-conformant from a file).
//!
//! Serde YAML types for providers, routes, attribution rules (GB-1/2/3),
//! and operator-defined rejection templates (GB-4), plus the startup
//! validation that makes a bad file fail fast with precise errors —
//! unknown provider refs, empty required keys, placeholder typos — instead
//! of failing at request time.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use serde::Deserialize;

use crate::adapters::{
    anthropic::AnthropicAdapter, bedrock::BedrockAdapter, openai::OpenAiAdapter, Adapter,
};
use crate::template;

/// Attribution keys travel as `x-attr-<key>` request headers.
pub const ATTR_HEADER_PREFIX: &str = "x-attr-";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// name → provider; routes reference providers by name.
    pub providers: BTreeMap<String, Provider>,
    pub routes: Vec<Route>,
    /// GB-4: the operator owns every rejection body, verbatim.
    pub rejections: Rejections,
    /// GB-2 (optional): JWT verification for claim-mapped attribution.
    #[serde(default)]
    pub auth: Option<Auth>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub kind: ProviderKind,
    pub upstream: Upstream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    OpenAi,
    Anthropic,
    Bedrock,
}

impl ProviderKind {
    pub fn name(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "openai",
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Bedrock => "bedrock",
        }
    }

    /// Fresh adapter for one response stream. `Send + Sync` because pingora
    /// requires `ProxyHttp::CTX: Send + Sync`; the adapters are plain-data
    /// push parsers so both auto-derive. (Promoted from
    /// `spikes/proxy-pingora/src/provider.rs`.)
    pub fn new_adapter(self) -> Box<dyn Adapter + Send + Sync> {
        match self {
            ProviderKind::OpenAi => Box::new(OpenAiAdapter::new()),
            ProviderKind::Anthropic => Box::new(AnthropicAdapter::new()),
            ProviderKind::Bedrock => Box::new(BedrockAdapter::new()),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Upstream {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub tls: bool,
    /// SNI for TLS upstreams; defaults to `host`.
    #[serde(default)]
    pub sni: Option<String>,
}

impl Upstream {
    pub fn sni(&self) -> &str {
        self.sni.as_deref().unwrap_or(&self.host)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    /// Path prefix, matched on segment boundaries: `/openai` matches
    /// `/openai` and `/openai/v1/...`, never `/openaix`. Longest prefix wins.
    pub prefix: String,
    /// Name of a provider in [`Config::providers`].
    pub provider: String,
    #[serde(default)]
    pub attribution: Attribution,
}

/// The route's attribution contract. Every tag on a forwarded request has an
/// origin — assigned (pinned), proven (JWT claim), or caller — resolved by
/// [`crate::attribution::resolve`].
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attribution {
    /// GB-1: keys that must be present (from any origin) or the request is
    /// rejected with the operator's `missing_attribution` template.
    #[serde(default)]
    pub required_keys: Vec<String>,
    /// GB-3: key → value assigned by the gateway. A caller-sent value for a
    /// pinned key is overwritten, never believed.
    #[serde(default)]
    pub pinned: BTreeMap<String, String>,
    /// GB-2: key → JWT claim name. The value comes only from a verified
    /// token; a caller header for a claim-mapped key is never believed.
    #[serde(default)]
    pub from_claims: BTreeMap<String, String>,
}

/// GB-4: one operator-defined template per rejection reason. Both reasons
/// are mandatory — a gateway that invents its own 4xx body is exactly what
/// the Baseline forbids.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rejections {
    /// Placeholders: `{{key}}` (the missing keys), `{{route}}` (the prefix).
    pub missing_attribution: RejectionTemplate,
    /// Placeholders: `{{route}}` (the unmatched request path).
    pub unknown_route: RejectionTemplate,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RejectionTemplate {
    pub status: u16,
    pub content_type: String,
    pub body: String,
    /// GB-4's streaming half: the terminal event emitted when an in-flight
    /// stream must be cut (budget exhausted mid-generation). The type and
    /// validation land now; the mid-stream cut itself wires in a later
    /// phase — it needs the pingora-proxy "finish downstream cleanly"
    /// change recorded in the spike README.
    #[serde(default)]
    pub streaming: Option<StreamingRejection>,
}

/// Shape of the operator's terminal event for a cut stream, rendered into
/// the response's native framing (an SSE `event:`/`data:` block for SSE
/// providers, a single event-stream frame for Bedrock).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamingRejection {
    /// Event name (`event:` line / `:event-type` header). `None` → a bare
    /// data frame.
    #[serde(default)]
    pub event: Option<String>,
    /// Payload template; same placeholders as the sibling `body`.
    pub data: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Auth {
    pub jwt: JwtAuth,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JwtAuth {
    /// Shared secret for HS256 verification. Demo/dev shape; asymmetric
    /// algs are a later phase.
    pub hs256_secret: String,
    /// Request header carrying `Bearer <token>`.
    #[serde(default = "default_jwt_header")]
    pub header: String,
}

fn default_jwt_header() -> String {
    "authorization".to_string()
}

#[derive(Debug)]
pub enum ConfigError {
    Io(String),
    Parse(String),
    /// Every validation failure, collected — an operator fixes the file
    /// once, not error-by-error.
    Invalid(Vec<String>),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "cannot read config: {e}"),
            ConfigError::Parse(e) => write!(f, "cannot parse config: {e}"),
            ConfigError::Invalid(errs) => {
                writeln!(f, "invalid config:")?;
                for e in errs {
                    writeln!(f, "  - {e}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::Io(format!("{}: {e}", path.display())))?;
        Self::from_yaml(&text)
    }

    pub fn from_yaml(text: &str) -> Result<Config, ConfigError> {
        let cfg: Config =
            serde_yaml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        cfg.validate().map_err(ConfigError::Invalid)?;
        Ok(cfg)
    }

    /// Longest-prefix route match on segment boundaries.
    ///
    /// Callers must pass a path already run through [`normalize_path`]:
    /// matching a raw path lets `/openai/../claims/...` select the `/openai`
    /// contract while the upstream serves `/claims/...` — the governance
    /// bypass the normalization exists to close. The proxy normalizes once
    /// in `request_filter` and forwards the same resolved path upstream.
    pub fn match_route(&self, path: &str) -> Option<&Route> {
        self.routes
            .iter()
            .filter(|r| prefix_matches(&r.prefix, path))
            .max_by_key(|r| r.prefix.trim_end_matches('/').len())
    }

    fn validate(&self) -> Result<(), Vec<String>> {
        let mut errs = Vec::new();

        if self.providers.is_empty() {
            errs.push("providers: at least one provider is required".to_string());
        }
        for (name, p) in &self.providers {
            if name.trim().is_empty() {
                errs.push("providers: provider name must not be empty".to_string());
            }
            if p.upstream.host.trim().is_empty() {
                errs.push(format!("provider {name:?}: upstream.host must not be empty"));
            }
            if p.upstream.port == 0 {
                errs.push(format!("provider {name:?}: upstream.port must be 1-65535"));
            }
        }

        if self.routes.is_empty() {
            errs.push("routes: at least one route is required".to_string());
        }
        let mut seen_prefixes = BTreeSet::new();
        let has_jwt = self.auth.is_some();
        for route in &self.routes {
            let label = format!("route {:?}", route.prefix);
            if !route.prefix.starts_with('/') {
                errs.push(format!("{label}: prefix must start with '/'"));
            }
            if !seen_prefixes.insert(route.prefix.trim_end_matches('/').to_string()) {
                errs.push(format!("{label}: duplicate prefix"));
            }
            if !self.providers.contains_key(&route.provider) {
                let known: Vec<&str> = self.providers.keys().map(String::as_str).collect();
                errs.push(format!(
                    "{label}: unknown provider {:?} (defined providers: {})",
                    route.provider,
                    known.join(", ")
                ));
            }
            validate_attribution(&route.attribution, &label, has_jwt, &mut errs);
        }

        if let Some(auth) = &self.auth {
            if auth.jwt.hs256_secret.is_empty() {
                errs.push("auth.jwt.hs256_secret must not be empty".to_string());
            }
            if auth.jwt.header.trim().is_empty() {
                errs.push("auth.jwt.header must not be empty".to_string());
            }
        }

        // GB-4 templates: {{key}} only makes sense where a key is missing.
        validate_template(
            &self.rejections.missing_attribution,
            "missing_attribution",
            &["key", "route"],
            &mut errs,
        );
        validate_template(&self.rejections.unknown_route, "unknown_route", &["route"], &mut errs);

        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs)
        }
    }
}

/// Dot-segment and duplicate-slash resolution, applied to the request path
/// BEFORE route matching and upstream forwarding (RFC 3986 §5.2.4
/// `remove_dot_segments`, plus nginx-style slash merging).
///
/// Without it, `/openai/../claims/v1/chat` longest-prefix-matches the
/// `/openai` route — a WEAKER attribution contract — while an upstream that
/// collapses dot-segments (most HTTP servers do) serves `/claims/...`: the
/// caller picks its own contract and smuggles forged `x-attr-*` tags past
/// GB-1/GB-2. The gateway therefore resolves the path exactly as a
/// well-behaved upstream would, matches routes against the resolved path,
/// and forwards that same path, so gateway and upstream can never disagree
/// about which resource the contract was enforced for.
///
/// `%2e`/`%2E`-encoded dots count as dots when detecting a dot-segment
/// (closing the percent-encoded spelling of the same bypass); all other
/// bytes are forwarded verbatim — nothing else is percent-decoded, so
/// legitimately encoded segment data (e.g. Bedrock model ARNs) is
/// untouched. Non-origin-form targets (`*`, absolute-form) pass through
/// unchanged; they match no `/`-anchored route.
pub fn normalize_path(path: &str) -> String {
    if !path.starts_with('/') {
        return path.to_string();
    }
    let mut segments: Vec<&str> = Vec::new();
    // Whether the resolved path denotes a "directory" (keeps a trailing
    // '/'): true after `.` or `..`, per the RFC algorithm's `/` output.
    let mut trailing = false;
    for seg in path.split('/') {
        if seg.is_empty() {
            continue; // the leading slash, plus `//` merging
        }
        match dot_segment(seg) {
            Some(DotSegment::Current) => trailing = true,
            Some(DotSegment::Parent) => {
                segments.pop(); // popping past the root is a no-op, not an error
                trailing = true;
            }
            None => {
                segments.push(seg);
                trailing = false;
            }
        }
    }
    if segments.is_empty() {
        return "/".to_string();
    }
    let mut out = String::with_capacity(path.len());
    for seg in &segments {
        out.push('/');
        out.push_str(seg);
    }
    if trailing || path.ends_with('/') {
        out.push('/');
    }
    out
}

enum DotSegment {
    Current,
    Parent,
}

/// Is `seg` a `.` or `..` segment, counting `%2e`/`%2E` as a dot? `...` and
/// segments with any non-dot byte are ordinary data and pass verbatim.
fn dot_segment(seg: &str) -> Option<DotSegment> {
    let mut dots = 0usize;
    let mut rest = seg;
    while !rest.is_empty() {
        rest = rest
            .strip_prefix('.')
            .or_else(|| rest.strip_prefix("%2e"))
            .or_else(|| rest.strip_prefix("%2E"))?;
        dots += 1;
        if dots > 2 {
            return None;
        }
    }
    match dots {
        1 => Some(DotSegment::Current),
        2 => Some(DotSegment::Parent),
        _ => None, // 0: empty segments never reach here
    }
}

/// Segment-boundary prefix match: `/openai` matches `/openai` and
/// `/openai/v1`, never `/openaix`. A `/` prefix matches everything.
fn prefix_matches(prefix: &str, path: &str) -> bool {
    let p = prefix.trim_end_matches('/');
    if p.is_empty() {
        return true; // prefix was "/" (or "//"): the catch-all route
    }
    path == p || path.strip_prefix(p).is_some_and(|rest| rest.starts_with('/'))
}

fn validate_attribution(attr: &Attribution, label: &str, has_jwt: bool, errs: &mut Vec<String>) {
    let mut seen_required = BTreeSet::new();
    for key in &attr.required_keys {
        check_key(key, &format!("{label}: required_keys"), errs);
        if !seen_required.insert(key.as_str()) {
            errs.push(format!("{label}: required_keys: duplicate key {key:?}"));
        }
    }
    for (key, value) in &attr.pinned {
        check_key(key, &format!("{label}: pinned"), errs);
        if value.is_empty() {
            errs.push(format!("{label}: pinned {key:?}: value must not be empty"));
        }
    }
    for (key, claim) in &attr.from_claims {
        check_key(key, &format!("{label}: from_claims"), errs);
        if claim.trim().is_empty() {
            errs.push(format!("{label}: from_claims {key:?}: claim name must not be empty"));
        }
        if attr.pinned.contains_key(key) {
            errs.push(format!(
                "{label}: key {key:?} is both pinned and claim-mapped; pick one origin"
            ));
        }
    }
    if !attr.from_claims.is_empty() && !has_jwt {
        errs.push(format!(
            "{label}: from_claims requires auth.jwt to be configured"
        ));
    }
}

/// Keys become `x-attr-<key>` header names, so the charset is the safe
/// header subset — reject at load rather than panic at request time.
fn check_key(key: &str, ctx: &str, errs: &mut Vec<String>) {
    if key.is_empty() {
        errs.push(format!("{ctx}: empty attribution key"));
        return;
    }
    let ok = key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !ok {
        errs.push(format!(
            "{ctx}: attribution key {key:?} must be lowercase [a-z0-9_-] \
             (it becomes the {ATTR_HEADER_PREFIX}{key} header)"
        ));
    }
}

fn validate_template(t: &RejectionTemplate, reason: &str, allowed: &[&str], errs: &mut Vec<String>) {
    if !(100..=599).contains(&t.status) {
        errs.push(format!("rejections.{reason}: status {} is not a valid HTTP status", t.status));
    }
    if t.content_type.trim().is_empty() {
        errs.push(format!("rejections.{reason}: content_type must not be empty"));
    }
    check_placeholders(&t.body, &format!("rejections.{reason}.body"), allowed, errs);
    if let Some(streaming) = &t.streaming {
        check_placeholders(
            &streaming.data,
            &format!("rejections.{reason}.streaming.data"),
            allowed,
            errs,
        );
    }
}

fn check_placeholders(text: &str, ctx: &str, allowed: &[&str], errs: &mut Vec<String>) {
    match template::placeholders(text) {
        Ok(names) => {
            for name in names {
                if !allowed.contains(&name.as_str()) {
                    errs.push(format!(
                        "{ctx}: unknown placeholder {{{{{name}}}}} (allowed: {})",
                        allowed
                            .iter()
                            .map(|a| format!("{{{{{a}}}}}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
        }
        Err(e) => errs.push(format!("{ctx}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal valid config the failure tests mutate.
    fn valid_yaml() -> String {
        r#"
providers:
  openai-main:
    kind: openai
    upstream: { host: 127.0.0.1, port: 6190 }
routes:
  - prefix: /openai
    provider: openai-main
    attribution:
      required_keys: [team]
      pinned: { env: prod }
rejections:
  missing_attribution:
    status: 428
    content_type: application/json
    body: '{"error":"missing {{key}} on {{route}}"}'
    streaming:
      event: error
      data: '{"error":"missing {{key}}"}'
  unknown_route:
    status: 404
    content_type: application/json
    body: '{"error":"no route for {{route}}"}'
"#
        .to_string()
    }

    fn errors_of(yaml: &str) -> Vec<String> {
        match Config::from_yaml(yaml) {
            Err(ConfigError::Invalid(errs)) => errs,
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn valid_config_parses() {
        let cfg = Config::from_yaml(&valid_yaml()).unwrap();
        assert_eq!(cfg.providers["openai-main"].kind, ProviderKind::OpenAi);
        assert_eq!(cfg.routes[0].attribution.pinned["env"], "prod");
        let streaming = cfg.rejections.missing_attribution.streaming.as_ref().unwrap();
        assert_eq!(streaming.event.as_deref(), Some("error"));
    }

    #[test]
    fn unknown_provider_ref_names_the_route_and_provider() {
        let yaml = valid_yaml().replace("provider: openai-main", "provider: nope");
        let errs = errors_of(&yaml);
        assert!(errs.iter().any(|e| e.contains("unknown provider \"nope\"")), "{errs:?}");
    }

    #[test]
    fn empty_required_key_is_rejected() {
        let yaml = valid_yaml().replace("required_keys: [team]", "required_keys: [team, '']");
        let errs = errors_of(&yaml);
        assert!(errs.iter().any(|e| e.contains("empty attribution key")), "{errs:?}");
    }

    #[test]
    fn placeholder_typo_is_named() {
        let yaml = valid_yaml().replace("{{key}} on {{route}}", "{{keys}} on {{route}}");
        let errs = errors_of(&yaml);
        assert!(
            errs.iter().any(|e| e.contains("unknown placeholder {{keys}}")),
            "{errs:?}"
        );
    }

    #[test]
    fn key_placeholder_is_invalid_for_unknown_route() {
        let yaml = valid_yaml().replace("no route for {{route}}", "no route for {{key}}");
        let errs = errors_of(&yaml);
        assert!(
            errs.iter()
                .any(|e| e.contains("unknown_route.body") && e.contains("{{key}}")),
            "{errs:?}"
        );
    }

    #[test]
    fn from_claims_without_auth_is_rejected() {
        let yaml = valid_yaml().replace(
            "pinned: { env: prod }",
            "pinned: { env: prod }\n      from_claims: { user: sub }",
        );
        let errs = errors_of(&yaml);
        assert!(errs.iter().any(|e| e.contains("requires auth.jwt")), "{errs:?}");
    }

    #[test]
    fn pinned_and_claim_mapped_key_conflict_is_rejected() {
        let yaml = valid_yaml().replace(
            "pinned: { env: prod }",
            "pinned: { env: prod }\n      from_claims: { env: environment }",
        );
        let yaml = format!("{yaml}auth:\n  jwt:\n    hs256_secret: s\n");
        let errs = errors_of(&yaml);
        assert!(
            errs.iter().any(|e| e.contains("both pinned and claim-mapped")),
            "{errs:?}"
        );
    }

    #[test]
    fn uppercase_key_is_rejected_with_header_hint() {
        let yaml = valid_yaml().replace("required_keys: [team]", "required_keys: [Team]");
        let errs = errors_of(&yaml);
        assert!(errs.iter().any(|e| e.contains("x-attr-Team")), "{errs:?}");
    }

    #[test]
    fn duplicate_prefixes_collide_even_with_trailing_slash() {
        let yaml = valid_yaml().replace(
            "routes:",
            "routes:\n  - prefix: /openai/\n    provider: openai-main",
        );
        let errs = errors_of(&yaml);
        assert!(errs.iter().any(|e| e.contains("duplicate prefix")), "{errs:?}");
    }

    #[test]
    fn unknown_yaml_field_fails_parse() {
        let yaml = valid_yaml().replace("kind: openai", "kind: openai\n    typo_field: 1");
        assert!(matches!(Config::from_yaml(&yaml), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn normalize_path_resolves_dot_segments() {
        // The exact live-probe bypass: must resolve to the /claims resource.
        assert_eq!(normalize_path("/openai/../claims/v1/chat"), "/claims/v1/chat");
        assert_eq!(normalize_path("/a/./b"), "/a/b");
        assert_eq!(normalize_path("/a/b/.."), "/a/");
        assert_eq!(normalize_path("/a/b/."), "/a/b/");
        assert_eq!(normalize_path("/a/.."), "/");
        // Popping past the root is a no-op, never a panic or an escape.
        assert_eq!(normalize_path("/../../etc/passwd"), "/etc/passwd");
        assert_eq!(normalize_path("/.."), "/");
        assert_eq!(normalize_path("/."), "/");
    }

    #[test]
    fn normalize_path_treats_percent_encoded_dots_as_dots() {
        assert_eq!(normalize_path("/openai/%2e%2e/claims/v1"), "/claims/v1");
        assert_eq!(normalize_path("/openai/%2E%2e/claims"), "/claims");
        assert_eq!(normalize_path("/openai/.%2e/claims"), "/claims");
        assert_eq!(normalize_path("/a/%2e/b"), "/a/b");
        // A dot INSIDE a data segment is data, not a dot-segment.
        assert_eq!(normalize_path("/a/b%2ec/d"), "/a/b%2ec/d");
        assert_eq!(normalize_path("/models/gpt-4.1"), "/models/gpt-4.1");
    }

    #[test]
    fn normalize_path_merges_slashes_and_keeps_plain_paths_verbatim() {
        assert_eq!(normalize_path("/openai//v1///chat"), "/openai/v1/chat");
        assert_eq!(normalize_path("//claims/v1"), "/claims/v1");
        assert_eq!(normalize_path("/openai/v1/chat"), "/openai/v1/chat");
        assert_eq!(normalize_path("/openai/"), "/openai/");
        assert_eq!(normalize_path("/"), "/");
        // "..." is ordinary data per RFC 3986; other percent-encodings
        // (e.g. Bedrock model ARNs) are never decoded or altered.
        assert_eq!(normalize_path("/a/.../b"), "/a/.../b");
        assert_eq!(normalize_path("/model/arn%3Aaws%2Fthing/invoke"), "/model/arn%3Aaws%2Fthing/invoke");
        // Non-origin-form targets are left for routing to reject.
        assert_eq!(normalize_path("*"), "*");
    }

    #[test]
    fn dot_segment_path_cannot_select_a_weaker_route_contract() {
        // Two routes, /openai weaker than /claims: after normalization the
        // traversal spelling lands on /claims — the stronger contract.
        let yaml = valid_yaml().replace(
            "routes:",
            "routes:\n  - prefix: /claims\n    provider: openai-main",
        );
        let cfg = Config::from_yaml(&yaml).unwrap();
        let path = normalize_path("/openai/../claims/v1/chat");
        assert_eq!(cfg.match_route(&path).unwrap().prefix, "/claims");
        let path = normalize_path("/openai/%2e%2e/claims/v1/chat");
        assert_eq!(cfg.match_route(&path).unwrap().prefix, "/claims");
    }

    #[test]
    fn route_matching_is_longest_prefix_on_segment_boundaries() {
        let yaml = valid_yaml().replace(
            "routes:",
            "routes:\n  - prefix: /openai/v1/special\n    provider: openai-main",
        );
        let cfg = Config::from_yaml(&yaml).unwrap();
        assert_eq!(cfg.match_route("/openai/v1/chat").unwrap().prefix, "/openai");
        assert_eq!(
            cfg.match_route("/openai/v1/special/x").unwrap().prefix,
            "/openai/v1/special"
        );
        assert_eq!(cfg.match_route("/openai").unwrap().prefix, "/openai");
        assert!(cfg.match_route("/openaix/v1").is_none());
        assert!(cfg.match_route("/other").is_none());
    }
}
