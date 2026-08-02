//! The static config file (Phase 1: Baseline-conformant from a file).
//!
//! Serde YAML types for providers, the four policy scopes
//! (`fleet → project → route → app`, docs/02-architecture.md), attribution
//! rules (GB-1/2/3, plus CEL-derived values), operator-defined rejection
//! templates (GB-4), Vertex billing labels (GB-8), and STS session-tag
//! credentials for Bedrock (GB-7). Startup validation makes a bad file fail
//! fast with precise errors — unknown provider refs, contradictory pins,
//! CEL typos — instead of failing at request time; composition and
//! validation live in [`crate::scope`].

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use serde::Deserialize;

use crate::adapters::{
    anthropic::AnthropicAdapter, bedrock::BedrockAdapter, openai::OpenAiAdapter,
    vertex::VertexAdapter, Adapter,
};
use crate::expr::EvalCtx;
use crate::scope::CompiledRoute;

/// Attribution keys travel as `x-attr-<key>` request headers.
pub const ATTR_HEADER_PREFIX: &str = "x-attr-";

/// The explicit base marker (docs/02: "each level prepends/appends around
/// an explicit base marker"). In `required_keys` it is a plain list entry;
/// in `labels` it is a plain string entry among the label mappings. A list
/// WITHOUT the marker replaces the parent's list; a list with it splices
/// the parent in at the marker's position; an absent/empty list inherits.
pub const BASE_MARKER: &str = "<base>";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// name → provider; routes reference providers by name.
    pub providers: BTreeMap<String, Provider>,
    /// Scope 1: fleet-wide policy (a single node today; the composition
    /// model is day-one — docs/04, Phase 1 item 2).
    #[serde(default)]
    pub fleet: Option<Scope>,
    /// Scope 2: projects; routes opt in via `route.project`.
    #[serde(default)]
    pub projects: BTreeMap<String, Scope>,
    /// Scope 3: routes.
    pub routes: Vec<Route>,
    /// Scope 4: apps, keyed by the resolved value of one attribution key.
    #[serde(default)]
    pub apps: Option<Apps>,
    /// GB-4: the fleet-scope rejection templates — both reasons mandatory.
    /// Lower scopes may override per reason via their `rejections` block.
    pub rejections: Rejections,
    /// GB-2 (optional): JWT verification for claim-mapped attribution.
    #[serde(default)]
    pub auth: Option<Auth>,
    /// Tier-2 (optional, Phase 4): signed WASM policy modules. The DECLARATIVE
    /// half lives here — name, module source, signature, which hooks, and the
    /// counter schema version — so gateway-core (and the control-plane
    /// admission gate) can reason about the module set with NO wasmtime
    /// dependency. The data plane (`gatewayd`, via `gateway-wasm`) verifies the
    /// signature and compiles the bytes; this crate never links a wasm runtime,
    /// keeping the two-binary budget intact.
    #[serde(default)]
    pub wasm: WasmConfig,
}

// Tier-2 WASM config types (Phase 4) live in `wasm_config` and are re-exported
// here so `Config::wasm` and every caller keep one import surface.
pub use crate::wasm_config::{WasmConfig, WasmHook, WasmModule};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub kind: ProviderKind,
    pub upstream: Upstream,
    /// Operator-forced headers / body fields for every request through
    /// this provider (guardrails and friends). See [`Injection`].
    #[serde(default)]
    pub inject: Option<Injection>,
    /// GB-8's auth half (vertex-kind only): the gateway MINTS the Google
    /// credential itself — Workload Identity Federation token exchange, then
    /// a service-account access token — and sets `Authorization: Bearer` on
    /// the upstream. Absent: the caller's own Authorization passes through
    /// unchanged (the original behavior). See [`VertexAuth`].
    #[serde(default)]
    pub auth: Option<VertexAuth>,
    /// Vertex-kind only: the operator's ALLOWED location list. Present, the
    /// upstream host is derived per request from the `/locations/<loc>/`
    /// path segment (multi-regions `eu`/`us`/`global` use `upstream.host`
    /// as-is, a regional location prefixes it: `europe-west3-<host>`), and
    /// a request naming a location outside the list is refused with the
    /// operator's GB-4 unknown_route body. Absent: static host, no gate —
    /// the original behavior.
    #[serde(default)]
    pub locations: Option<Vec<String>>,
    /// GB-7 (bedrock kind only): exchange attribution values for STS
    /// session-tag credentials and SigV4-sign every upstream request.
    #[serde(default)]
    pub sts: Option<StsConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    OpenAi,
    Anthropic,
    Bedrock,
    Vertex,
}

