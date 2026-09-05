//! Config-PR admission (docs/07-control-plane.md, admission on config PRs;
//! docs/02-architecture.md, ArgoCD "Admission policies").
//!
//! Admission runs against a **candidate** config — a commit, a directory, or an
//! already-rendered flat config — BEFORE it can become desired. A failing
//! candidate is blocked from being rendered/rolled out, with a precise,
//! per-rule error naming exactly what is wrong. Two rule families:
//!
//! - **Built-in rules** ([`builtin`]): the Baseline guarantees the gateway must
//!   never ship without. GB-1 (every route carries attribution keys), GB-4 (both
//!   mandatory rejection templates present), a forbidden-construct gate (an
//!   in-gateway-templating escape hatch is banned — docs/07 bans it precisely so
//!   the reviewed diff is the served diff), and an override-governance gate (an
//!   app override that raises a pinned numeric cap beyond a configured factor
//!   must carry a justifying label).
//! - **CEL-expressed rules** ([`CelRule`]): operator-authored predicates over
//!   the candidate document, evaluated with the same sandboxed `cel` interpreter
//!   gateway-core compiles route conditions with. A rule is `{ id, expr,
//!   message }`; the expression evaluates against a `config` variable (the flat
//!   candidate as a CEL map) and must return `true` to ADMIT. `false` (or a
//!   non-bool / eval error) is a rejection carrying `message`.
//!
//! Admission is a pure function of the candidate plus the ruleset — no wall
//! clock, no I/O — so a CI `gatewayctl admit` and the pre-rollout gate reach the
//! identical verdict on the identical candidate.

use gateway_core::config::{Config, LabelEntry, Scope};

use crate::render::{parse_rendered, RenderError};
use crate::source::{ConfigSource, SourceError};

/// The default factor beyond which an app override of a numeric pinned cap must
/// carry a justifying label (see [`builtin::override_governance`]). A 4x jump in
/// a per-app TPM cap without a label is the kind of unreviewed blast-radius
/// change admission exists to catch.
pub const DEFAULT_OVERRIDE_FACTOR: f64 = 4.0;

/// The label an over-factor override must carry to be admitted.
pub const OVERRIDE_JUSTIFICATION_LABEL: &str = "override-approved";

/// One admission failure: which rule failed and why, precise enough for a CI
/// annotation or a PR comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleFailure {
    /// Stable rule id (`GB-1`, `GB-4`, `no-forbidden-construct`, a CEL rule id).
    pub rule: String,
    /// Human-readable, specific: names the route/app/key at fault.
    pub detail: String,
}

impl std::fmt::Display for RuleFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.rule, self.detail)
    }
}

/// The verdict: admitted, or blocked with every failing rule collected (an
/// operator fixes the PR once, not rule-by-rule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Admit,
    Block(Vec<RuleFailure>),
}

impl Verdict {
    pub fn is_admitted(&self) -> bool {
        matches!(self, Verdict::Admit)
    }

    /// The failures, or an empty slice when admitted.
    pub fn failures(&self) -> &[RuleFailure] {
        match self {
            Verdict::Admit => &[],
            Verdict::Block(f) => f,
        }
    }
}

/// A failure to even evaluate admission (the candidate did not parse). This is
/// distinct from a `Block` verdict: a candidate that will not render is a harder
/// failure than one that renders but violates a rule.
#[derive(Debug)]
pub enum AdmitError {
    /// The candidate could not be rendered to a flat config at all.
    Unrenderable(String),
}

impl std::fmt::Display for AdmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdmitError::Unrenderable(e) => write!(f, "candidate is unrenderable: {e}"),
        }
    }
}

impl std::error::Error for AdmitError {}

impl From<RenderError> for AdmitError {
    fn from(e: RenderError) -> AdmitError {
        AdmitError::Unrenderable(e.to_string())
    }
}

impl From<SourceError> for AdmitError {
    fn from(e: SourceError) -> AdmitError {
        AdmitError::Unrenderable(e.to_string())
    }
}

/// One operator-authored CEL admission rule.
#[derive(Debug, Clone)]
pub struct CelRule {
    pub id: String,
    /// A CEL expression over `config` (the flat candidate as a map). Must return
    /// `true` to ADMIT.
    pub expr: String,
    /// The rejection detail surfaced when the rule does not admit.
    pub message: String,
}

