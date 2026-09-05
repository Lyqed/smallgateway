//! Wire-level pins for the rejection-shape contract (docs/13): the shapes
//! the GATEWAY owns, exercised against the real binary so an accidental
//! change to a default fails CI instead of a caller's parser. Operator
//! templates are pinned verbatim elsewhere (the gb4 tests); these cover
//! the residuals.

use crate::harness::*;

/// Fleet config whose default_response body uses the optional GB-5
/// placeholders, so the "-" defaults are observable on a non-budget
/// rejection; no streaming block anywhere, so a cut wears the built-in
/// default payload.
fn contract_cfg(mock_port: u16) -> String {
    format!(
        r#"
providers:
  openai-main:
    kind: openai
    upstream: {{ host: 127.0.0.1, port: {mock_port} }}
fleet:
  attribution:
    required_keys: [team]
    headers: {{ team: x-attr-team }}
    spend_caps:
      team:
        default: 3
routes:
  - prefix: /openai
    provider: openai-main
rejections:
  default_response:
    status: 428
    content_type: application/json
    body: '{{"error":"no_attr","key":"{{{{key}}}}","cap":"{{{{cap}}}}","spend":"{{{{spend}}}}"}}'
  unknown_route:
    status: 404
    content_type: application/json
    body: '{{"error":"unknown_route","path":"{{{{route}}}}"}}'
"#
    )
}

/// Contract: the optional `{{cap}}`/`{{spend}}` placeholders default to
/// `-` on every non-budget rejection, so an operator body that uses them
/// never leaks a literal placeholder. Byte-pinned.
#[test]
fn contract_cap_and_spend_placeholders_default_to_dash_on_non_budget_rejections() {
    check(
        "GB-4",
        "contract_cap_and_spend_placeholders_default_to_dash_on_non_budget_rejections",
        || {
            let p = ports(2);
            let _mock = spawn_mock(p[0], &spike_fixture("openai.sse"), "openai", false);
            let _gw = spawn_gatewayd(&contract_cfg(p[0]), p[1], "ctr1");

            // Missing required key: a non-budget rejection.
            let r = http(p[1], "POST", "/openai/v1/chat", &[], b"{}");
            assert_eq!(r.status, 428);
            let body = r.body_text();
            assert!(
                body.contains(r#""cap":"-""#) && body.contains(r#""spend":"-""#),
                "the dash defaults are contract: {body}"
            );
        },
    );
}

/// Contract: with no operator streaming block anywhere, a mid-stream cut
/// wears the built-in default payload: a bare SSE data frame (no event
/// line) whose body names the exhausted spender, cap, and spend. The
/// shape is pinned up to the live spend number.
#[test]
fn contract_default_cut_payload_shape_is_frozen() {
    check(
        "GB-4",
        "contract_default_cut_payload_shape_is_frozen",
        || {
            let p = ports(2);
            let _mock = spawn_mock(p[0], &spike_fixture("openai.sse"), "openai", false);
            let _gw = spawn_gatewayd(&contract_cfg(p[0]), p[1], "ctr2");

            // The 3-token cap trips mid-stream on the first request.
            let r = http(
                p[1],
                "POST",
                "/openai/v1/chat",
                &[("x-attr-team", "ml-research")],
                b"{}",
            );
            assert_eq!(r.status, 200, "cut streams still start as 200");
            let stream = r.body_text();
            assert!(
                stream.contains(r#"data: {"error":"budget exhausted for team=ml-research","cap":3,"spend":"#),
                "default cut payload shape: {stream}"
            );
            assert!(
                !stream.contains("event: stream_cut"),
                "the default SSE cut is a bare data frame, no event line: {stream}"
            );
        },
    );
}