impl ProviderKind {
    pub fn name(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "openai",
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Bedrock => "bedrock",
            ProviderKind::Vertex => "vertex",
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
            ProviderKind::Vertex => Box::new(VertexAdapter::new()),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
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

/// GB-7: AssumeRole-with-session-tags against an STS endpoint; the tags are
/// the invoice-grade join — operator/attribution-derived only, never
/// caller-raw (validation rejects a tag sourced from a caller-origin key).
///
/// `role_arn` and `session_name` are [`OperatorValueSpec`]s: a bare string
/// behaves exactly as before, and a string containing `{{key}}` placeholders
/// is an operator-authored TEMPLATE resolved per request against adjudicated
/// attribution (the operator writes the template; a caller can never change
/// it). Placeholder keys must be gateway-established (pinned, claim-mapped,
/// or derived) — with ONE deliberate exception: a caller-asserted key that
/// this block's `allow` list gates is admissible, because the allow-list
/// closes the value set to operator-approved members (the APIM-parity
/// affordance: the caller picks WHICH pre-built door, never a new one).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StsConfig {
    pub endpoint: Upstream,
    pub role_arn: OperatorValueSpec,
    #[serde(default = "default_session_name_spec")]
    pub session_name: OperatorValueSpec,
    #[serde(default = "default_region")]
    pub region: String,
    /// Requested credential lifetime; the cache honors the Expiration the
    /// STS response actually grants.
    #[serde(default = "default_sts_duration")]
    pub duration_secs: u32,
    pub tags: Vec<SessionTag>,
    /// Closed-set gate on ONE attribution key: a request whose resolved
    /// value of `key` is not in `values` is rejected with the GB-4 body
    /// before any credential is minted. Also the only mechanism that admits
    /// a caller-asserted key into `role_arn`/`session_name` templates.
    #[serde(default)]
    pub allow: Option<AllowList>,
    /// The optional web-identity BASE hop for the two-hop role chain: hop 1
    /// exchanges a platform-provided OIDC token for BASE credentials via
    /// `AssumeRoleWithWebIdentity` (token-authed, unsigned); hop 2 then
    /// chains into the (possibly per-request) target role with an
    /// `AssumeRole` call SigV4-SIGNED by those base credentials — which is
    /// what live STS requires. Absent: today's single unsigned hop against
    /// the mock pair, byte-for-byte unchanged. Role chaining caps the
    /// chained session at one hour, so `duration_secs` must be <= 3600
    /// when a base hop is configured (validated).
    #[serde(default)]
    pub base: Option<BaseHop>,
}

/// The web-identity base hop of the role chain. Both hops talk to the same
/// [`StsConfig::endpoint`]; this block carries the base identity and the
/// SigV4 region for the signed hop-2 call.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaseHop {
    pub web_identity_token: TokenSourceSpec,
    /// The base role: the PLATFORM's own identity. Static by design — the
    /// per-request/team selection happens on the chained hop, never here.
    pub role_arn: String,
    #[serde(default = "default_base_session_name")]
    pub session_name: String,
    /// SigV4 region for the signed hop-2 STS call.
    #[serde(default = "default_region")]
    pub sts_region: String,
}

/// Where the gateway reads its web-identity token: a platform-mounted file
/// (projected service-account token, managed-identity token file) or an
/// environment variable. Exactly one (validated). Deliberately NOT a
/// caller header — the base identity belongs to the platform, never to the
/// request. Shared by the AWS base hop and the Vertex WIF exchange: one
/// generic OIDC source, not a cloud-specific client.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenSourceSpec {
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub env: Option<String>,
}

