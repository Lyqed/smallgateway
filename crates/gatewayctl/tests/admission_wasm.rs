//! The "no unsigned WASM module" admission rule (docs/02 admission slot;
//! docs/04 Phase 4), end to end. Every declared tier-2 module must carry a
//! signature; an unsigned one is blocked at admission before it can be
//! rendered into a snapshot. The cryptographic MATCH is the data plane's
//! load-time check (`gateway_wasm::sig::verify`); admission owns the presence
//! gate, a pure function of the candidate.

use gatewayctl::admission::AdmissionPolicy;

/// A flat candidate config declaring a WASM module, signed or not.
fn flat_with_wasm(signature: &str) -> String {
    let sig_line = if signature.is_empty() {
        String::new()
    } else {
        format!("\n      signature: \"{signature}\"")
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
wasm:
  per_event_hooks: false
  modules:
    - name: header-policy
      source: modules/header-policy.wat
      hooks: [on_request]{sig_line}
routes:
  - prefix: /openai
    provider: openai-main
rejections:
  missing_attribution:
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
fn an_unsigned_wasm_module_is_blocked_at_admission() {
    let flat = flat_with_wasm("");
    let verdict = AdmissionPolicy::new().admit_yaml(&flat).unwrap();
    assert!(
        verdict
            .failures()
            .iter()
            .any(|f| f.rule == "no-unsigned-wasm-module"),
        "expected a no-unsigned-wasm-module failure, got {:?}",
        verdict.failures()
    );
}

#[test]
fn a_signed_wasm_module_is_admitted() {
    // Any non-empty signature passes the PRESENCE gate at admission; the
    // cryptographic match is the data plane's load-time check.
    let flat = flat_with_wasm("deadbeefcafe");
    let verdict = AdmissionPolicy::new().admit_yaml(&flat).unwrap();
    assert!(
        !verdict
            .failures()
            .iter()
            .any(|f| f.rule == "no-unsigned-wasm-module"),
        "a signed module must pass the presence gate, got {:?}",
        verdict.failures()
    );
}
