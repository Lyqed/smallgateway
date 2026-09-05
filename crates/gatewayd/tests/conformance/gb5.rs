//! GB-5 (spend caps via budget shares) + GB-6 (native alerts) conformance,
//! Phase 3: driven against a REAL gatewayd (see the module note in main.rs).
//! Split out of main.rs to keep each file under the size budget.

use crate::harness::*;

// -------------------------------------------------------------- GB-5 / GB-6

/// A config with a GB-5 spend cap on `team=ml-research` (tokens) and a GB-4
/// streaming terminal event, so both the request-start rejection and the
/// mid-stream cut are exercised against the real binary. `cap` is the fleet
/// default; the streaming block is the terminal event a cut stream emits.
fn budget_cfg(mock_port: u16, cap: u64) -> String {
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
    pinned: {{ env: prod }}
    spend_caps:
      team:
        default: {cap}
        overrides:
          unlimited-team: null
routes:
  - prefix: /openai
    provider: openai-main
rejections:
  default_response:
    status: 428
    content_type: application/json
    body: '{{"error":"budget_or_attribution","key":"{{{{key}}}}","cap":"{{{{cap}}}}","spend":"{{{{spend}}}}"}}'
    streaming:
      event: error
      data: '{{"error":"budget_exhausted","key":"{{{{key}}}}","cap":{{{{cap}}}},"spend":{{{{spend}}}}}}'
  unknown_route:
    status: 404
    content_type: application/json
    body: '{{"error":"unknown_route","path":"{{{{route}}}}"}}'
"#
    )
}