/// The Vertex auth chain: the platform's OIDC token is exchanged at Google
/// STS for a FEDERATED token (Workload Identity Federation), which then
/// mints a SERVICE-ACCOUNT access token via iamcredentials
/// `generateAccessToken`; that bearer signs the upstream Vertex request.
/// The SA token carries NO per-caller identity — per-caller attribution
/// rides the GB-8 billing labels in the body — so one token is correctly
/// shared across callers and cached per (sa, scopes, pool, provider).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VertexAuth {
    pub web_identity_token: TokenSourceSpec,
    pub wif: WifAudience,
    pub service_account_email: String,
    /// Google STS (`sts.googleapis.com`), as an [`Upstream`] so tests mock it.
    pub sts_endpoint: Upstream,
    /// iamcredentials (`iamcredentials.googleapis.com`), mockable likewise.
    pub iam_endpoint: Upstream,
    #[serde(default = "default_gcp_scopes")]
    pub scopes: Vec<String>,
    /// SA-token lifetime; sent as the "<n>s" string form Google requires.
    #[serde(default = "default_gcp_lifetime")]
    pub lifetime_secs: u32,
}

/// The WIF audience components:
/// `//iam.googleapis.com/projects/{project_number}/locations/global/`
/// `workloadIdentityPools/{pool_id}/providers/{provider_id}` (no scheme —
/// Google STS rejects one).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WifAudience {
    pub project_number: String,
    pub pool_id: String,
    pub provider_id: String,
}

impl WifAudience {
    /// The audience string, exactly as Google STS expects it.
    pub fn audience(&self) -> String {
        format!(
            "//iam.googleapis.com/projects/{}/locations/global/workloadIdentityPools/{}/providers/{}",
            self.project_number, self.pool_id, self.provider_id
        )
    }
}

/// The Google multi-region location names that use the bare API host
/// (`aiplatform.googleapis.com`); every other location prefixes it
/// (`europe-west3-aiplatform.googleapis.com`).
pub const VERTEX_MULTI_REGIONS: [&str; 3] = ["eu", "us", "global"];

/// Derive the Vertex host for a request's location: multi-regions use the
/// configured base host, regional locations prefix it.
pub fn derive_vertex_host(location: &str, base_host: &str) -> String {
    if VERTEX_MULTI_REGIONS.contains(&location) {
        base_host.to_string()
    } else {
        format!("{location}-{base_host}")
    }
}

/// The `<loc>` from a Vertex path's `/locations/<loc>/` segment, if present.
pub fn vertex_path_location(path: &str) -> Option<&str> {
    let rest = &path[path.find("/locations/")? + "/locations/".len()..];
    let end = rest.find('/').unwrap_or(rest.len());
    let loc = &rest[..end];
    if loc.is_empty() {
        None
    } else {
        Some(loc)
    }
}

fn default_gcp_scopes() -> Vec<String> {
    vec!["https://www.googleapis.com/auth/cloud-platform".to_string()]
}

fn default_gcp_lifetime() -> u32 {
    3600
}

fn default_base_session_name() -> String {
    "gatewayd-base".to_string()
}

/// A value the operator decides. Either a bare string (also the template
/// form: `{{key}}` placeholders resolve against adjudicated attribution) or
/// the explicit map form with exactly one of `value` / `from_attribution`.
/// Mirrors the GB-8 LabelSpec static-or-dynamic split; deliberately has NO
/// free CEL arm — role material must not be steerable by request contents
/// (an expression can read caller headers; a template cannot).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum OperatorValueSpec {
    /// Bare-string sugar: `role_arn: "arn:..."` or a `{{key}}` template.
    Bare(String),
    /// Explicit map form.
    Spec(OperatorValueFields),
}

