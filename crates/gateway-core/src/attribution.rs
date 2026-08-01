//! Attribution tag resolution (GB-1/2/3 + CEL derivation), pure and
//! proxy-free.
//!
//! Every tag on a forwarded request carries an origin:
//!
//! - **assigned** — operator-pinned (GB-3); the gateway sets the value and a
//!   caller-sent value is overwritten, never believed;
//! - **proven** — mapped from a verified JWT claim (GB-2); a caller header
//!   for a claim-mapped key is likewise never believed;
//! - **derived** — computed by an operator-written CEL expression over the
//!   request + verified claims (tier-1 extensibility); the caller cannot
//!   set it directly, though the operator's expression may choose to read
//!   caller-controlled inputs;
//! - **caller** — sent by the caller for a plain required key (GB-1: the
//!   Baseline enforces presence, the value is the caller's assertion).
//!
//! Resolution runs against a COMPOSED [`EffectivePolicy`] — the scoped
//! chain (fleet → project → route → app) is already flattened by
//! [`crate::scope::finalize`]. The proxy binds this to `x-attr-<key>`
//! headers and evaluates the derived expressions; this module only decides.

use serde_json::Value;

use crate::scope::EffectivePolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Assigned,
    Proven,
    Derived,
    Caller,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Assigned => "assigned",
            Origin::Proven => "proven",
            Origin::Derived => "derived",
            Origin::Caller => "caller",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub key: String,
    pub value: String,
    pub origin: Origin,
}

/// What one resolution pass established: the tags that DID resolve, and
/// the required keys nothing satisfied. Deliberately not a `Result`: the
/// app scope is selected by a RESOLVED value (`apps.key`) possibly while
/// other keys are still missing, and the app override may itself satisfy
/// them (e.g. pin a required key) — so enforcement belongs to the FINAL
/// policy of the chain, not to an intermediate pass. Callers reject when
/// the last pass still reports `missing` (GB-1, fail closed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    /// Sorted by key (deterministic logs).
    pub tags: Vec<Tag>,
    /// Sorted required keys nothing satisfied.
    pub missing: Vec<String>,
}

impl Resolution {
    pub fn ok(&self) -> bool {
        self.missing.is_empty()
    }

    /// The resolved value of one key, if any.
    pub fn value(&self, key: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|t| t.key == key)
            .map(|t| t.value.as_str())
    }
}

/// Resolve a composed attribution contract against what the caller sent,
/// what a verified token proved, and what the operator's derivations
/// computed.
///
/// `derived_value` is the pre-evaluated result of the policy's derived
/// expression for a key (`None` on evaluation failure — the caller logs
/// the error; a required key then reports missing, fail closed).
pub fn resolve(
    policy: &EffectivePolicy,
    caller_value: impl Fn(&str) -> Option<String>,
    claims: Option<&serde_json::Map<String, Value>>,
    derived_value: impl Fn(&str) -> Option<String>,
) -> Resolution {
    let mut keys: Vec<&str> = policy
        .required_keys
        .iter()
        .map(String::as_str)
        .chain(policy.pinned.keys().map(String::as_str))
        .chain(policy.from_claims.keys().map(String::as_str))
        .chain(policy.derived.keys().map(String::as_str))
        .collect();
    keys.sort_unstable();
    keys.dedup();

    let mut tags = Vec::new();
    let mut missing = Vec::new();
    let mut push = |key: &str, value: String, origin: Origin| {
        tags.push(Tag { key: key.to_string(), value, origin });
    };
    for key in keys {
        let required = policy.required_keys.iter().any(|k| k == key);
        if let Some(value) = policy.pinned.get(key) {
            push(key, value.clone(), Origin::Assigned);
        } else if let Some(claim) = policy.from_claims.get(key) {
            // Claim-mapped: proven or absent. Deliberately no caller
            // fallback — "proven or assigned, never believed".
            match claims.and_then(|c| c.get(claim)).and_then(claim_string) {
                Some(value) => push(key, value, Origin::Proven),
                None if required => missing.push(key.to_string()),
                None => {}
            }
        } else if policy.derived.contains_key(key) {
            // Derived: computed or absent. Same no-caller-fallback rule.
            match derived_value(key).filter(|v| !v.is_empty()) {
                Some(value) => push(key, value, Origin::Derived),
                None if required => missing.push(key.to_string()),
                None => {}
            }
        } else {
            match caller_value(key).filter(|v| !v.is_empty()) {
                Some(value) => push(key, value, Origin::Caller),
                None if required => missing.push(key.to_string()),
                None => {}
            }
        }
    }

    Resolution { tags, missing }
}

