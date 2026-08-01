//! GB-8 at request time: resolve the composed Vertex billing labels and
//! merge them into the outbound `generateContent` body.
//!
//! These are the same semantics we wrote for upstream agentgateway
//! (`upstream/agentgateway/gb8-vertex-operator-labels`), native here:
//!
//! - label values are static, attribution-derived (`from_attribution`
//!   references a RESOLVED attribution key), or CEL expressions over
//!   request + jwt + attribution;
//! - operator labels merge OVER client-sent body labels — on a key
//!   conflict the operator's value wins, so callers cannot override the
//!   gateway's cost attribution; client labels without conflicts pass
//!   through unchanged;
//! - fail closed: a label whose value cannot be resolved (absent
//!   attribution key, erroring expression, or a value Google Cloud would
//!   reject) is an error the proxy turns into the route's effective GB-4
//!   `missing_attribution` rejection — the request never reaches the
//!   provider unattributed.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::expr::EvalCtx;
use crate::scope::{validate_label_value, EffectiveLabel, LabelValue};

/// A label that could not be resolved: `key` names the label (for the
/// GB-4 template's `{{key}}`), `reason` is the log-side detail.
#[derive(Debug, PartialEq, Eq)]
pub struct LabelError {
    pub key: String,
    pub reason: String,
}

impl std::fmt::Display for LabelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "label {:?}: {}", self.key, self.reason)
    }
}

/// Resolve the composed labels against the resolved attribution and the
/// request context. Returns key → value pairs in composed order (a `Vec`
/// keeps the order deterministic; [`merge_into_body`] builds the JSON map).
pub fn resolve(
    labels: &[EffectiveLabel],
    attribution: &BTreeMap<String, String>,
    ctx: &EvalCtx,
) -> Result<Vec<(String, String)>, LabelError> {
    let fail = |key: &str, reason: String| LabelError { key: key.to_string(), reason };
    let mut out = Vec::with_capacity(labels.len());
    for label in labels {
        let value = match &label.value {
            LabelValue::Static(v) => v.clone(),
            LabelValue::FromAttribution(key) => {
                attribution.get(key).cloned().ok_or_else(|| {
                    fail(
                        &label.key,
                        format!("attribution key {key:?} did not resolve on this request"),
                    )
                })?
            }
            LabelValue::Expr(expr) => expr
                .eval_string(ctx)
                .map_err(|e| fail(&label.key, e))?,
        };
        // Static values were validated at config load; attribution-derived
        // and CEL values are validated per request — fail closed, exactly
        // like the AWS session-tag posture.
        validate_label_value(&value).map_err(|e| fail(&label.key, e))?;
        out.push((label.key.clone(), value));
    }
    Ok(out)
}