/// The explicit map form of [`OperatorValueSpec`]. Exactly one of `value`
/// (static or `{{key}}` template) or `from_attribution` (single resolved
/// attribution key) must be set — enforced by validation, which reports the
/// precise error the untagged enum cannot.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorValueFields {
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub from_attribution: Option<String>,
}

impl OperatorValueSpec {
    /// The template string this spec resolves through: a bare string or
    /// `value:` is itself the template; `from_attribution: k` is exactly
    /// the one-key template `{{k}}`. Returns `None` when the map form is
    /// mis-specified (neither or both set) — validation reports that.
    pub fn as_template(&self) -> Option<std::borrow::Cow<'_, str>> {
        match self {
            OperatorValueSpec::Bare(s) => Some(std::borrow::Cow::Borrowed(s)),
            OperatorValueSpec::Spec(f) => match (&f.value, &f.from_attribution) {
                (Some(v), None) => Some(std::borrow::Cow::Borrowed(v)),
                (None, Some(k)) => Some(std::borrow::Cow::Owned(format!("{{{{{k}}}}}"))),
                _ => None,
            },
        }
    }
}

/// Operator-forced injection: headers and JSON body fields the operator
/// stamps onto every upstream request for this provider, operator value
/// ALWAYS winning over a caller's. The general mechanism behind Bedrock
/// guardrail forcing (two forced headers + an if_absent guardrailConfig
/// body block are just config), applicable to any provider. Values are
/// [`OperatorValueSpec`]s (static or `{{key}}` templates over adjudicated
/// attribution; template keys must be gateway-established — a caller must
/// never pick which guardrail applies). Unresolvable at request time:
/// fail closed, the GB-4 rejection.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Injection {
    #[serde(default)]
    pub headers: Vec<ForcedHeader>,
    #[serde(default)]
    pub body: Vec<ForcedBodyField>,
}

/// One forced header. Always overrides a caller-sent value, and on signing
/// providers it enters the SIGNED header set (the signature covers what the
/// operator forced, so a stripped or altered value fails verification).
/// `value` is a nested field, never flattened: serde's `flatten` silently
/// disables `deny_unknown_fields`, which is exactly the config-typo
/// acceptance this file exists to prevent.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForcedHeader {
    pub name: String,
    pub value: OperatorValueSpec,
}

/// One forced JSON body field at a dotted path (intermediate objects are
/// created). `if_absent: true` injects only when the path is missing (the
/// Bedrock guardrailConfig semantic); the default overrides
/// unconditionally (operator wins).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForcedBodyField {
    pub path: String,
    pub value: OperatorValueSpec,
    #[serde(default)]
    pub if_absent: bool,
}

/// The closed-set gate for [`StsConfig::allow`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllowList {
    /// The attribution key the gate reads (operator-chosen; nothing is
    /// hardcoded — `team`, `tenant`, `cost_center`, whatever the fleet uses).
    pub key: String,
    /// The operator-approved values. Anything else is rejected.
    pub values: Vec<String>,
}

fn default_session_name_spec() -> OperatorValueSpec {
    OperatorValueSpec::Bare("gatewayd".to_string())
}

fn default_region() -> String {
    "us-east-1".to_string()
}

fn default_sts_duration() -> u32 {
    900
}

/// One session tag: a static operator value, or the resolved value of an
/// attribution key (which validation requires to be pinned, claim-mapped,
/// or derived — never a plain caller-asserted key).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionTag {
    pub key: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub from_attribution: Option<String>,
}

/// One composable policy layer: what fleet, a project, or an app override
/// may specify. Routes carry the same fields inline.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    #[serde(default)]
    pub attribution: Attribution,
    /// GB-8: Vertex billing labels; consulted only where the route's
    /// provider is vertex-kind.
    #[serde(default)]
    pub labels: Vec<LabelEntry>,
    #[serde(default)]
    pub rejections: Option<RejectionOverrides>,
}