/// The admission policy: the built-in gates plus any CEL rules. Built-ins are
/// always on; CEL rules are additive.
#[derive(Debug, Clone, Default)]
pub struct AdmissionPolicy {
    pub cel_rules: Vec<CelRule>,
    /// The override factor gate's threshold. `None` uses [`DEFAULT_OVERRIDE_FACTOR`].
    pub override_factor: Option<f64>,
}

impl AdmissionPolicy {
    pub fn new() -> AdmissionPolicy {
        AdmissionPolicy::default()
    }

    pub fn with_cel_rule(mut self, id: &str, expr: &str, message: &str) -> AdmissionPolicy {
        self.cel_rules.push(CelRule {
            id: id.to_string(),
            expr: expr.to_string(),
            message: message.to_string(),
        });
        self
    }

    fn factor(&self) -> f64 {
        self.override_factor.unwrap_or(DEFAULT_OVERRIDE_FACTOR)
    }

    /// Admit (or block) a candidate rendered flat-config YAML string. The
    /// candidate MUST already be a valid `Config` (render validated it); this
    /// applies the admission rules ON TOP of validity.
    pub fn admit_yaml(&self, flat_yaml: &str) -> Result<Verdict, AdmitError> {
        let cfg = parse_rendered(flat_yaml.as_bytes())?;
        let mut failures = Vec::new();

        // Built-in rules first (the Baseline non-negotiables).
        failures.extend(builtin::attribution_keys_present(&cfg));
        failures.extend(builtin::rejection_templates_present(&cfg));
        failures.extend(builtin::no_forbidden_construct(flat_yaml));
        failures.extend(builtin::override_governance(&cfg, self.factor()));
        failures.extend(builtin::no_unsigned_wasm_module(&cfg));

        // CEL rules over the candidate document.
        let doc = candidate_to_json(flat_yaml);
        for rule in &self.cel_rules {
            if let Some(f) = eval_cel_rule(rule, &doc) {
                failures.push(f);
            }
        }

        Ok(if failures.is_empty() {
            Verdict::Admit
        } else {
            Verdict::Block(failures)
        })
    }

    /// Admit a candidate config **source** (a directory or a Git ref/commit):
    /// resolve + render it, then apply the rules — INCLUDING the GatewaySet-
    /// stamped variants (a GatewaySet is admission-checked like any config,
    /// docs/02). This is the pre-rollout gate and the `admit` subcommand's core.
    pub fn admit_source(&self, source: &dyn ConfigSource) -> Result<Verdict, AdmitError> {
        let resolved = source.resolve()?;
        self.admit_resolved(&resolved)
    }

    /// Admit an already-resolved repo: the fleet-wide render FIRST, then every
    /// GatewaySet's stamped effect. A GatewaySet overlay changes what a matching
    /// node serves, so an overlay that breaks a Baseline guarantee (drops the
    /// attribution keys, smuggles in a forbidden construct, raises a pinned cap
    /// past the override factor) must be caught here, at admission, regardless of
    /// which node it lands on — never discovered only when a node NACKs it. Each
    /// GatewaySet is admitted by stamping it onto the base with a REPRESENTATIVE
    /// label set that satisfies its own selector, so the check is a pure function
    /// of the repo (no live node labels needed) and deterministic in CI.
    pub fn admit_resolved(
        &self,
        resolved: &crate::source::ResolvedRepo,
    ) -> Result<Verdict, AdmitError> {
        // Fleet-wide render: the base every non-matching node serves.
        let rendered = crate::render::render_resolved(resolved)?;
        let flat = String::from_utf8(rendered.config_bytes)
            .map_err(|e| AdmitError::Unrenderable(format!("rendered config not utf-8: {e}")))?;
        let mut failures = match self.admit_yaml(&flat)? {
            Verdict::Admit => Vec::new(),
            Verdict::Block(f) => f,
        };

        // Each GatewaySet's stamped variant, under a representative matching
        // label set. A failure is attributed to the GatewaySet by name.
        let gatewaysets = crate::render::read_gatewaysets(resolved)
            .map_err(|e| AdmitError::Unrenderable(e.to_string()))?;
        for set in &gatewaysets.sets {
            let labels = representative_labels(&set.selector);
            let stamped = crate::render::render_resolved_for_node(resolved, &gatewaysets, &labels)?;
            let stamped_flat = String::from_utf8(stamped.config_bytes).map_err(|e| {
                AdmitError::Unrenderable(format!("gatewayset {:?} render not utf-8: {e}", set.name))
            })?;
            if let Verdict::Block(fs) = self.admit_yaml(&stamped_flat)? {
                for mut f in fs {
                    f.detail = format!("via GatewaySet {:?}: {}", set.name, f.detail);
                    failures.push(f);
                }
            }
        }

        // The config-canary policy (`canary.yaml`) is reviewed config too: a
        // malformed or nonsensical policy (a non-positive factor, an out-of-range
        // error-rate increase) is BLOCKED at admission, not discovered mid-
        // rollout when the analysis would run. Parsing validates the thresholds
        // (see `canary::parse_canary_policy`), so a parse error IS the admission
        // failure. Absent file → default policy (analysis off) → admits.
        if let Some(bytes) = resolved.get("canary.yaml") {
            match std::str::from_utf8(bytes) {
                Ok(text) => {
                    if let Err(e) = crate::canary::parse_canary_policy(text) {
                        failures.push(RuleFailure {
                            rule: "canary-policy".to_string(),
                            detail: e.to_string(),
                        });
                    }
                }
                Err(e) => failures.push(RuleFailure {
                    rule: "canary-policy".to_string(),
                    detail: format!("canary.yaml is not utf-8: {e}"),
                }),
            }
        }

        Ok(if failures.is_empty() {
            Verdict::Admit
        } else {
            Verdict::Block(failures)
        })
    }
}

