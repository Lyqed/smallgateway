//! The scoped policy chain: `fleet → project → route → app` (docs/02,
//! "APIM's best idea, done properly").
//!
//! Composition semantics, per field kind:
//!
//! - **Lists** (`required_keys`, `labels`): an ABSENT/empty list inherits
//!   the parent's; a non-empty list WITHOUT the explicit `<base>` marker
//!   REPLACES the parent's (exactly APIM's `<base/>`: leaving it out drops
//!   the inherited chain, on purpose and visibly); a list WITH the marker
//!   splices the parent's list in at the marker's position.
//! - **Maps** (`pinned`, `from_claims`, `derived`): merged; the LOWER scope
//!   wins on the same key — but only within the same origin. A key pinned
//!   at one scope and claim-mapped or derived at another is a
//!   CONTRADICTORY PIN: two origins for one key cannot both hold, and
//!   validation rejects the file with both scopes named.
//! - **Rejection templates**: each reason overrides independently; the
//!   fleet-scope `rejections` block (both reasons mandatory) is the base.
//!
//! Apps are the fourth scope, keyed by the RESOLVED value of one
//! attribution key (`apps.key`): the fleet→project→route chain resolves
//! first, the value selects the app override, and the request re-resolves
//! under the composed route⊕app policy. An app override may not redefine
//! its own selector key — the selection could otherwise invalidate itself.
//!
//! Everything here runs at config load: [`finalize`] validates the file,
//! compiles every CEL expression, composes every chain (each route, and
//! each route × app override), and stamps the results onto the routes.
//! Request time only ever reads a precomposed [`EffectivePolicy`].

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::config::{
    Attribution, Config, LabelEntry, ProviderKind, RejectionOverrides, RejectionTemplate,
    Rejections, Route, Scope, StsConfig, BASE_MARKER,
};
// Field validators live in the sibling module; re-exported so external
// callers keep one import surface for scope semantics.
pub use crate::validate::{
    validate_label_key, validate_label_value, validate_session_tag_key,
    validate_session_tag_value,
};
use crate::validate::{
    check_key, validate_alerts, validate_auth, validate_providers, validate_rejection_overrides,
    validate_telemetry,
    validate_rejections, validate_wasm,
};
use crate::expr::{CompiledExpr, ExprKind};

/// Google Cloud label limits (GB-8), per
/// <https://cloud.google.com/resource-manager/docs/labels-overview#requirements>.
pub const MAX_LABELS: usize = 64;
pub const MAX_LABEL_KEY_LEN: usize = 63;
pub const MAX_LABEL_VALUE_LEN: usize = 63;

/// AWS session tag limits (GB-7).
pub const MAX_SESSION_TAG_KEY_LEN: usize = 128;
pub const MAX_SESSION_TAG_VALUE_LEN: usize = 256;

/// One route's fully-composed policy: what request handling actually
/// consults. Everything is resolved down to values and compiled
/// expressions — no scope walking at request time.
#[derive(Debug, Clone)]
pub struct EffectivePolicy {
    pub required_keys: Vec<String>,
    pub pinned: BTreeMap<String, String>,
    pub from_claims: BTreeMap<String, String>,
    pub derived: BTreeMap<String, Arc<CompiledExpr>>,
    /// GB-8 labels (consulted only when the route's provider is
    /// vertex-kind), in composed order.
    pub labels: Vec<EffectiveLabel>,
    /// GB-5: the composed spend cap per attribution key (tokens). A key absent
    /// here is uncapped. Composed down the chain: a lower scope's `default` and
    /// per-value overrides win over a higher one (docs/02 GB-5).
    pub spend_caps: BTreeMap<String, crate::budget::KeyCap>,
    /// The composed model allow-list; `None` = no gate on this route.
    pub models: Option<Vec<String>>,
    /// The EXACT caller header per attribution key (operator-named; no
    /// built-in convention exists). A key absent here has no caller channel
    /// and its adjudicated value is not forwarded upstream.
    pub headers: BTreeMap<String, String>,
    pub missing_attribution: RejectionTemplate,
    pub unknown_route: RejectionTemplate,
    /// The model-gate refusal body (operator's, or the built-in default).
    pub model_not_allowed: RejectionTemplate,
    /// Allow-list refusal body; `None` = `missing_attribution` speaks.
    pub value_not_allowed: Option<RejectionTemplate>,
    /// Budget refusal body (admission + the mid-stream terminal event via
    /// its `streaming:` half); `None` = `missing_attribution` speaks.
    pub cap_exceeded: Option<RejectionTemplate>,
}