/// Apps: the fourth scope. `key` names the attribution key whose RESOLVED
/// value selects the override — an app is an adjudicated identity, never a
/// caller-chosen header alone (the key's value comes out of the
/// fleet→project→route resolution first).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Apps {
    pub key: String,
    #[serde(default)]
    pub overrides: BTreeMap<String, Scope>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    /// Path prefix, matched on segment boundaries: `/openai` matches
    /// `/openai` and `/openai/v1/...`, never `/openaix`. Longest prefix wins.
    pub prefix: String,
    /// Name of a provider in [`Config::providers`].
    pub provider: String,
    /// Optional project reference (scope 2).
    #[serde(default)]
    pub project: Option<String>,
    /// Optional CEL route-match condition beyond the prefix (header/method
    /// predicates). Compiled at load; an erroring condition never selects
    /// the route. Among matching routes: longest prefix wins, then a
    /// conditioned route beats an unconditioned one at the same prefix.
    #[serde(rename = "match", default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub attribution: Attribution,
    #[serde(default)]
    pub labels: Vec<LabelEntry>,
    #[serde(default)]
    pub rejections: Option<RejectionOverrides>,
    /// Filled by [`crate::scope::finalize`]: compiled condition + the
    /// composed effective policies for this route and its app overrides.
    #[serde(skip)]
    pub compiled: Option<CompiledRoute>,
}

impl Route {
    /// The composed fleet→project→route policy. Panics only if the config
    /// skipped finalization — impossible via [`Config::from_yaml`].
    pub fn policy(&self) -> &crate::scope::EffectivePolicy {
        &self.compiled.as_ref().expect("config finalized").effective
    }

    /// The composed policy including the app layer for `app_value`, if an
    /// override exists.
    pub fn app_policy(&self, app_value: &str) -> Option<&crate::scope::EffectivePolicy> {
        self.compiled
            .as_ref()
            .expect("config finalized")
            .apps
            .get(app_value)
    }
}

/// One scope's attribution contract. Every tag on a forwarded request has
/// an origin — assigned (pinned), proven (JWT claim), derived (CEL), or
/// caller — resolved by [`crate::attribution::resolve`] against the
/// COMPOSED policy.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attribution {
    /// GB-1: keys that must be present (from any origin) or the request is
    /// rejected with the effective `missing_attribution` template. May
    /// contain the `<base>` marker to splice the parent scope's list.
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
    /// CEL tier 1: key → expression over `request` + `jwt` (e.g. a claim
    /// transform). Compiled at load; an eval failure leaves the key
    /// unresolved, which for a required key is a GB-4 rejection.
    #[serde(default)]
    pub derived: BTreeMap<String, String>,
    /// GB-5: a spend cap per attribution key. `key → { default, overrides }`
    /// in tokens; the default caps every value of the key, per-value overrides
    /// (Git-reviewed) tighten or loosen one value, and a lower scope's entry
    /// composes over a higher one exactly like the pins (docs/02 GB-5). The
    /// 100k-token scenario is five lines of YAML here. Absent → uncapped.
    #[serde(default)]
    pub spend_caps: BTreeMap<String, SpendCapSpec>,
    /// Model allow-list for this scope: which models the requests it
    /// governs may use. Exact names, or a trailing `*` for a family
    /// (`claude-3*`). Composes down the chain like the rejection templates:
    /// a lower scope's list REPLACES a higher one's. Absent → no gate.
    /// The model comes from the request path (bedrock, vertex) or the
    /// request body (openai dialects); a gated request whose model cannot
    /// be determined is refused, fail closed, with the operator's
    /// `model_not_allowed` body.
    #[serde(default)]
    pub models: Option<Vec<String>>,
}

