//! Attribution tag resolution (GB-1/2/3), pure and proxy-free.
//!
//! Every tag on a forwarded request carries an origin:
//!
//! - **assigned** — operator-pinned (GB-3); the gateway sets the value and a
//!   caller-sent value is overwritten, never believed;
//! - **proven** — mapped from a verified JWT claim (GB-2); a caller header
//!   for a claim-mapped key is likewise never believed;
//! - **caller** — sent by the caller for a plain required key (GB-1: the
//!   Baseline enforces presence, the value is the caller's assertion).
//!
//! The proxy binds this to `x-attr-<key>` headers; this module only decides.

use serde_json::Value;

use crate::config::Attribution;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Assigned,
    Proven,
    Caller,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Assigned => "assigned",
            Origin::Proven => "proven",
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

/// Resolve a route's attribution contract against what the caller sent and
/// what a verified token proved.
///
/// Returns the tags sorted by key (deterministic logs), or — when GB-1 is
/// unmet — the sorted list of required keys nothing satisfied.
pub fn resolve(
    attr: &Attribution,
    caller_value: impl Fn(&str) -> Option<String>,
    claims: Option<&serde_json::Map<String, Value>>,
) -> Result<Vec<Tag>, Vec<String>> {
    let mut keys: Vec<&str> = attr
        .required_keys
        .iter()
        .map(String::as_str)
        .chain(attr.pinned.keys().map(String::as_str))
        .chain(attr.from_claims.keys().map(String::as_str))
        .collect();
    keys.sort_unstable();
    keys.dedup();

    let mut tags = Vec::new();
    let mut missing = Vec::new();
    for key in keys {
        let required = attr.required_keys.iter().any(|k| k == key);
        if let Some(value) = attr.pinned.get(key) {
            tags.push(Tag {
                key: key.to_string(),
                value: value.clone(),
                origin: Origin::Assigned,
            });
        } else if let Some(claim) = attr.from_claims.get(key) {
            // Claim-mapped: proven or absent. Deliberately no caller
            // fallback — "proven or assigned, never believed".
            match claims.and_then(|c| c.get(claim)).and_then(claim_string) {
                Some(value) => tags.push(Tag {
                    key: key.to_string(),
                    value,
                    origin: Origin::Proven,
                }),
                None if required => missing.push(key.to_string()),
                None => {}
            }
        } else {
            match caller_value(key).filter(|v| !v.is_empty()) {
                Some(value) => tags.push(Tag {
                    key: key.to_string(),
                    value,
                    origin: Origin::Caller,
                }),
                None if required => missing.push(key.to_string()),
                None => {}
            }
        }
    }

    if missing.is_empty() {
        Ok(tags)
    } else {
        Err(missing)
    }
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

    fn attr(required: &[&str], pinned: &[(&str, &str)], claims: &[(&str, &str)]) -> Attribution {
        Attribution {
            required_keys: required.iter().map(|s| s.to_string()).collect(),
            pinned: pinned
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            from_claims: claims
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn caller(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn lookup(map: &BTreeMap<String, String>) -> impl Fn(&str) -> Option<String> + '_ {
        move |k| map.get(k).cloned()
    }

    #[test]
    fn pinned_overrides_caller_value() {
        let a = attr(&[], &[("env", "prod")], &[]);
        let c = caller(&[("env", "not-prod")]);
        let tags = resolve(&a, lookup(&c), None).unwrap();
        assert_eq!(tags, vec![Tag { key: "env".into(), value: "prod".into(), origin: Origin::Assigned }]);
    }

    #[test]
    fn missing_required_keys_are_reported_sorted() {
        let a = attr(&["team", "app"], &[], &[]);
        let c = caller(&[]);
        let missing = resolve(&a, lookup(&c), None).unwrap_err();
        assert_eq!(missing, vec!["app", "team"]);
    }

    #[test]
    fn empty_caller_value_counts_as_missing() {
        let a = attr(&["team"], &[], &[]);
        let c = caller(&[("team", "")]);
        assert_eq!(resolve(&a, lookup(&c), None).unwrap_err(), vec!["team"]);
    }

    #[test]
    fn required_key_satisfied_by_pin_needs_no_caller_header() {
        let a = attr(&["env"], &[("env", "prod")], &[]);
        let c = caller(&[]);
        let tags = resolve(&a, lookup(&c), None).unwrap();
        assert_eq!(tags[0].origin, Origin::Assigned);
    }

    #[test]
    fn claim_mapped_key_is_proven_and_never_believed_from_caller() {
        let a = attr(&["user"], &[], &[("user", "sub")]);
        let forged = caller(&[("user", "mallory")]);

        // No verified claims: the forged caller header does not count.
        assert_eq!(resolve(&a, lookup(&forged), None).unwrap_err(), vec!["user"]);

        // Verified claims win, caller header ignored.
        let claims = serde_json::json!({ "sub": "alice", "exp": 4102444800u64 });
        let tags = resolve(&a, lookup(&forged), claims.as_object()).unwrap();
        assert_eq!(
            tags,
            vec![Tag { key: "user".into(), value: "alice".into(), origin: Origin::Proven }]
        );
    }

    #[test]
    fn numeric_claims_stringify_and_structured_claims_do_not_count() {
        let a = attr(&[], &[], &[("org", "org_id"), ("bad", "nested")]);
        let claims = serde_json::json!({ "org_id": 42, "nested": {"x": 1} });
        let tags = resolve(&a, |_| None, claims.as_object()).unwrap();
        assert_eq!(tags, vec![Tag { key: "org".into(), value: "42".into(), origin: Origin::Proven }]);
    }

    #[test]
    fn mixed_origins_resolve_sorted_by_key() {
        let a = attr(&["team"], &[("env", "prod")], &[("user", "sub")]);
        let c = caller(&[("team", "ml-research")]);
        let claims = serde_json::json!({ "sub": "alice" });
        let tags = resolve(&a, lookup(&c), claims.as_object()).unwrap();
        let keys: Vec<&str> = tags.iter().map(|t| t.key.as_str()).collect();
        assert_eq!(keys, vec!["env", "team", "user"]);
        let origins: Vec<Origin> = tags.iter().map(|t| t.origin).collect();
        assert_eq!(origins, vec![Origin::Assigned, Origin::Caller, Origin::Proven]);
    }
}