impl EffectivePolicy {
    /// The composed spend cap in tokens for a resolved `key=value`, or `None`
    /// if the key is uncapped on this policy (docs/02 GB-5). The single lookup
    /// the enforcement layer needs per resolved tag.
    pub fn cap_for(&self, key: &str, value: &str) -> Option<u64> {
        self.spend_caps.get(key).and_then(|c| c.cap_for(value))
    }

    /// The composed enforcement TERMS (cap + window + alert threshold) for a
    /// resolved `key=value`, or `None` if uncapped. What the request path
    /// carries per capped tag.
    pub fn terms_for(&self, key: &str, value: &str) -> Option<crate::budget::CapTerms> {
        self.spend_caps.get(key).and_then(|c| c.terms_for(value))
    }
}

impl EffectivePolicy {
    /// Every key this policy knows an origin for.
    pub fn key_universe(&self) -> BTreeSet<&str> {
        self.required_keys
            .iter()
            .map(String::as_str)
            .chain(self.pinned.keys().map(String::as_str))
            .chain(self.from_claims.keys().map(String::as_str))
            .chain(self.derived.keys().map(String::as_str))
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct EffectiveLabel {
    pub key: String,
    pub value: LabelValue,
}

#[derive(Debug, Clone)]
pub enum LabelValue {
    Static(String),
    FromAttribution(String),
    Expr(Arc<CompiledExpr>),
}

/// What [`finalize`] stamps onto each route.
#[derive(Debug)]
pub struct CompiledRoute {
    pub condition: Option<CompiledExpr>,
    /// fleet → project → route.
    pub effective: EffectivePolicy,
    /// app value → (fleet → project → route → app).
    pub apps: BTreeMap<String, EffectivePolicy>,
}

/// One scope, validated and compiled, ready to compose.
struct Layer {
    name: String,
    required_keys: Vec<String>,
    headers: BTreeMap<String, String>,
    pinned: BTreeMap<String, String>,
    from_claims: BTreeMap<String, String>,
    derived: BTreeMap<String, Arc<CompiledExpr>>,
    labels: Vec<LabelItem>,
    spend_caps: BTreeMap<String, crate::budget::KeyCap>,
    models: Option<Vec<String>>,
    missing_attribution: Option<RejectionTemplate>,
    unknown_route: Option<RejectionTemplate>,
    model_not_allowed: Option<RejectionTemplate>,
    value_not_allowed: Option<RejectionTemplate>,
    cap_exceeded: Option<RejectionTemplate>,
}

enum LabelItem {
    Base,
    Label(String, LabelValue),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OriginKind {
    Pinned,
    Claim,
    Derived,
}

impl OriginKind {
    fn label(self) -> &'static str {
        match self {
            OriginKind::Pinned => "pinned",
            OriginKind::Claim => "claim-mapped",
            OriginKind::Derived => "derived",
        }
    }
}

/// Validate + compile + compose the whole file. On success every route
/// carries its [`CompiledRoute`]; on failure ALL collected errors return.
pub fn finalize(cfg: &mut Config) -> Result<(), Vec<String>> {
    let mut errs = Vec::new();

    validate_providers(cfg, &mut errs);
    validate_auth(cfg, &mut errs);
    validate_telemetry(cfg, &mut errs);
    validate_alerts(cfg, &mut errs);
    validate_wasm(cfg, &mut errs);
    validate_rejections(&cfg.rejections, "rejections", &mut errs);

    // Per-scope validation + compilation into layers.
    let fleet_layer = cfg
        .fleet
        .as_ref()
        .map(|s| compile_scope(s, "fleet", &mut errs));
    let project_layers: BTreeMap<&str, Layer> = cfg
        .projects
        .iter()
        .map(|(name, s)| {
            (
                name.as_str(),
                compile_scope(s, &format!("project {name:?}"), &mut errs),
            )
        })
        .collect();
    let app_selector = cfg.apps.as_ref().map(|a| a.key.clone());
    let app_layers: BTreeMap<&str, Layer> = match &cfg.apps {
        None => BTreeMap::new(),
        Some(apps) => {
            check_key(&apps.key, "apps.key", &mut errs);
            apps.overrides
                .iter()
                .map(|(value, s)| {
                    let name = format!("app {value:?}");
                    if value.trim().is_empty() {
                        errs.push("apps.overrides: empty app value".to_string());
                    }
                    let layer = compile_scope(s, &name, &mut errs);
                    // Caller headers are read BEFORE the app is selected, so
                    // an app override cannot rename the caller channel.
                    if !layer.headers.is_empty() {
                        errs.push(format!(
                            "{name}: attribution.headers is route-scope and \
                             above — the caller channel must be known before \
                             the app override is selected"
                        ));
                    }
                    // The selector key must not be redefined by the layer
                    // it selects — the selection could invalidate itself.
                    for (map_name, has) in [
                        ("pinned", s.attribution.pinned.contains_key(&apps.key)),
                        ("from_claims", s.attribution.from_claims.contains_key(&apps.key)),
                        ("derived", s.attribution.derived.contains_key(&apps.key)),
                    ] {
                        if has {
                            errs.push(format!(
                                "{name}: {map_name} redefines the app selector key \
                                 {:?}; an app override may not change how its own \
                                 selector resolves",
                                apps.key
                            ));
                        }
                    }
                    (value.as_str(), layer)
                })
                .collect()
        }
    };

    // Route validation + per-route composition.
    let mut seen_unconditioned = BTreeSet::new();
    let mut compiled: Vec<CompiledRoute> = Vec::with_capacity(cfg.routes.len());
    for route in &cfg.routes {
        let label = format!("route {:?}", route.prefix);
        if !route.prefix.starts_with('/') {
            errs.push(format!("{label}: prefix must start with '/'"));
        }
        if route.condition.is_none()
            && !seen_unconditioned.insert(route.prefix.trim_end_matches('/').to_string())
        {
            errs.push(format!(
                "{label}: duplicate prefix without a `match` condition \
                 (two unconditioned routes on one prefix are ambiguous)"
            ));
        }
        if !cfg.providers.contains_key(&route.provider) {
            let known: Vec<&str> = cfg.providers.keys().map(String::as_str).collect();
            errs.push(format!(
                "{label}: unknown provider {:?} (defined providers: {})",
                route.provider,
                known.join(", ")
            ));
        }
        let condition = route.condition.as_deref().and_then(|src| {
            match CompiledExpr::compile(src, ExprKind::Condition) {
                Ok(c) => Some(c),
                Err(e) => {
                    errs.push(format!("{label}: match: {e}"));
                    None
                }
            }
        });
        let project_layer = match &route.project {
            None => None,
            Some(p) => match project_layers.get(p.as_str()) {
                Some(layer) => Some(layer),
                None => {
                    let known: Vec<&str> = cfg.projects.keys().map(String::as_str).collect();
                    errs.push(format!(
                        "{label}: unknown project {:?} (defined projects: {})",
                        p,
                        known.join(", ")
                    ));
                    None
                }
            },
        };
        let route_layer = compile_route_layer(route, &label, &mut errs);

        let mut chain: Vec<&Layer> = Vec::new();
        if let Some(f) = &fleet_layer {
            chain.push(f);
        }
        if let Some(p) = project_layer {
            chain.push(p);
        }
        chain.push(&route_layer);

        let effective = compose(&cfg.rejections, &chain, &label, &mut errs);
        let mut apps = BTreeMap::new();
        for (value, app_layer) in &app_layers {
            let mut app_chain = chain.clone();
            app_chain.push(app_layer);
            let composed = compose(
                &cfg.rejections,
                &app_chain,
                &format!("{label} ⊕ app {value:?}"),
                &mut errs,
            );
            apps.insert(value.to_string(), composed);
        }

        // Cross-scope checks over the COMPOSED policies.
        let provider = cfg.providers.get(&route.provider);
        for (ctx_name, policy) in std::iter::once((label.clone(), &effective))
            .chain(apps.iter().map(|(v, p)| (format!("{label} ⊕ app {v:?}"), p)))
        {
            if !policy.from_claims.is_empty() && cfg.auth.is_none() {
                errs.push(format!("{ctx_name}: from_claims requires auth.jwt to be configured"));
            }
            validate_attr_headers(
                &ctx_name,
                policy,
                provider.and_then(|p| p.sts.as_ref()),
                &mut errs,
            );
            if let Some(p) = provider {
                validate_effective_for_provider(
                    &ctx_name,
                    policy,
                    p.kind,
                    p.sts.as_ref(),
                    p.inject.as_ref(),
                    &mut errs,
                );
            }
        }
        if !route.labels.is_empty()
            && provider.is_some_and(|p| p.kind != ProviderKind::Vertex)
        {
            errs.push(format!(
                "{label}: labels require a vertex-kind provider ({:?} is {})",
                route.provider,
                provider.map(|p| p.kind.name()).unwrap_or("?"),
            ));
        }
        // The app selector must be resolvable on every route when apps
        // exist: it has to appear in the base chain's key universe.
        if let Some(selector) = &app_selector {
            if !app_layers.is_empty() && !effective.key_universe().contains(selector.as_str()) {
                errs.push(format!(
                    "{label}: apps are keyed by {selector:?} but the composed \
                     fleet→project→route chain never requires, pins, claim-maps, \
                     or derives that key — no request on this route could ever \
                     select an app override"
                ));
            }
        }

        compiled.push(CompiledRoute { condition, effective, apps });
    }

    if cfg.routes.is_empty() {
        errs.push("routes: at least one route is required".to_string());
    }

    if errs.is_empty() {
        for (route, c) in cfg.routes.iter_mut().zip(compiled) {
            route.compiled = Some(c);
        }
        // GB-2 RS256: the JWKS parses at LOAD (validate_auth reported any
        // error above, so this parse cannot fail here) — key rotation is a
        // config change, distributed by the same hot-swap as every rule.
        if let Some(auth) = &mut cfg.auth {
            if let Some(jwks) = &auth.jwt.jwks {
                auth.jwt.compiled_jwks =
                    Some(crate::jwt::Jwks::parse(jwks).expect("validated above"));
            }
        }
        Ok(())
    } else {
        Err(errs)
    }
}

/// GB-7/GB-8 checks that need the composed policy AND the provider.
/// Headers owned by transport, signing, or upstream credentials: naming one
/// as an attribution channel would corrupt the request or strip real auth.
const RESERVED_ATTR_HEADERS: [&str; 9] = [
    "host",
    "authorization",
    "content-length",
    "transfer-encoding",
    "content-type",
    "api-key",
    "x-amz-date",
    "x-amz-security-token",
    "x-amz-content-sha256",
];

/// The `attribution.headers` contract, checked over the COMPOSED policy:
/// there is deliberately NO default header name, so every caller-sourced
/// key must be named by the operator, every name must be a safe token, and
/// no two keys may share one header.
fn validate_attr_headers(
    ctx_name: &str,
    policy: &EffectivePolicy,
    sts: Option<&StsConfig>,
    errs: &mut Vec<String>,
) {
    // A mapped key outside this chain's universe is inert, not an error: a
    // fleet-wide map may name keys only some routes use.
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for (key, name) in &policy.headers {
        let ok = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !ok {
            errs.push(format!(
                "{ctx_name}: header {name:?} for key {key:?} must be non-empty \
                 [a-z0-9-] (proxies commonly drop underscore headers)"
            ));
        }
        if RESERVED_ATTR_HEADERS.contains(&name.as_str()) {
            errs.push(format!(
                "{ctx_name}: header {name:?} for key {key:?} is owned by \
                 transport/signing/credentials and cannot carry attribution"
            ));
        }
        if let Some(prev) = seen.insert(name.as_str(), key.as_str()) {
            errs.push(format!(
                "{ctx_name}: header {name:?} is named for both {prev:?} and \
                 {key:?} — one header, one key"
            ));
        }
    }
    let established = |key: &str| {
        policy.pinned.contains_key(key)
            || policy.from_claims.contains_key(key)
            || policy.derived.contains_key(key)
    };
    for key in &policy.required_keys {
        if !established(key) && !policy.headers.contains_key(key) {
            errs.push(format!(
                "{ctx_name}: required key {key:?} has no source — not pinned, \
                 claim-mapped, or derived, and no caller header is named for \
                 it under attribution.headers (there is no default header name)"
            ));
        }
    }
    if let Some(allow) = sts.and_then(|s| s.allow.as_ref()) {
        if !established(&allow.key) && !policy.headers.contains_key(&allow.key) {
            errs.push(format!(
                "{ctx_name}: sts allow key {:?} has no source — it is neither \
                 gateway-established nor named under attribution.headers, so \
                 every request would be refused",
                allow.key
            ));
        }
    }
}

fn validate_effective_for_provider(
    ctx_name: &str,
    policy: &EffectivePolicy,
    kind: ProviderKind,
    sts: Option<&StsConfig>,
    inject: Option<&crate::config::Injection>,
    errs: &mut Vec<String>,
) {
    // Operator-forced injection templates: referenced keys must exist on
    // the chain AND be gateway-established. No allow-list exception here —
    // a guardrail identifier is pure operator policy; a caller must never
    // pick which guardrail applies.
    if let Some(inject) = inject {
        let universe = policy.key_universe();
        let specs = inject
            .headers
            .iter()
            .map(|h| (format!("inject header {:?}", h.name), &h.value))
            .chain(inject.body.iter().map(|f| (format!("inject body {:?}", f.path), &f.value)));
        for (what, spec) in specs {
            let Some(template) = spec.as_template() else { continue };
            let Ok(keys) = crate::template::placeholders(&template) else { continue };
            for key in keys {
                if !universe.contains(key.as_str()) {
                    errs.push(format!(
                        "{ctx_name}: {what} references attribution key {key:?}, which \
                         the composed chain never requires, pins, claim-maps, or \
                         derives — it could never resolve"
                    ));
                    continue;
                }
                let gateway_established = policy.pinned.contains_key(&key)
                    || policy.from_claims.contains_key(&key)
                    || policy.derived.contains_key(&key);
                if !gateway_established {
                    errs.push(format!(
                        "{ctx_name}: {what} references attribution key {key:?}, which \
                         is caller-asserted (not pinned, claim-mapped, or derived) — \
                         forced injection is operator policy, never caller-steerable"
                    ));
                }
            }
        }
    }
    if kind == ProviderKind::Vertex {
        for l in &policy.labels {
            if let LabelValue::FromAttribution(key) = &l.value {
                if !policy.key_universe().contains(key.as_str()) {
                    errs.push(format!(
                        "{ctx_name}: label {:?} references attribution key {key:?}, \
                         which the composed chain never requires, pins, claim-maps, \
                         or derives — it could never resolve",
                        l.key
                    ));
                }
            }
        }
    }
    if let Some(sts) = sts {
        // GB-7's static never-caller-raw guarantee: a session tag may only
        // reference keys whose value the GATEWAY establishes (pinned,
        // claim-mapped, or derived). A plain required key is caller-
        // asserted and can never become an invoice-grade session tag.
        for tag in &sts.tags {
            if let Some(key) = &tag.from_attribution {
                let gateway_established = policy.pinned.contains_key(key)
                    || policy.from_claims.contains_key(key)
                    || policy.derived.contains_key(key);
                if !gateway_established {
                    errs.push(format!(
                        "{ctx_name}: sts tag {:?} references attribution key {key:?}, \
                         which is not pinned, claim-mapped, or derived on this chain — \
                         session tags are operator/attribution-derived only, never \
                         caller-raw",
                        tag.key
                    ));
                }
            }
        }
        // Role material (role_arn / session_name templates) follows the same
        // rule, with ONE deliberate exception: a caller-asserted key gated by
        // this block's allow-list is admissible, because the allow-list closes
        // the value set to operator-approved members — the caller picks WHICH
        // pre-built role, never a new one.
        let universe = policy.key_universe();
        if let Some(allow) = &sts.allow {
            if !universe.contains(allow.key.as_str()) {
                errs.push(format!(
                    "{ctx_name}: sts allow.key {:?} is a key the composed chain never \
                     requires, pins, claim-maps, or derives — it could never resolve",
                    allow.key
                ));
            }
        }
        for (what, spec) in [("role_arn", &sts.role_arn), ("session_name", &sts.session_name)] {
            let Some(template) = spec.as_template() else { continue };
            let Ok(keys) = crate::template::placeholders(&template) else { continue };
            for key in keys {
                if !universe.contains(key.as_str()) {
                    errs.push(format!(
                        "{ctx_name}: sts {what} references attribution key {key:?}, \
                         which the composed chain never requires, pins, claim-maps, \
                         or derives — it could never resolve"
                    ));
                    continue;
                }
                let gateway_established = policy.pinned.contains_key(&key)
                    || policy.from_claims.contains_key(&key)
                    || policy.derived.contains_key(&key);
                let allow_gated = sts
                    .allow
                    .as_ref()
                    .is_some_and(|a| a.key == key);
                if !gateway_established && !allow_gated {
                    errs.push(format!(
                        "{ctx_name}: sts {what} references attribution key {key:?}, \
                         which is caller-asserted (not pinned, claim-mapped, or derived) \
                         and not gated by this block's allow-list — a caller must never \
                         steer which role is assumed"
                    ));
                }
            }
        }
    }
}

fn compile_scope(scope: &Scope, name: &str, errs: &mut Vec<String>) -> Layer {
    compile_layer(
        name,
        &scope.attribution,
        &scope.labels,
        scope.rejections.as_ref(),
        errs,
    )
}

fn compile_route_layer(route: &Route, name: &str, errs: &mut Vec<String>) -> Layer {
    compile_layer(
        name,
        &route.attribution,
        &route.labels,
        route.rejections.as_ref(),
        errs,
    )
}

/// Per-scope validation + CEL compilation. Errors carry the scope's name;
/// composition later never re-validates what a single scope can establish.
fn compile_layer(
    name: &str,
    attr: &Attribution,
    labels: &[LabelEntry],
    rejections: Option<&RejectionOverrides>,
    errs: &mut Vec<String>,
) -> Layer {
    let mut seen_required = BTreeSet::new();
    let mut markers = 0usize;
    for key in &attr.required_keys {
        if key == BASE_MARKER {
            markers += 1;
            if markers > 1 {
                errs.push(format!("{name}: required_keys: more than one {BASE_MARKER} marker"));
            }
            continue;
        }
        check_key(key, &format!("{name}: required_keys"), errs);
        if !seen_required.insert(key.as_str()) {
            errs.push(format!("{name}: required_keys: duplicate key {key:?}"));
        }
    }
    for (key, value) in &attr.pinned {
        check_key(key, &format!("{name}: pinned"), errs);
        if value.is_empty() {
            errs.push(format!("{name}: pinned {key:?}: value must not be empty"));
        }
    }
    for (key, claim) in &attr.from_claims {
        check_key(key, &format!("{name}: from_claims"), errs);
        if claim.trim().is_empty() {
            errs.push(format!("{name}: from_claims {key:?}: claim name must not be empty"));
        }
    }
    let mut derived = BTreeMap::new();
    for (key, src) in &attr.derived {
        check_key(key, &format!("{name}: derived"), errs);
        match CompiledExpr::compile(src, ExprKind::Derived) {
            Ok(c) => {
                derived.insert(key.clone(), Arc::new(c));
            }
            Err(e) => errs.push(format!("{name}: derived {key:?}: {e}")),
        }
    }
    // Same-scope cross-origin conflicts: one key, one origin.
    for key in attr.from_claims.keys() {
        if attr.pinned.contains_key(key) {
            errs.push(format!(
                "{name}: key {key:?} is both pinned and claim-mapped; pick one origin"
            ));
        }
    }
    for key in attr.derived.keys() {
        if attr.pinned.contains_key(key) {
            errs.push(format!(
                "{name}: key {key:?} is both pinned and derived; pick one origin"
            ));
        }
        if attr.from_claims.contains_key(key) {
            errs.push(format!(
                "{name}: key {key:?} is both claim-mapped and derived; pick one origin"
            ));
        }
    }

    let mut label_items = Vec::new();
    let mut seen_labels = BTreeSet::new();
    let mut label_markers = 0usize;
    for entry in labels {
        match entry {
            LabelEntry::Base(s) if s == BASE_MARKER => {
                label_markers += 1;
                if label_markers > 1 {
                    errs.push(format!("{name}: labels: more than one {BASE_MARKER} marker"));
                }
                label_items.push(LabelItem::Base);
            }
            LabelEntry::Base(s) => errs.push(format!(
                "{name}: labels: unexpected entry {s:?} (did you mean {BASE_MARKER:?}?)"
            )),
            LabelEntry::Spec(spec) => {
                let lctx = format!("{name}: label {:?}", spec.key);
                if let Err(e) = validate_label_key(&spec.key) {
                    errs.push(format!("{lctx}: {e}"));
                }
                if !seen_labels.insert(spec.key.as_str()) {
                    errs.push(format!("{lctx}: duplicate label key in one scope"));
                }
                let value = match (&spec.value, &spec.from_attribution, &spec.expression) {
                    (Some(v), None, None) => {
                        if let Err(e) = validate_label_value(v) {
                            errs.push(format!("{lctx}: {e}"));
                        }
                        Some(LabelValue::Static(v.clone()))
                    }
                    (None, Some(k), None) => {
                        check_key(k, &format!("{lctx}: from_attribution"), errs);
                        Some(LabelValue::FromAttribution(k.clone()))
                    }
                    (None, None, Some(src)) => match CompiledExpr::compile(src, ExprKind::Label) {
                        Ok(c) => Some(LabelValue::Expr(Arc::new(c))),
                        Err(e) => {
                            errs.push(format!("{lctx}: {e}"));
                            None
                        }
                    },
                    _ => {
                        errs.push(format!(
                            "{lctx}: exactly one of 'value', 'from_attribution', or \
                             'expression' must be set"
                        ));
                        None
                    }
                };
                if let Some(value) = value {
                    label_items.push(LabelItem::Label(spec.key.clone(), value));
                }
            }
        }
    }

    if let Some(o) = rejections {
        validate_rejection_overrides(o, name, errs);
    }

    // GB-5: compile each key's spend-cap spec into a pure KeyCap. A cap on a
    // key the chain never establishes an origin for is harmless (it just never
    // matches a resolved tag), so no cross-field validation is needed here.
    let mut spend_caps = BTreeMap::new();
    for (key, spec) in &attr.spend_caps {
        check_key(key, &format!("{name}: spend_caps"), errs);
        if let Some(pct) = spec.alert_at {
            if !(1..=100).contains(&pct) {
                errs.push(format!(
                    "{name}: spend_caps {key:?}: alert_at must be 1-100 (percent), got {pct}"
                ));
            }
        }
        spend_caps.insert(key.clone(), spec.to_key_cap());
    }

    if let Some(models) = &attr.models {
        if models.is_empty() {
            errs.push(format!("{name}: models must not be an empty list (omit for no gate)"));
        }
        for m in models {
            if m.trim().is_empty() || m.trim_end_matches('*').contains('*') {
                errs.push(format!(
                    "{name}: models entry {m:?} must be a name or a trailing-* family pattern"
                ));
            }
        }
    }

    Layer {
        name: name.to_string(),
        required_keys: attr.required_keys.clone(),
        headers: attr.headers.clone(),
        pinned: attr.pinned.clone(),
        from_claims: attr.from_claims.clone(),
        derived,
        labels: label_items,
        spend_caps,
        models: attr.models.clone(),
        missing_attribution: rejections.and_then(|o| o.missing_attribution.clone()),
        unknown_route: rejections.and_then(|o| o.unknown_route.clone()),
        model_not_allowed: rejections.and_then(|o| o.model_not_allowed.clone()),
        value_not_allowed: rejections.and_then(|o| o.value_not_allowed.clone()),
        cap_exceeded: rejections.and_then(|o| o.cap_exceeded.clone()),
    }
}

/// Compose a chain of layers (top scope first) into one effective policy.
fn compose(
    base: &Rejections,
    chain: &[&Layer],
    ctx_name: &str,
    errs: &mut Vec<String>,
) -> EffectivePolicy {
    // Lists: fold with the explicit-base-marker splice.
    let mut required: Vec<String> = Vec::new();
    for layer in chain {
        required = splice_required(&required, &layer.required_keys);
    }
    dedup_keep_first(&mut required);

    // Maps: merge, lower scope wins WITHIN one origin; cross-origin = the
    // contradictory pin validation this composition exists to catch.
    let mut origin: BTreeMap<String, (OriginKind, String)> = BTreeMap::new();
    let mut pinned = BTreeMap::new();
    let mut from_claims = BTreeMap::new();
    let mut derived = BTreeMap::new();
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    for layer in chain {
        let sets: [(OriginKind, Vec<&String>); 3] = [
            (OriginKind::Pinned, layer.pinned.keys().collect()),
            (OriginKind::Claim, layer.from_claims.keys().collect()),
            (OriginKind::Derived, layer.derived.keys().collect()),
        ];
        for (kind, keys) in sets {
            for key in keys {
                match origin.get(key) {
                    Some((prev, owner)) if *prev != kind => errs.push(format!(
                        "{ctx_name}: contradictory pin: key {key:?} is {} at {owner} \
                         but {} at {}; one key, one origin down the chain",
                        prev.label(),
                        kind.label(),
                        layer.name
                    )),
                    _ => {
                        origin.insert(key.clone(), (kind, layer.name.clone()));
                    }
                }
            }
        }
        for (k, v) in &layer.pinned {
            pinned.insert(k.clone(), v.clone());
        }
        for (k, v) in &layer.from_claims {
            from_claims.insert(k.clone(), v.clone());
        }
        for (k, v) in &layer.headers {
            // Header names are case-insensitive on the wire; one canonical
            // lowercase form everywhere.
            headers.insert(k.clone(), v.to_ascii_lowercase());
        }
        for (k, v) in &layer.derived {
            derived.insert(k.clone(), Arc::clone(v));
        }
    }

    // Labels: same splice semantics, then deeper-scope-wins on key clash.
    let mut labels: Vec<(usize, String, LabelValue)> = Vec::new();
    for (depth, layer) in chain.iter().enumerate() {
        if layer.labels.is_empty() {
            continue; // absent → inherit
        }
        let mut next: Vec<(usize, String, LabelValue)> = Vec::new();
        let mut spliced = false;
        for item in &layer.labels {
            match item {
                LabelItem::Base => {
                    if !spliced {
                        next.extend(labels.iter().cloned());
                        spliced = true;
                    }
                }
                LabelItem::Label(k, v) => next.push((depth, k.clone(), v.clone())),
            }
        }
        labels = next;
    }
    // Key clash: the deeper scope's entry survives, in its own position.
    let mut final_labels: Vec<EffectiveLabel> = Vec::new();
    let mut best: BTreeMap<&str, usize> = BTreeMap::new(); // key → max depth
    for (depth, key, _) in &labels {
        let e = best.entry(key.as_str()).or_insert(*depth);
        if depth > e {
            *e = *depth;
        }
    }
    let mut emitted = BTreeSet::new();
    for (depth, key, value) in &labels {
        if best[key.as_str()] == *depth && emitted.insert(key.clone()) {
            final_labels.push(EffectiveLabel { key: key.clone(), value: value.clone() });
        }
    }
    if final_labels.len() > MAX_LABELS {
        errs.push(format!(
            "{ctx_name}: {} labels after composition exceed the Google Cloud \
             limit of {MAX_LABELS}",
            final_labels.len()
        ));
    }

    // GB-5 spend caps: compose down the chain, lower scope winning per key
    // (compose_child folds the child's default + per-value overrides over the
    // parent's) — the cap analog of the pin merge above.
    let mut spend_caps: BTreeMap<String, crate::budget::KeyCap> = BTreeMap::new();
    for layer in chain {
        for (key, cap) in &layer.spend_caps {
            let composed = match spend_caps.get(key) {
                Some(parent) => parent.compose_child(cap),
                None => cap.clone(),
            };
            spend_caps.insert(key.clone(), composed);
        }
    }

    // Model gate: a lower scope's list REPLACES a higher one's (a route
    // narrowing the fleet's families to one exact model must not merge).
    let mut models: Option<Vec<String>> = None;
    for layer in chain {
        if let Some(m) = &layer.models {
            models = Some(m.clone());
        }
    }

    // Rejections: base, then per-reason overrides down the chain.
    let mut missing_attribution = base.missing_attribution.clone();
    let mut unknown_route = base.unknown_route.clone();
    let mut model_not_allowed = base
        .model_not_allowed
        .clone()
        .unwrap_or_else(crate::config::default_model_not_allowed);
    let mut value_not_allowed = base.value_not_allowed.clone();
    let mut cap_exceeded = base.cap_exceeded.clone();
    for layer in chain {
        if let Some(t) = &layer.missing_attribution {
            missing_attribution = t.clone();
        }
        if let Some(t) = &layer.unknown_route {
            unknown_route = t.clone();
        }
        if let Some(t) = &layer.model_not_allowed {
            model_not_allowed = t.clone();
        }
        if let Some(t) = &layer.value_not_allowed {
            value_not_allowed = Some(t.clone());
        }
        if let Some(t) = &layer.cap_exceeded {
            cap_exceeded = Some(t.clone());
        }
    }

    EffectivePolicy {
        required_keys: required,
        headers,
        pinned,
        from_claims,
        derived,
        labels: final_labels,
        spend_caps,
        models,
        missing_attribution,
        unknown_route,
        model_not_allowed,
        value_not_allowed,
        cap_exceeded,
    }
}

/// The explicit-base-marker list semantics for `required_keys`.
fn splice_required(parent: &[String], child: &[String]) -> Vec<String> {
    if child.is_empty() {
        return parent.to_vec(); // absent → inherit
    }
    let mut out = Vec::with_capacity(parent.len() + child.len());
    let mut spliced = false;
    for entry in child {
        if entry == BASE_MARKER {
            if !spliced {
                out.extend(parent.iter().cloned());
                spliced = true;
            }
        } else {
            out.push(entry.clone());
        }
    }
    out
}

fn dedup_keep_first(list: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    list.retain(|k| seen.insert(k.clone()));
}