/// A representative label map that satisfies `selector`: for each term, the
/// first accepted value. An empty selector (matches everything) yields empty
/// labels, which every render treats as the fleet-wide base plus the empty-
/// selector GatewaySet. This lets admission render a GatewaySet's stamped effect
/// without any live node — the overlay's effect on the Baseline is the same for
/// every node the selector matches.
fn representative_labels(
    selector: &crate::waves::Selector,
) -> std::collections::BTreeMap<String, String> {
    let mut labels = std::collections::BTreeMap::new();
    for term in &selector.terms {
        if let Some(first) = term.in_values.first() {
            labels.insert(term.label.clone(), first.clone());
        }
    }
    labels
}

/// The built-in admission gates — the Baseline guarantees the gateway must
/// never ship without, plus the docs/07 construct/override bans.
pub mod builtin {
    use super::*;

    /// GB-1: every route must enforce at least one attribution key. A route
    /// with an empty *composed* required-key set forwards caller traffic with
    /// no ownership contract — exactly the attribution hole GB-1 closes.
    pub fn attribution_keys_present(cfg: &Config) -> Vec<RuleFailure> {
        let mut out = Vec::new();
        for route in &cfg.routes {
            // The COMPOSED policy is what actually runs; a route can inherit its
            // required keys from fleet/project, so we check the effective set,
            // not the route's own inline list.
            if route.policy().required_keys.is_empty() {
                out.push(RuleFailure {
                    rule: "GB-1".to_string(),
                    detail: format!(
                        "route {:?} has no effective attribution required_keys — every route must \
                         enforce at least one attribution key (GB-1)",
                        route.prefix
                    ),
                });
            }
        }
        out
    }

    /// GB-4: both mandatory rejection templates must be present at fleet scope.
    /// (The `Config` type already makes them mandatory to parse; this gate
    /// additionally rejects an EMPTY body, which would ship a blank 4xx — the
    /// invent-your-own-body failure GB-4 forbids.)
    pub fn rejection_templates_present(cfg: &Config) -> Vec<RuleFailure> {
        let mut out = Vec::new();
        if cfg.rejections.default_response.body.trim().is_empty() {
            out.push(RuleFailure {
                rule: "GB-4".to_string(),
                detail: "rejections.default_response.body is empty — the operator must own \
                         the rejection body (GB-4), never ship a blank 4xx"
                    .to_string(),
            });
        }
        if cfg.rejections.unknown_route.body.trim().is_empty() {
            out.push(RuleFailure {
                rule: "GB-4".to_string(),
                detail: "rejections.unknown_route.body is empty — the operator must own the \
                         rejection body (GB-4)"
                    .to_string(),
            });
        }
        out
    }