/// GB-5: one attribution key's spend cap as written in the config. Tokens.
/// `default` caps every value of the key; `overrides` set a per-value cap
/// (`null` inside `overrides` is an explicit "this value is uncapped"). A whole
/// spec with neither is a no-op (uncapped). Composes down the scoped chain via
/// [`crate::budget::KeyCap::compose_child`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpendCapSpec {
    /// The default cap in tokens for every value of this key. Absent → the key
    /// is uncapped unless an override sets a cap.
    #[serde(default)]
    pub default: Option<u64>,
    /// Per-value cap overrides in tokens. A `null` value is an explicit
    /// uncapped override of a capped default.
    #[serde(default)]
    pub overrides: BTreeMap<String, Option<u64>>,
    /// The billing window the counters roll on: `minute`, `hour`, `day`, or
    /// `month` (calendar, UTC). Absent → a lifetime cap that never resets
    /// (the original behavior, unchanged). Windows align on UTC wall-clock
    /// boundaries on every node; residual error is bounded by clock skew.
    #[serde(default)]
    pub window: Option<crate::budget::Window>,
    /// GB-6 alert threshold as a percent (1-100): someone is told when spend
    /// crosses this fraction of the cap; enforcement stays at 100. Absent →
    /// 80. Composes down the chain like the cap itself (lower scope wins).
    #[serde(default)]
    pub alert_at: Option<u8>,
}

impl SpendCapSpec {
    /// The pure [`crate::budget::KeyCap`] this spec compiles to.
    pub fn to_key_cap(&self) -> crate::budget::KeyCap {
        crate::budget::KeyCap {
            default: self.default,
            overrides: self.overrides.clone(),
            window: self.window,
            alert_fraction: self.alert_at.map(|p| f64::from(p) / 100.0),
        }
    }
}

/// One entry in a scope's GB-8 label list: either the `<base>` splice
/// marker (a plain string) or a label mapping.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum LabelEntry {
    Base(String),
    Spec(LabelSpec),
}

/// GB-8: one Vertex billing label. Exactly one of `value` (static),
/// `from_attribution` (a resolved attribution key), or `expression` (CEL
/// over request + jwt + attribution) must be set. Unresolvable at request
/// time → the effective GB-4 rejection, fail closed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabelSpec {
    pub key: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub from_attribution: Option<String>,
    #[serde(default)]
    pub expression: Option<String>,
}

/// GB-4: one operator-defined template per rejection reason. Both reasons
/// are mandatory at fleet scope — a gateway that invents its own 4xx body
/// is exactly what the Baseline forbids.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rejections {
    /// Placeholders: `{{key}}` (the missing keys), `{{route}}` (the prefix).
    pub missing_attribution: RejectionTemplate,
    /// Placeholders: `{{route}}` (the unmatched request path).
    pub unknown_route: RejectionTemplate,
    /// Model gate refusal (scopes with a `models:` allow-list).
    /// Placeholders: `{{model}}`, `{{route}}`. OPTIONAL: absent, the
    /// built-in conservative default applies — the only rejection reason
    /// with a default, because the gate itself is opt-in per scope.
    #[serde(default)]
    pub model_not_allowed: Option<RejectionTemplate>,
}

/// The built-in `model_not_allowed` body used when the operator sets a
/// `models:` gate but no template for it.
pub fn default_model_not_allowed() -> RejectionTemplate {
    RejectionTemplate {
        status: 403,
        content_type: "application/json".to_string(),
        body: r#"{"error":"model_not_allowed","model":"{{model}}","route":"{{route}}"}"#.to_string(),
        streaming: None,
    }
}

/// Lower-scope rejection overrides: each reason overrides independently.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RejectionOverrides {
    #[serde(default)]
    pub missing_attribution: Option<RejectionTemplate>,
    #[serde(default)]
    pub unknown_route: Option<RejectionTemplate>,
    #[serde(default)]
    pub model_not_allowed: Option<RejectionTemplate>,
}

// ------------------------------------------------- model gate (pure helpers)

