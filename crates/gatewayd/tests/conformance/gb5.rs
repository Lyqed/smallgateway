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
  missing_attribution:
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