/// Claim values usable as attribution tags: scalars, stringified. Objects
/// and arrays don't name a spender; treat them as absent.
fn claim_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use crate::expr::{CompiledExpr, ExprKind};
    use crate::scope::EffectivePolicy;

    fn template() -> crate::config::RejectionTemplate {
        crate::config::RejectionTemplate {
            status: 428,
            content_type: "application/json".to_string(),
            body: "{}".to_string(),
            streaming: None,
        }
    }

    fn policy(
        required: &[&str],
        pinned: &[(&str, &str)],
        claims: &[(&str, &str)],
        derived: &[&str],
    ) -> EffectivePolicy {
        EffectivePolicy {
            required_keys: required.iter().map(|s| s.to_string()).collect(),
            pinned: pinned.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            from_claims: claims.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            derived: derived
                .iter()
                .map(|k| {
                    (
                        k.to_string(),
                        Arc::new(CompiledExpr::compile("request.path", ExprKind::Derived).unwrap()),
                    )
                })
                .collect(),
            labels: Vec::new(),
            missing_attribution: template(),
            unknown_route: template(),
        }
    }

    fn caller(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn lookup(map: &BTreeMap<String, String>) -> impl Fn(&str) -> Option<String> + '_ {
        move |k| map.get(k).cloned()
    }

    fn no_derived(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn pinned_overrides_caller_value() {
        let p = policy(&[], &[("env", "prod")], &[], &[]);
        let c = caller(&[("env", "not-prod")]);
        let r = resolve(&p, lookup(&c), None, no_derived);
        assert!(r.ok());
        let tags = r.tags;
        assert_eq!(tags, vec![Tag { key: "env".into(), value: "prod".into(), origin: Origin::Assigned }]);
    }

    #[test]
    fn missing_required_keys_are_reported_sorted() {
        let p = policy(&["team", "app"], &[], &[], &[]);
        let c = caller(&[]);
        let missing = resolve(&p, lookup(&c), None, no_derived).missing;
        assert_eq!(missing, vec!["app", "team"]);
    }

    #[test]
    fn empty_caller_value_counts_as_missing() {
        let p = policy(&["team"], &[], &[], &[]);
        let c = caller(&[("team", "")]);
        assert_eq!(resolve(&p, lookup(&c), None, no_derived).missing, vec!["team"]);
    }

    #[test]
    fn required_key_satisfied_by_pin_needs_no_caller_header() {
        let p = policy(&["env"], &[("env", "prod")], &[], &[]);
        let c = caller(&[]);
        let r = resolve(&p, lookup(&c), None, no_derived);
        assert!(r.ok());
        let tags = r.tags;
        assert_eq!(tags[0].origin, Origin::Assigned);
    }

    #[test]
    fn claim_mapped_key_is_proven_and_never_believed_from_caller() {
        let p = policy(&["user"], &[], &[("user", "sub")], &[]);
        let forged = caller(&[("user", "mallory")]);

        // No verified claims: the forged caller header does not count.
        assert_eq!(
            resolve(&p, lookup(&forged), None, no_derived).missing,
            vec!["user"]
        );

        // Verified claims win, caller header ignored.
        let claims = serde_json::json!({ "sub": "alice", "exp": 4102444800u64 });
        let tags = resolve(&p, lookup(&forged), claims.as_object(), no_derived).tags;
        assert_eq!(
            tags,
            vec![Tag { key: "user".into(), value: "alice".into(), origin: Origin::Proven }]
        );
    }

    #[test]
    fn derived_key_is_never_believed_from_caller_and_fails_closed() {
        let p = policy(&["team"], &[], &[], &["team"]);
        let forged = caller(&[("team", "mallory-team")]);

        // Derivation produced a value: origin is derived, caller ignored.
        let tags = resolve(&p, lookup(&forged), None, |_| Some("ml".to_string())).tags;
        assert_eq!(
            tags,
            vec![Tag { key: "team".into(), value: "ml".into(), origin: Origin::Derived }]
        );

        // Derivation failed (None): required key is MISSING — the forged
        // caller header cannot rescue it. Fail closed.
        assert_eq!(
            resolve(&p, lookup(&forged), None, no_derived).missing,
            vec!["team"]
        );

        // Empty derivation output counts as absent too.
        assert_eq!(
            resolve(&p, lookup(&forged), None, |_| Some(String::new())).missing,
            vec!["team"]
        );
    }

    #[test]
    fn numeric_claims_stringify_and_structured_claims_do_not_count() {
        let p = policy(&[], &[], &[("org", "org_id"), ("bad", "nested")], &[]);
        let claims = serde_json::json!({ "org_id": 42, "nested": {"x": 1} });
        let tags = resolve(&p, |_| None, claims.as_object(), no_derived).tags;
        assert_eq!(tags, vec![Tag { key: "org".into(), value: "42".into(), origin: Origin::Proven }]);
    }

    #[test]
    fn mixed_origins_resolve_sorted_by_key() {
        let p = policy(&["team"], &[("env", "prod")], &[("user", "sub")], &["region"]);
        let c = caller(&[("team", "ml-research")]);
        let claims = serde_json::json!({ "sub": "alice" });
        let tags = resolve(&p, lookup(&c), claims.as_object(), |_| Some("eu".to_string())).tags;
        let keys: Vec<&str> = tags.iter().map(|t| t.key.as_str()).collect();
        assert_eq!(keys, vec!["env", "region", "team", "user"]);
        let origins: Vec<Origin> = tags.iter().map(|t| t.origin).collect();
        assert_eq!(
            origins,
            vec![Origin::Assigned, Origin::Derived, Origin::Caller, Origin::Proven]
        );
    }
}