    /// A forbidden-construct gate. docs/07 bans in-gateway templating precisely
    /// so the diff a human reviews is the diff the fleet runs; a candidate that
    /// smuggles a `{{ ... }}` templating directive OUTSIDE the two known
    /// rejection-body placeholders (`{{key}}`, `{{route}}`) is an attempt to
    /// move behavior out of review and into runtime. It is blocked at admission.
    pub fn no_forbidden_construct(flat_yaml: &str) -> Vec<RuleFailure> {
        const ALLOWED: [&str; 2] = ["{{key}}", "{{route}}"];
        let mut out = Vec::new();
        let mut rest = flat_yaml;
        while let Some(pos) = rest.find("{{") {
            let after = &rest[pos..];
            let end = after.find("}}").map(|e| e + 2).unwrap_or(after.len());
            let directive = &after[..end];
            // Normalize inner whitespace for the allow-list comparison.
            let normalized = format!(
                "{{{{{}}}}}",
                directive
                    .trim_start_matches("{{")
                    .trim_end_matches("}}")
                    .trim()
            );
            if !ALLOWED.contains(&normalized.as_str()) {
                out.push(RuleFailure {
                    rule: "no-forbidden-construct".to_string(),
                    detail: format!(
                        "candidate contains a templating directive {directive:?} outside the \
                         allowed rejection-body placeholders {ALLOWED:?} — in-gateway templating \
                         is banned so the reviewed diff is the served diff (docs/07)"
                    ),
                });
                break; // one is enough to block; naming the first is precise.
            }
            rest = &after[end..];
        }
        out
    }

    /// Override governance: an app override that raises a numeric pinned cap
    /// beyond `factor`x the route/fleet baseline must carry a justifying label
    /// ([`OVERRIDE_JUSTIFICATION_LABEL`]). A silent 10x bump to one app's TPM cap
    /// is the unreviewed blast-radius change this catches; adding the label makes
    /// the intent explicit in the reviewed diff.
    pub fn override_governance(cfg: &Config, factor: f64) -> Vec<RuleFailure> {
        let mut out = Vec::new();
        let Some(apps) = &cfg.apps else {
            return out;
        };
        // The baseline a per-app override is measured against: the fleet-scope
        // pinned numerics (the widest default). Absent a fleet baseline for a
        // key, an override introduces a NEW cap and is not a "raise", so it is
        // exempt from the factor gate (there is nothing to multiply).
        let baseline = fleet_numeric_pins(cfg);
        for (app_value, scope) in &apps.overrides {
            let has_label = scope_has_label(scope, OVERRIDE_JUSTIFICATION_LABEL);
            for (key, raw) in &scope.attribution.pinned {
                let Ok(new_val) = raw.parse::<f64>() else {
                    continue; // non-numeric pins are not caps; skip.
                };
                let Some(&base) = baseline.get(key.as_str()) else {
                    continue; // no baseline to compare against.
                };
                if base > 0.0 && new_val > base * factor && !has_label {
                    out.push(RuleFailure {
                        rule: "override-factor".to_string(),
                        detail: format!(
                            "app {app_value:?} overrides pinned {key:?} to {new_val} — more than \
                             {factor}x the fleet baseline {base} — without the \
                             {OVERRIDE_JUSTIFICATION_LABEL:?} label; a blast-radius override must \
                             be labeled to pass admission"
                        ),
                    });
                }
            }
        }
        out
    }

    /// "No unsigned WASM module" (docs/02 admission slot; docs/04 Phase 4).
    /// Every declared tier-2 module MUST carry a signature — a module with an
    /// absent or blank signature is BLOCKED at admission, before it can be
    /// rendered into a snapshot. Cryptographic MATCH (the signature actually
    /// verifying against the operator key over the module bytes) is checked by
    /// the data-plane loader at config load (`gateway_wasm::sig::verify`),
    /// which holds the key and the bytes; admission owns the presence gate,
    /// which is a pure function of the candidate and thus deterministic in CI.
    pub fn no_unsigned_wasm_module(cfg: &Config) -> Vec<RuleFailure> {
        let mut out = Vec::new();
        for module in &cfg.wasm.modules {
            let signed = module
                .signature
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty());
            if !signed {
                out.push(RuleFailure {
                    rule: "no-unsigned-wasm-module".to_string(),
                    detail: format!(
                        "wasm module {:?} carries no signature — unsigned WASM modules are \
                         rejected at admission; sign it with the operator key (docs/02)",
                        module.name
                    ),
                });
            }
        }
        out
    }

    /// The fleet-scope numeric pinned values, the baseline overrides multiply.
    fn fleet_numeric_pins(cfg: &Config) -> std::collections::BTreeMap<String, f64> {
        let mut out = std::collections::BTreeMap::new();
        if let Some(fleet) = &cfg.fleet {
            for (k, v) in &fleet.attribution.pinned {
                if let Ok(n) = v.parse::<f64>() {
                    out.insert(k.clone(), n);
                }
            }
        }
        out
    }

    /// Whether a scope's GB-8 label list carries a label with the given key.
    fn scope_has_label(scope: &Scope, key: &str) -> bool {
        scope.labels.iter().any(|entry| match entry {
            LabelEntry::Spec(spec) => spec.key == key,
            LabelEntry::Base(_) => false,
        })
    }
}