#[test]
fn gb5_value_over_its_cap_is_rejected_at_request_start_with_the_operator_template() {
    check(
        "GB-5",
        "gb5_value_over_its_cap_is_rejected_at_request_start_with_the_operator_template",
        || {
            let p = ports(2);
            // The openai fixture meters ~10 estimated output tokens per stream.
            // A 6-token cap is exhausted by the FIRST stream, so the SECOND
            // request for the same value is denied at admission before it ever
            // reaches the upstream.
            let _mock = spawn_mock(p[0], &spike_fixture("openai.sse"), "openai", false);
            let _gw = spawn_gatewayd(&budget_cfg(p[0], 6), p[1], "gb5a");

            // First request: allowed (the cap is not yet consumed at admission),
            // and it spends past the cap while streaming.
            let first = http(
                p[1],
                "POST",
                "/openai/v1/chat",
                &[("x-attr-team", "ml-research")],
                b"{}",
            );
            assert_eq!(first.status, 200, "first request admitted");

            // Second request for the SAME value: the cap is now exhausted, so it
            // is rejected at request start with the operator's GB-4 body, naming
            // the cap and spend. No token reaches the upstream.
            let second = http(
                p[1],
                "POST",
                "/openai/v1/chat",
                &[("x-attr-team", "ml-research")],
                b"{}",
            );
            assert_eq!(second.status, 428, "cap-exhausted value rejected at start");
            let body = second.body_text();
            assert!(body.contains("budget_or_attribution"), "operator body: {body}");
            assert!(body.contains(r#""cap":"6""#), "cap named: {body}");

            // A DIFFERENT value is unaffected — the cap is per attribution value.
            let other = http(
                p[1],
                "POST",
                "/openai/v1/chat",
                &[("x-attr-team", "web-team")],
                b"{}",
            );
            assert_eq!(other.status, 200, "a different value spends its own cap");
        },
    );
}

#[test]
fn gb5_cap_refusal_and_cut_wear_the_dedicated_cap_exceeded_voice() {
    check(
        "GB-5",
        "gb5_cap_refusal_and_cut_wear_the_dedicated_cap_exceeded_voice",
        || {
            let p = ports(2);
            // Same 3-token cap as the cut test, plus a DEDICATED cap_exceeded
            // template (429, its own body, its own streaming event). Both the
            // mid-stream cut and the admission refusal must speak with it —
            // and a plain missing-key refusal must still speak
            // default_response, proving the templates stay separate.
            let _mock = spawn_mock(p[0], &spike_fixture("openai.sse"), "openai", false);
            let cfg = budget_cfg(p[0], 3)
                + concat!(
                    "  cap_exceeded:\n",
                    "    status: 429\n",
                    "    content_type: application/json\n",
                    "    body: '{\"error\":\"token_budget_exhausted\",\"who\":\"{{key}}\",\"cap\":\"{{cap}}\",\"spend\":\"{{spend}}\"}'\n",
                    "    streaming:\n",
                    "      event: cap\n",
                    "      data: '{\"error\":\"cap_cut\",\"who\":\"{{key}}\"}'\n",
                );
            let _gw = spawn_gatewayd(&cfg, p[1], "gb5v");

            // First stream crosses the cap mid-generation: cut with the
            // DEDICATED terminal event, not default_response's.
            let first = http(
                p[1],
                "POST",
                "/openai/v1/chat",
                &[("x-attr-team", "ml-research")],
                b"{}",
            );
            assert_eq!(first.status, 200);
            let stream = first.body_text();
            assert!(stream.contains("event: cap"), "dedicated event name: {stream}");
            assert!(stream.contains("cap_cut"), "dedicated event body: {stream}");
            assert!(!stream.contains("budget_exhausted"), "not the fallback voice: {stream}");

            // Admission refusal for the exhausted value: the dedicated 429.
            let second = http(
                p[1],
                "POST",
                "/openai/v1/chat",
                &[("x-attr-team", "ml-research")],
                b"{}",
            );
            assert_eq!(second.status, 429, "dedicated status: {}", second.body_text());
            let body = second.body_text();
            assert!(body.contains("token_budget_exhausted"), "{body}");
            assert!(body.contains(r#""cap":"3""#), "cap named: {body}");

            // A missing attribution key still speaks default_response.
            let missing = http(p[1], "POST", "/openai/v1/chat", &[], b"{}");
            assert_eq!(missing.status, 428);
            assert!(missing.body_text().contains("budget_or_attribution"));
        },
    );
}

#[test]
fn gb5_budget_exhausted_mid_stream_cuts_with_the_gb4_terminal_event() {
    check(
        "GB-5",
        "gb5_budget_exhausted_mid_stream_cuts_with_the_gb4_terminal_event",
        || {
            let p = ports(2);
            // A 3-token cap: the fixture (~10 estimated output tokens across
            // several delayed chunks) crosses it MID-generation, so the stream
            // is cut with the operator's terminal event rather than running to
            // completion. The mock streams frame-per-chunk with a delay, so the
            // tap meters incrementally and the cut fires before [DONE].
            let _mock = spawn_mock(p[0], &spike_fixture("openai.sse"), "openai", false);
            let _gw = spawn_gatewayd(&budget_cfg(p[0], 3), p[1], "gb5b");

            let resp = http(
                p[1],
                "POST",
                "/openai/v1/chat",
                &[("x-attr-team", "ml-research")],
                b"{}",
            );
            // The upstream returned 200 and streaming had begun before the cut,
            // so the client sees a 200 whose body ends in the terminal event.
            assert_eq!(resp.status, 200);
            let body = resp.body_text();
            assert!(
                body.contains("budget_exhausted"),
                "terminal event present: {body}"
            );
            assert!(
                body.contains("event: error"),
                "terminal event carries the operator event name: {body}"
            );
            // The stream was CUT: the upstream's [DONE] sentinel never reaches
            // the client, because content after the cut is suppressed.
            assert!(
                !body.contains("[DONE]"),
                "cut stream must not run to completion: {body}"
            );
        },
    );
}

#[test]
fn gb5_an_uncapped_value_override_is_never_cut() {
    check("GB-5", "gb5_an_uncapped_value_override_is_never_cut", || {
        let p = ports(2);
        // `unlimited-team` has an explicit `null` override: uncapped even under
        // a tiny fleet default. Its stream runs to completion, [DONE] included.
        let _mock = spawn_mock(p[0], &spike_fixture("openai.sse"), "openai", false);
        let _gw = spawn_gatewayd(&budget_cfg(p[0], 3), p[1], "gb5c");

        let resp = http(
            p[1],
            "POST",
            "/openai/v1/chat",
            &[("x-attr-team", "unlimited-team")],
            b"{}",
        );
        assert_eq!(resp.status, 200);
        let body = resp.body_text();
        assert!(body.contains("[DONE]"), "uncapped stream completes: {body}");
        assert!(!body.contains("budget_exhausted"), "not cut: {body}");
    });
}

#[test]
fn gb5_cap_composes_down_the_scoped_chain_route_tightens_fleet() {
    check(
        "GB-5",
        "gb5_cap_composes_down_the_scoped_chain_route_tightens_fleet",
        || {
            let p = ports(2);
            // Fleet default 1_000_000 (effectively unlimited for this fixture);
            // a route lowers team's default to 3 tokens. The route-scoped cap
            // wins (lower scope tightens), so the stream on that route is cut.
            let cfg = budget_cfg(p[0], 1_000_000).replace(
                "  - prefix: /openai\n    provider: openai-main\n",
                concat!(
                    "  - prefix: /openai\n",
                    "    provider: openai-main\n",
                    "    attribution:\n",
                    "      spend_caps:\n",
                    "        team:\n",
                    "          default: 3\n",
                ),
            );
            let _mock = spawn_mock(p[0], &spike_fixture("openai.sse"), "openai", false);
            let _gw = spawn_gatewayd(&cfg, p[1], "gb5d");

            let resp = http(
                p[1],
                "POST",
                "/openai/v1/chat",
                &[("x-attr-team", "ml-research")],
                b"{}",
            );
            assert_eq!(resp.status, 200);
            let body = resp.body_text();
            // The route's tighter 3-token cap applied, not the fleet's 1M: cut.
            assert!(
                body.contains("budget_exhausted"),
                "route-scoped tighter cap enforced: {body}"
            );
            assert!(!body.contains("[DONE]"), "route cap cut the stream: {body}");
        },
    );
}

/// Regression: rejection overrides compose down the scope chain for
/// ADMISSION refusals, and the mid-stream cut must honor the same
/// composition. Before the fix, the cut read the FLEET streaming template
/// (proxy.rs response tap) and a route's dedicated cap_exceeded streaming
/// voice was silently ignored.
#[test]
fn gb4_route_scoped_streaming_template_speaks_on_the_mid_stream_cut() {
    check(
        "GB-4",
        "gb4_route_scoped_streaming_template_speaks_on_the_mid_stream_cut",
        || {
            let p = ports(2);
            let _mock = spawn_mock(p[0], &spike_fixture("openai.sse"), "openai", false);
            // Fleet carries its own cap_exceeded streaming voice; the ROUTE
            // overrides it with a different one. The cut must speak ROUTE.
            let cfg = format!(
                r#"
providers:
  openai-main:
    kind: openai
    upstream: {{ host: 127.0.0.1, port: {mock} }}
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
      cap_exceeded:
        status: 429
        content_type: application/json
        body: '{{"error":"route_refusal","who":"{{{{key}}}}"}}'
        streaming:
          event: route-cap
          data: '{{"error":"route_cut","who":"{{{{key}}}}"}}'
rejections:
  default_response:
    status: 428
    content_type: application/json
    body: '{{"error":"no_attr","key":"{{{{key}}}}"}}'
  unknown_route:
    status: 404
    content_type: application/json
    body: '{{"error":"unknown_route","path":"{{{{route}}}}"}}'
  cap_exceeded:
    status: 429
    content_type: application/json
    body: '{{"error":"fleet_refusal","who":"{{{{key}}}}"}}'
    streaming:
      event: fleet-cap
      data: '{{"error":"fleet_cut","who":"{{{{key}}}}"}}'
"#,
                mock = p[0]
            );
            let _gw = spawn_gatewayd(&cfg, p[1], "gb4rs");

            // The 3-token cap trips mid-stream on the first request: the cut
            // must wear the ROUTE's streaming voice, not the fleet's.
            let first = http(
                p[1],
                "POST",
                "/openai/v1/chat",
                &[("x-attr-team", "ml-research")],
                b"{}",
            );
            assert_eq!(first.status, 200);
            let stream = first.body_text();
            assert!(stream.contains("event: route-cap"), "route voice on the cut: {stream}");
            assert!(stream.contains("route_cut"), "route payload on the cut: {stream}");
            assert!(!stream.contains("fleet_cut"), "fleet voice must not speak: {stream}");

            // And the admission refusal for the now-exhausted value wears the
            // same route-scoped template, proving one composition serves both.
            let second = http(
                p[1],
                "POST",
                "/openai/v1/chat",
                &[("x-attr-team", "ml-research")],
                b"{}",
            );
            assert_eq!(second.status, 429);
            assert!(second.body_text().contains("route_refusal"));
        },
    );
}