/// Merge operator labels into a `generateContent` request body. The body
/// must be a JSON object (Vertex would reject anything else before any
/// spend occurs); client `labels` entries are preserved unless the
/// operator's key conflicts — the operator wins.
pub fn merge_into_body(body: &[u8], operator: &[(String, String)]) -> Result<Vec<u8>, String> {
    if operator.is_empty() {
        return Ok(body.to_vec());
    }
    let mut root: Value = serde_json::from_slice(body)
        .map_err(|e| format!("request body is not valid JSON: {e}"))?;
    let Some(obj) = root.as_object_mut() else {
        return Err("request body is not a JSON object".to_string());
    };
    let labels = obj
        .entry("labels")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(labels) = labels.as_object_mut() else {
        // A client sent `"labels": [...]` or a scalar: replace it — the
        // operator's attribution cannot be blocked by a malformed field.
        *labels = Value::Object(
            operator
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect(),
        );
        return serde_json::to_vec(&root).map_err(|e| e.to_string());
    };
    for (key, value) in operator {
        labels.insert(key.clone(), Value::String(value.clone()));
    }
    serde_json::to_vec(&root).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::expr::{CompiledExpr, ExprKind};

    fn eff(key: &str, value: LabelValue) -> EffectiveLabel {
        EffectiveLabel { key: key.to_string(), value }
    }

    fn attribution(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn ctx(attr: &BTreeMap<String, String>) -> EvalCtx {
        EvalCtx {
            method: "POST".to_string(),
            path: "/vertex/v1/x".to_string(),
            attribution: attr.clone(),
            ..EvalCtx::default()
        }
    }

    #[test]
    fn static_attribution_and_expression_labels_resolve_in_order() {
        let attr = attribution(&[("team", "ml-research")]);
        let labels = vec![
            eff("cost_center", LabelValue::Static("platform".to_string())),
            eff("team", LabelValue::FromAttribution("team".to_string())),
            eff(
                "tenant",
                LabelValue::Expr(Arc::new(
                    CompiledExpr::compile(r#"attribution["team"] + "-gw""#, ExprKind::Label)
                        .unwrap(),
                )),
            ),
        ];
        let resolved = resolve(&labels, &attr, &ctx(&attr)).unwrap();
        assert_eq!(
            resolved,
            vec![
                ("cost_center".to_string(), "platform".to_string()),
                ("team".to_string(), "ml-research".to_string()),
                ("tenant".to_string(), "ml-research-gw".to_string()),
            ]
        );
    }

    #[test]
    fn unresolvable_attribution_reference_fails_closed() {
        let attr = attribution(&[]);
        let labels = vec![eff("team", LabelValue::FromAttribution("team".to_string()))];
        let err = resolve(&labels, &attr, &ctx(&attr)).unwrap_err();
        assert_eq!(err.key, "team");
        assert!(err.reason.contains("did not resolve"), "{err}");
    }

    #[test]
    fn erroring_expression_fails_closed() {
        let attr = attribution(&[]);
        let labels = vec![eff(
            "tenant",
            LabelValue::Expr(Arc::new(
                CompiledExpr::compile(r#"request.headers["x-absent"]"#, ExprKind::Label).unwrap(),
            )),
        )];
        assert!(resolve(&labels, &attr, &ctx(&attr)).is_err());
    }

    #[test]
    fn value_google_would_reject_fails_closed() {
        // Attribution values are a wider charset than Google labels — the
        // per-request validation is what keeps the invoice clean.
        let attr = attribution(&[("team", "ML Research!")]);
        let labels = vec![eff("team", LabelValue::FromAttribution("team".to_string()))];
        let err = resolve(&labels, &attr, &ctx(&attr)).unwrap_err();
        assert!(err.reason.contains("Google Cloud"), "{err}");
    }

    #[test]
    fn merge_adds_labels_to_a_body_without_them() {
        let body = br#"{"contents":[{"parts":[{"text":"hi"}]}]}"#;
        let merged = merge_into_body(body, &[("team".to_string(), "ml".to_string())]).unwrap();
        let v: Value = serde_json::from_slice(&merged).unwrap();
        assert_eq!(v["labels"]["team"], "ml");
        assert_eq!(v["contents"][0]["parts"][0]["text"], "hi");
    }

    #[test]
    fn operator_wins_on_conflict_and_client_labels_survive_otherwise() {
        let body = br#"{"contents":[],"labels":{"team":"client-spoofed","env":"client"}}"#;
        let merged = merge_into_body(body, &[("team".to_string(), "ml".to_string())]).unwrap();
        let v: Value = serde_json::from_slice(&merged).unwrap();
        assert_eq!(v["labels"]["team"], "ml", "operator wins on conflict");
        assert_eq!(v["labels"]["env"], "client", "client label without conflict preserved");
    }

    #[test]
    fn malformed_client_labels_field_is_replaced_not_obeyed() {
        let body = br#"{"contents":[],"labels":[1,2]}"#;
        let merged = merge_into_body(body, &[("team".to_string(), "ml".to_string())]).unwrap();
        let v: Value = serde_json::from_slice(&merged).unwrap();
        assert_eq!(v["labels"]["team"], "ml");
    }

    #[test]
    fn non_object_body_is_an_error() {
        assert!(merge_into_body(b"[]", &[("k".to_string(), "v".to_string())]).is_err());
        assert!(merge_into_body(b"not json", &[("k".to_string(), "v".to_string())]).is_err());
    }

    #[test]
    fn empty_operator_set_leaves_the_body_untouched() {
        let body = br#"{"labels":{"team":"client"}}"#;
        assert_eq!(merge_into_body(body, &[]).unwrap(), body.to_vec());
    }
}