/// Convert the flat candidate YAML into a `serde_json::Value` for CEL. The CEL
/// interpreter takes JSON-shaped values; YAML maps/sequences/scalars round-trip
/// cleanly. A parse failure yields an empty object, which makes any `config.*`
/// reference in a rule evaluate to absent (and thus the rule blocks loudly).
fn candidate_to_json(flat_yaml: &str) -> serde_json::Value {
    serde_yaml::from_str::<serde_json::Value>(flat_yaml)
        .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()))
}

/// Evaluate one CEL rule against the candidate document. Returns `Some(failure)`
/// when the rule does NOT admit (false / non-bool / eval error / compile error);
/// `None` when it admits. A rule that will not even compile is itself a block —
/// a broken admission rule must never silently pass a candidate.
fn eval_cel_rule(rule: &CelRule, doc: &serde_json::Value) -> Option<RuleFailure> {
    use cel::{Context, Program};

    let program = match Program::compile(&rule.expr) {
        Ok(p) => p,
        Err(e) => {
            return Some(RuleFailure {
                rule: rule.id.clone(),
                detail: format!("admission rule expression failed to compile ({e}): {}", rule.expr),
            });
        }
    };
    let mut ctx = Context::default();
    if let Err(e) = ctx.add_variable("config", doc.clone()) {
        return Some(RuleFailure {
            rule: rule.id.clone(),
            detail: format!("admission rule context build error: {e}"),
        });
    }
    match program.execute(&ctx) {
        Ok(cel::Value::Bool(true)) => None,
        Ok(cel::Value::Bool(false)) => Some(RuleFailure {
            rule: rule.id.clone(),
            detail: rule.message.clone(),
        }),
        Ok(other) => Some(RuleFailure {
            rule: rule.id.clone(),
            detail: format!(
                "{} (rule produced {:?}, expected a bool — a non-bool admission rule is treated \
                 as a block)",
                rule.message,
                other.type_of()
            ),
        }),
        Err(e) => Some(RuleFailure {
            rule: rule.id.clone(),
            detail: format!("{} (rule eval error: {e})", rule.message),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal admitted candidate: one route with a required key, both
    /// rejection bodies non-empty, no forbidden construct.
    fn good_flat() -> String {
        r#"
providers:
  openai-main:
    kind: openai
    upstream: { host: 127.0.0.1, port: 6190 }
fleet:
  attribution:
    required_keys: [team]
    headers: { team: x-attr-team }
routes:
  - prefix: /openai
    provider: openai-main
rejections:
  default_response:
    status: 428
    content_type: application/json
    body: '{"error":"missing {{key}} on {{route}}"}'
  unknown_route:
    status: 404
    content_type: application/json
    body: '{"error":"no route for {{route}}"}'
"#
        .to_string()
    }

    #[test]
    fn a_clean_candidate_is_admitted() {
        let policy = AdmissionPolicy::new();
        assert_eq!(policy.admit_yaml(&good_flat()).unwrap(), Verdict::Admit);
    }

    #[test]
    fn gb1_blocks_a_route_with_no_attribution_key() {
        // Remove the fleet required_keys AND give the route none: the composed
        // required-key set is empty -> GB-1 block.
        let flat = good_flat().replace("    required_keys: [team]\n", "");
        let verdict = AdmissionPolicy::new().admit_yaml(&flat).unwrap();
        let failures = verdict.failures();
        assert!(
            failures.iter().any(|f| f.rule == "GB-1"),
            "expected a GB-1 failure, got {failures:?}"
        );
    }

    #[test]
    fn gb4_blocks_an_empty_rejection_body() {
        let flat = good_flat().replace(
            r#"    body: '{"error":"no route for {{route}}"}'"#,
            r#"    body: ''"#,
        );
        let verdict = AdmissionPolicy::new().admit_yaml(&flat).unwrap();
        assert!(
            verdict.failures().iter().any(|f| f.rule == "GB-4"),
            "expected a GB-4 failure, got {:?}",
            verdict.failures()
        );
    }

    #[test]
    fn a_forbidden_templating_construct_is_blocked() {
        // Inject a templating directive into a place gateway-core does NOT
        // itself placeholder-validate — a pinned attribution value. This is a
        // valid Config (it renders), but the `{{ env.SECRET }}` is an
        // in-gateway-templating escape hatch admission must block: the effect
        // of a runtime-expanded value never appears in the reviewed diff.
        let flat = good_flat().replace(
            "  attribution:\n    required_keys: [team]",
            "  attribution:\n    required_keys: [team]\n    pinned: { region: \"{{ env.SECRET }}\" }",
        );
        // Sanity: this candidate is a VALID config (renders fine); the block
        // comes from admission, not from render-time validation.
        assert!(parse_rendered(flat.as_bytes()).is_ok(), "candidate must be valid config");
        let verdict = AdmissionPolicy::new().admit_yaml(&flat).unwrap();
        assert!(
            verdict
                .failures()
                .iter()
                .any(|f| f.rule == "no-forbidden-construct"),
            "expected a forbidden-construct failure, got {:?}",
            verdict.failures()
        );
    }

    #[test]
    fn the_two_allowed_placeholders_do_not_trip_the_forbidden_gate() {
        // good_flat uses {{key}} and {{route}} in its bodies; it must admit.
        assert!(AdmissionPolicy::new().admit_yaml(&good_flat()).unwrap().is_admitted());
    }

    /// An over-factor app override without the justification label is blocked;
    /// adding the label admits it.
    fn flat_with_override(app_cap: &str, labeled: bool) -> String {
        // The justification label sits at the app-override SCOPE level (a
        // sibling of `attribution`), matching gateway-core's Scope shape.
        let label = if labeled {
            "\n      labels:\n        - { key: override-approved, value: \"yes\" }"
        } else {
            ""
        };
        format!(
            r#"
providers:
  openai-main:
    kind: openai
    upstream: {{ host: 127.0.0.1, port: 6190 }}
fleet:
  attribution:
    required_keys: [team]
    headers: {{ team: x-attr-team }}
    pinned: {{ tpm_cap: "100" }}
apps:
  key: team
  overrides:
    whale:
      attribution:
        pinned: {{ tpm_cap: "{app_cap}" }}{label}
routes:
  - prefix: /openai
    provider: openai-main
rejections:
  default_response:
    status: 428
    content_type: application/json
    body: '{{"error":"missing {{{{key}}}} on {{{{route}}}}"}}'
  unknown_route:
    status: 404
    content_type: application/json
    body: '{{"error":"no route for {{{{route}}}}"}}'
"#
        )
    }

    #[test]
    fn an_over_factor_override_without_a_label_is_blocked() {
        // baseline 100, factor 4 -> anything over 400 needs a label. 1000 does.
        let flat = flat_with_override("1000", false);
        let verdict = AdmissionPolicy::new().admit_yaml(&flat).unwrap();
        assert!(
            verdict.failures().iter().any(|f| f.rule == "override-factor"),
            "expected an override-factor failure, got {:?}",
            verdict.failures()
        );
    }

    #[test]
    fn an_over_factor_override_with_the_label_is_admitted() {
        let flat = flat_with_override("1000", true);
        assert!(
            AdmissionPolicy::new().admit_yaml(&flat).unwrap().is_admitted(),
            "the justifying label admits the override"
        );
    }

    #[test]
    fn a_within_factor_override_needs_no_label() {
        // 300 is within 4x of 100 -> no label required.
        let flat = flat_with_override("300", false);
        assert!(AdmissionPolicy::new().admit_yaml(&flat).unwrap().is_admitted());
    }

    #[test]
    fn a_cel_rule_can_admit_and_reject() {
        // A rule requiring at least one route: good_flat has one -> admit.
        let policy = AdmissionPolicy::new().with_cel_rule(
            "has-a-route",
            "size(config.routes) > 0",
            "the candidate must define at least one route",
        );
        assert!(policy.admit_yaml(&good_flat()).unwrap().is_admitted());

        // A rule that will not hold: require MORE than one route -> block.
        let strict = AdmissionPolicy::new().with_cel_rule(
            "needs-two-routes",
            "size(config.routes) > 1",
            "the candidate must define more than one route",
        );
        let verdict = strict.admit_yaml(&good_flat()).unwrap();
        assert!(
            verdict.failures().iter().any(|f| f.rule == "needs-two-routes"),
            "got {:?}",
            verdict.failures()
        );
    }

    // The "no unsigned WASM module" admission rule is exercised end to end in
    // `tests/admission_wasm.rs` (an integration test, to keep this file
    // focused); the built-in itself lives in `builtin::no_unsigned_wasm_module`.

    #[test]
    fn a_broken_cel_rule_blocks_rather_than_silently_passing() {
        let policy = AdmissionPolicy::new().with_cel_rule(
            "broken",
            "this is not (valid cel",
            "unused",
        );
        let verdict = policy.admit_yaml(&good_flat()).unwrap();
        assert!(
            verdict.failures().iter().any(|f| f.rule == "broken"),
            "a rule that will not compile must block, got {:?}",
            verdict.failures()
        );
    }

    #[test]
    fn all_failures_are_collected_not_just_the_first() {
        // Empty rejection body (GB-4) AND no attribution key (GB-1) at once.
        let flat = good_flat()
            .replace("    required_keys: [team]\n", "")
            .replace(
                r#"    body: '{"error":"no route for {{route}}"}'"#,
                r#"    body: ''"#,
            );
        let verdict = AdmissionPolicy::new().admit_yaml(&flat).unwrap();
        let rules: Vec<&str> = verdict.failures().iter().map(|f| f.rule.as_str()).collect();
        assert!(rules.contains(&"GB-1"), "{rules:?}");
        assert!(rules.contains(&"GB-4"), "{rules:?}");
    }

    // --- GatewaySet admission (docs/02: a GatewaySet is admitted like config) ---

    /// Write a base repo (env=prod, valid) plus a `gatewaysets.yaml` with `body`.
    fn repo_with_gatewaysets(gatewaysets_yaml: &str) -> crate::source::ResolvedRepo {
        use crate::source::{ConfigSource, DirectorySource};
        let root = crate::render::testrepo::write("prod");
        std::fs::write(root.join("gatewaysets.yaml"), gatewaysets_yaml).unwrap();
        DirectorySource::new(&root).resolve().unwrap()
    }

    #[test]
    fn a_benign_gatewayset_is_admitted() {
        // A GatewaySet that only stamps a harmless extra pin admits — it does not
        // break any Baseline guarantee.
        let resolved = repo_with_gatewaysets(
            "\
gatewaysets:
  - name: eu-tier
    selector: { region: eu }
    overlay:
      fleet:
        attribution:
          pinned: { tier: gold }
",
        );
        let verdict = AdmissionPolicy::new().admit_resolved(&resolved).unwrap();
        assert_eq!(verdict, Verdict::Admit, "{:?}", verdict.failures());
    }

    #[test]
    fn a_gatewayset_that_smuggles_a_forbidden_construct_is_blocked() {
        // The fleet-wide render is clean, but a GatewaySet overlay introduces a
        // banned in-gateway-templating construct. Admission must catch it on the
        // stamped variant (docs/07 bans the construct precisely so the reviewed
        // diff is the served diff), attributed to the GatewaySet by name — never
        // discovered only when a matching node NACKs.
        let resolved = repo_with_gatewaysets(
            "\
gatewaysets:
  - name: sneaky-eu
    selector: { region: eu }
    overlay:
      fleet:
        attribution:
          pinned: { greeting: \"{{ request.headers.x }}\" }
",
        );
        let verdict = AdmissionPolicy::new().admit_resolved(&resolved).unwrap();
        assert!(!verdict.is_admitted(), "the forbidden construct must block");
        assert!(
            verdict
                .failures()
                .iter()
                .any(|f| f.detail.contains("GatewaySet \"sneaky-eu\"")),
            "the failure names the offending GatewaySet: {:?}",
            verdict.failures()
        );
    }
}