/// The model named by a request PATH, per provider dialect: bedrock carries
/// it as `/model/<id>/...`, vertex as `/models/<id>:method`. OpenAI-dialect
/// requests carry it in the BODY ([`model_from_body`]). `None` for dialects
/// whose path has no model, or when the segment is absent/empty.
pub fn model_from_path(kind: ProviderKind, path: &str) -> Option<String> {
    let (marker, terminators): (&str, &[char]) = match kind {
        ProviderKind::Bedrock => ("/model/", &['/']),
        ProviderKind::Vertex => ("/models/", &[':', '/']),
        _ => return None,
    };
    let rest = &path[path.find(marker)? + marker.len()..];
    let end = rest
        .find(|c| terminators.contains(&c))
        .unwrap_or(rest.len());
    let model = &rest[..end];
    if model.is_empty() {
        None
    } else {
        Some(model.to_string())
    }
}

/// The `"model"` field of a JSON request body (the openai/anthropic
/// dialects). `None` when the body is not JSON or has no string model.
pub fn model_from_body(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.get("model")?.as_str().map(str::to_string)
}

/// Whether `model` is admitted by the allow-list: exact match, or a
/// trailing-`*` family pattern (`claude-3*`). No other wildcarding, on
/// purpose.
pub fn model_allowed(allow: &[String], model: &str) -> bool {
    allow.iter().any(|pat| match pat.strip_suffix('*') {
        Some(prefix) => model.starts_with(prefix),
        None => pat == model,
    })
}

#[derive(Debug, Clone, Deserialize)]
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
#[derive(Debug, Clone, Deserialize)]
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
    /// Shared secret for HS256 verification: the fleet-minted-token shape
    /// (a fleet that issues its own tokens). Exactly one of this or `jwks`.
    #[serde(default)]
    pub hs256_secret: Option<String>,
    /// INLINE JWKS document (the RS256 / real-IdP shape): the JSON is part
    /// of the config, so key ROTATION is a config change — Git-reviewed and
    /// distributed by the same GB-9 hot-swap as every other rule. No file
    /// watcher, no fetcher, no phone-home; a sidecar or CI job that syncs
    /// the IdP's JWKS into the repo owns freshness. Parsed and validated at
    /// config load into `compiled_jwks`.
    #[serde(default)]
    pub jwks: Option<String>,
    /// Request header carrying `Bearer <token>`.
    #[serde(default = "default_jwt_header")]
    pub header: String,
    /// The parsed `jwks`, populated at load. Never deserialized.
    #[serde(skip)]
    pub compiled_jwks: Option<crate::jwt::Jwks>,
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
        let mut cfg: Config =
            serde_yaml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        crate::scope::finalize(&mut cfg).map_err(ConfigError::Invalid)?;
        Ok(cfg)
    }

    /// Route selection: longest prefix on segment boundaries, with CEL
    /// conditions consulted per request. Among candidate routes whose
    /// prefix matches AND whose condition (if any) evaluates true: the
    /// longest prefix wins; at equal prefix length a conditioned route
    /// beats an unconditioned fallback; remaining ties go to config order.
    /// A condition that ERRORS evaluates as "does not match" — an erroring
    /// predicate can never select a route.
    ///
    /// Callers must pass a path already run through [`normalize_path`]:
    /// matching a raw path lets `/openai/../claims/...` select the `/openai`
    /// contract while the upstream serves `/claims/...` — the governance
    /// bypass the normalization exists to close. The proxy normalizes once
    /// in `request_filter` and forwards the same resolved path upstream.
    pub fn match_route(&self, path: &str, ctx: &EvalCtx) -> Option<&Route> {
        self.routes
            .iter()
            .enumerate()
            .filter(|(_, r)| prefix_matches(&r.prefix, path))
            .filter(|(_, r)| {
                match &r.compiled.as_ref().expect("config finalized").condition {
                    None => true,
                    Some(cond) => cond.eval_bool(ctx).unwrap_or(false),
                }
            })
            .max_by_key(|(i, r)| {
                (
                    r.prefix.trim_end_matches('/').len(),
                    r.condition.is_some(),
                    std::cmp::Reverse(*i),
                )
            })
            .map(|(_, r)| r)
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
pub(crate) fn prefix_matches(prefix: &str, path: &str) -> bool {
    let p = prefix.trim_end_matches('/');
    if p.is_empty() {
        return true; // prefix was "/" (or "//"): the catch-all route
    }
    path == p || path.strip_prefix(p).is_some_and(|rest| rest.starts_with('/'))
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

    fn ctx() -> EvalCtx {
        EvalCtx {
            method: "POST".to_string(),
            ..EvalCtx::default()
        }
    }

    #[test]
    fn valid_config_parses_and_finalizes() {
        let cfg = Config::from_yaml(&valid_yaml()).unwrap();
        assert_eq!(cfg.providers["openai-main"].kind, ProviderKind::OpenAi);
        assert_eq!(cfg.routes[0].attribution.pinned["env"], "prod");
        let policy = cfg.routes[0].policy();
        assert_eq!(policy.pinned["env"], "prod");
        assert_eq!(policy.required_keys, vec!["team"]);
        let streaming = cfg.rejections.missing_attribution.streaming.as_ref().unwrap();
        assert_eq!(streaming.event.as_deref(), Some("error"));
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
        assert_eq!(cfg.match_route(&path, &ctx()).unwrap().prefix, "/claims");
        let path = normalize_path("/openai/%2e%2e/claims/v1/chat");
        assert_eq!(cfg.match_route(&path, &ctx()).unwrap().prefix, "/claims");
    }

    #[test]
    fn route_matching_is_longest_prefix_on_segment_boundaries() {
        let yaml = valid_yaml().replace(
            "routes:",
            "routes:\n  - prefix: /openai/v1/special\n    provider: openai-main",
        );
        let cfg = Config::from_yaml(&yaml).unwrap();
        assert_eq!(cfg.match_route("/openai/v1/chat", &ctx()).unwrap().prefix, "/openai");
        assert_eq!(
            cfg.match_route("/openai/v1/special/x", &ctx()).unwrap().prefix,
            "/openai/v1/special"
        );
        assert_eq!(cfg.match_route("/openai", &ctx()).unwrap().prefix, "/openai");
        assert!(cfg.match_route("/openaix/v1", &ctx()).is_none());
        assert!(cfg.match_route("/other", &ctx()).is_none());
    }

    #[test]
    fn route_conditions_gate_matching_and_erroring_conditions_never_select() {
        let yaml = valid_yaml().replace(
            "routes:",
            concat!(
                "routes:\n",
                "  - prefix: /openai\n",
                "    provider: openai-main\n",
                "    match: 'request.method == \"GET\"'\n",
                "    attribution: { pinned: { tier: readonly } }\n",
                "  - prefix: /cond\n",
                "    provider: openai-main\n",
                "    match: 'request.headers[\"x-absent\"] == \"x\"'\n",
            ),
        );
        let cfg = Config::from_yaml(&yaml).unwrap();

        // POST → the conditioned /openai route does not match; the
        // unconditioned fallback with the same prefix does.
        let post = cfg.match_route("/openai/v1/chat", &ctx()).unwrap();
        assert!(post.condition.is_none());

        // GET → the conditioned route wins over the unconditioned fallback.
        let get_ctx = EvalCtx { method: "GET".to_string(), ..EvalCtx::default() };
        let get = cfg.match_route("/openai/v1/chat", &get_ctx).unwrap();
        assert_eq!(get.condition.as_deref(), Some(r#"request.method == "GET""#));

        // An erroring condition (absent header lookup) never selects.
        assert!(cfg.match_route("/cond/x", &ctx()).is_none());
    }

    // The WASM config block's parse + structural validation (defaults, no-hooks,
    // duplicate names) is exercised in `tests/wasm_config.rs` to keep this file
    // focused; the types live in `crate::wasm_config`, validation in
    // `crate::validate::validate_wasm`.

    #[test]
    fn bad_cel_condition_fails_config_load() {
        let yaml = valid_yaml().replace(
            "provider: openai-main",
            "provider: openai-main\n    match: 'request.method =='",
        );
        match Config::from_yaml(&yaml) {
            Err(ConfigError::Invalid(errs)) => {
                assert!(errs.iter().any(|e| e.contains("parse error")), "{errs:?}")
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }
}
