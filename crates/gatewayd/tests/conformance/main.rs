//! Phase 1 exit criterion: the Baseline checks as automated conformance
//! tests, run against a REAL gatewayd instance (the actual binary,
//! spawned per test on its own ports) with mock upstreams — the same
//! public-documentation standard every other gateway's tracker row gets.
//!
//! Naming maps tests to checks: `gb1_*` … `gb8_*` (plus `cel_*` and
//! `scope_*` for the tier-1/composition mechanics the checks lean on).
//! Every test records its verdict into `target/conformance.json` — a
//! machine-readable check → pass/fail summary for the tracker's future
//! consumption. The file is rewritten after every recorded test, so even
//! a partial run leaves a consistent artifact.
//!
//! GB-5 (spend caps via budget shares) and GB-6 (native alerts) land in
//! Phase 3: `gb5_*` / `gb6_*` below drive a REAL gatewayd — a value hitting its
//! cap gets the GB-4 rejection at request start, a budget exhausted mid-stream
//! is CUT with the operator's terminal event, and the cap composes down the
//! scoped chain. The mid-stream cut's alert firing at the enforcement point, the
//! ~90% escalation boundary, the share allocation + continuous rebalance, and
//! the MEASURED bounded overspend under partition are unit-tested in
//! `gateway_core::budget`, `gatewayd::budget`, and `gatewayctl::budget`, and
//! shown end-to-end in `../gatewayctl/scripts/budget-demo.sh`.

mod harness;
use std::process::Command;

use harness::*;

// ------------------------------------------------------------ configs

/// The standard conformance config: openai route with fleet-scope
/// attribution (required team + pinned env), a claim-mapped route, and
/// operator rejection templates with recognizable bodies.
fn base_cfg(mock_port: u16) -> String {
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
routes:
  - prefix: /openai
    provider: openai-main
  - prefix: /claims
    provider: openai-main
    attribution:
      required_keys: ['<base>', user]
      from_claims: {{ user: sub }}
auth:
  jwt:
    hs256_secret: conformance-secret
rejections:
  missing_attribution:
    status: 428
    content_type: application/json
    body: '{{"error":"attribution_required","missing":"{{{{key}}}}","route":"{{{{route}}}}"}}'
  unknown_route:
    status: 404
    content_type: application/json
    body: '{{"error":"unknown_route","path":"{{{{route}}}}"}}'
"#
    )
}

// -------------------------------------------------------------- GB-1

#[test]
fn gb1_missing_required_key_rejected_with_operator_template() {
    check("GB-1", "gb1_missing_required_key_rejected_with_operator_template", || {
        let p = ports(2);
        let _mock = spawn_mock(p[0], &spike_fixture("openai.sse"), "openai", false);
        let _gw = spawn_gatewayd(&base_cfg(p[0]), p[1], "gb1a");
        let resp = http(p[1], "POST", "/openai/v1/chat/completions", &[], b"{}");
        assert_eq!(resp.status, 428, "operator status, not a generic 4xx");
        assert_eq!(resp.header("content-type"), Some("application/json"));
        let body = resp.body_text();
        assert!(body.contains(r#""error":"attribution_required""#), "{body}");
        assert!(body.contains(r#""missing":"team""#), "{body}");
        assert!(body.contains(r#""route":"/openai""#), "{body}");
    });
}

#[test]
fn gb1_required_key_present_reaches_the_upstream() {
    check("GB-1", "gb1_required_key_present_reaches_the_upstream", || {
        let p = ports(2);
        let _mock = spawn_mock(p[0], &spike_fixture("openai.sse"), "openai", false);
        let _gw = spawn_gatewayd(&base_cfg(p[0]), p[1], "gb1b");
        let resp = http(
            p[1],
            "POST",
            "/openai/v1/chat/completions",
            &[("x-attr-team", "ml-research")],
            b"{}",
        );
        assert_eq!(resp.status, 200);
        assert_eq!(resp.header("x-echo-attr-team"), Some("ml-research"));
        // The streamed fixture arrives intact through the tap.
        assert!(resp.body_text().contains("[DONE]"));
    });
}

#[test]
fn gb1_app_override_pin_satisfies_a_required_key() {
    check("GB-1", "gb1_app_override_pin_satisfies_a_required_key", || {
        let p = ports(2);
        // purpose is required at fleet scope; only the ml-research app
        // pins it — other teams must send it themselves.
        let cfg = base_cfg(p[0]).replace(
            "routes:",
            concat!(
                "apps:\n",
                "  key: team\n",
                "  overrides:\n",
                "    ml-research:\n",
                "      attribution:\n",
                "        pinned: { purpose: research }\n",
                "routes:",
            ),
        );
        let cfg = cfg.replace("required_keys: [team]", "required_keys: [team, purpose]");
        let _mock = spawn_mock(p[0], &spike_fixture("openai.sse"), "openai", false);
        let _gw = spawn_gatewayd(&cfg, p[1], "gb1c");

        // ml-research: the app override pins purpose → allowed, pin visible.
        let resp = http(
            p[1],
            "POST",
            "/openai/v1/chat",
            &[("x-attr-team", "ml-research")],
            b"{}",
        );
        assert_eq!(resp.status, 200);
        assert_eq!(resp.header("x-echo-attr-purpose"), Some("research"));

        // Any other team: purpose still required → operator 428.
        let resp = http(p[1], "POST", "/openai/v1/chat", &[("x-attr-team", "web")], b"{}");
        assert_eq!(resp.status, 428);
        assert!(resp.body_text().contains(r#""missing":"purpose""#));
    });
}

// -------------------------------------------------------------- GB-2

#[test]
fn gb2_claim_mapped_key_proven_from_verified_jwt() {
    check("GB-2", "gb2_claim_mapped_key_proven_from_verified_jwt", || {
        let p = ports(2);
        let _mock = spawn_mock(p[0], &spike_fixture("openai.sse"), "openai", false);
        let _gw = spawn_gatewayd(&base_cfg(p[0]), p[1], "gb2a");
        let token = mint_jwt(serde_json::json!({"sub": "alice", "exp": 4102444800u64}));
        let auth = format!("Bearer {token}");
        let resp = http(
            p[1],
            "POST",
            "/claims/v1/chat",
            &[
                ("authorization", auth.as_str()),
                ("x-attr-team", "ml-research"),
                ("x-attr-user", "mallory"), // forged; must be ignored
            ],
            b"{}",
        );
        assert_eq!(resp.status, 200);
        // The PROVEN claim value reached the upstream, not the forgery.
        assert_eq!(resp.header("x-echo-attr-user"), Some("alice"));
    });
}

#[test]
fn gb2_forged_caller_header_never_believed_without_token() {
    check("GB-2", "gb2_forged_caller_header_never_believed_without_token", || {
        let p = ports(2);
        let _mock = spawn_mock(p[0], &spike_fixture("openai.sse"), "openai", false);
        let _gw = spawn_gatewayd(&base_cfg(p[0]), p[1], "gb2b");
        let resp = http(
            p[1],
            "POST",
            "/claims/v1/chat",
            &[("x-attr-team", "ml-research"), ("x-attr-user", "mallory")],
            b"{}",
        );
        assert_eq!(resp.status, 428, "claim-mapped key: proven or absent, never believed");
        assert!(resp.body_text().contains(r#""missing":"user""#));
    });
}

// -------------------------------------------------------------- GB-3

#[test]
fn gb3_pinned_key_overwrites_caller_value() {
    check("GB-3", "gb3_pinned_key_overwrites_caller_value", || {
        let p = ports(2);
        let _mock = spawn_mock(p[0], &spike_fixture("openai.sse"), "openai", false);
        let _gw = spawn_gatewayd(&base_cfg(p[0]), p[1], "gb3a");
        let resp = http(
            p[1],
            "POST",
            "/openai/v1/chat",
            &[("x-attr-team", "ml-research"), ("x-attr-env", "shadow-prod")],
            b"{}",
        );
        assert_eq!(resp.status, 200);
        // The upstream saw the ASSIGNED value; the forgery never crossed.
        assert_eq!(resp.header("x-echo-attr-env"), Some("prod"));
    });
}

// -------------------------------------------------------------- GB-4

#[test]
fn gb4_unknown_route_uses_the_operator_template_verbatim() {
    check("GB-4", "gb4_unknown_route_uses_the_operator_template_verbatim", || {
        let p = ports(2);
        let _gw = spawn_gatewayd(&base_cfg(p[0]), p[1], "gb4a");
        let resp = http(p[1], "GET", "/definitely/not/routed", &[], b"");
        assert_eq!(resp.status, 404);
        assert_eq!(resp.header("content-type"), Some("application/json"));
        let body = resp.body_text();
        assert!(body.contains(r#""error":"unknown_route""#), "{body}");
        assert!(body.contains(r#""path":"/definitely/not/routed""#), "{body}");
    });
}

#[test]
fn gb4_scoped_rejection_template_overrides_down_the_chain() {
    check("GB-4", "gb4_scoped_rejection_template_overrides_down_the_chain", || {
        let p = ports(2);
        let cfg = base_cfg(p[0]).replace(
            "  - prefix: /openai\n    provider: openai-main",
            concat!(
                "  - prefix: /openai\n    provider: openai-main\n",
                "    rejections:\n",
                "      missing_attribution:\n",
                "        status: 451\n",
                "        content_type: text/plain\n",
                "        body: 'route scope says: {{key}} required'",
            ),
        );
        let _gw = spawn_gatewayd(&cfg, p[1], "gb4b");
        // The overridden route uses its own template...
        let resp = http(p[1], "POST", "/openai/v1/chat", &[], b"{}");
        assert_eq!(resp.status, 451);
        assert_eq!(resp.header("content-type"), Some("text/plain"));
        assert!(resp.body_text().contains("route scope says: team required"));
        // ...while the untouched route keeps the fleet template.
        let resp = http(p[1], "POST", "/claims/v1/chat", &[], b"{}");
        assert_eq!(resp.status, 428);
    });
}

// -------------------------------------------------------------- GB-7

/// Bedrock + STS config: session tags from a PROVEN (claim-mapped) key and
/// an ASSIGNED (pinned) key — never caller-raw.
fn gb7_cfg(bedrock_port: u16, sts_port: u16) -> String {
    format!(
        r#"
providers:
  bedrock-main:
    kind: bedrock
    upstream: {{ host: 127.0.0.1, port: {bedrock_port} }}
    sts:
      endpoint: {{ host: 127.0.0.1, port: {sts_port} }}
      role_arn: arn:aws:iam::000000000000:role/conformance
      region: us-east-1
      tags:
        - {{ key: user, from_attribution: user }}
        - {{ key: env, from_attribution: env }}
routes:
  - prefix: /bedrock
    provider: bedrock-main
    attribution:
      required_keys: [user]
      pinned: {{ env: prod }}
      from_claims: {{ user: sub }}
auth:
  jwt:
    hs256_secret: conformance-secret
rejections:
  missing_attribution:
    status: 428
    content_type: application/json
    body: '{{"error":"attribution_required","missing":"{{{{key}}}}","route":"{{{{route}}}}"}}'
  unknown_route:
    status: 404
    content_type: application/json
    body: '{{"error":"unknown_route","path":"{{{{route}}}}"}}'
"#
    )
}

#[test]
fn gb7_session_tags_ride_the_credentials_to_bedrock() {
    check("GB-7", "gb7_session_tags_ride_the_credentials_to_bedrock", || {
        let p = ports(3);
        let _sts = spawn_sts(p[1]);
        let _mock = spawn_mock(p[0], &spike_fixture("bedrock.jsonl"), "bedrock", true);
        let _gw = spawn_gatewayd(&gb7_cfg(p[0], p[1]), p[2], "gb7a");
        let token = mint_jwt(serde_json::json!({"sub": "alice", "exp": 4102444800u64}));
        let auth = format!("Bearer {token}");
        let resp = http(
            p[2],
            "POST",
            "/bedrock/model/anthropic.claude/converse-stream",
            &[("authorization", auth.as_str())],
            b"{}",
        );
        // The mock REQUIRES a valid SigV4 signature: a 200 here means the
        // credential exchange + signing round-tripped. The echoed session
        // tags were decoded from the SECURITY TOKEN, not from any header —
        // the attribution rode the credentials (the invoice-grade join).
        assert_eq!(resp.status, 200, "body: {}", resp.body_text());
        assert_eq!(resp.header("x-echo-session-tag-user"), Some("alice"));
        assert_eq!(resp.header("x-echo-session-tag-env"), Some("prod"));
        assert!(resp.header("x-echo-access-key-id").unwrap().starts_with("ASIAMOCK"));
    });
}

#[test]
fn gb7_credentials_cached_per_tag_set_with_expiry() {
    check("GB-7", "gb7_credentials_cached_per_tag_set_with_expiry", || {
        let p = ports(3);
        let _sts = spawn_sts(p[1]);
        let _mock = spawn_mock(p[0], &spike_fixture("bedrock.jsonl"), "bedrock", true);
        let _gw = spawn_gatewayd(&gb7_cfg(p[0], p[1]), p[2], "gb7b");
        let alice = format!(
            "Bearer {}",
            mint_jwt(serde_json::json!({"sub": "alice", "exp": 4102444800u64}))
        );
        let bob = format!(
            "Bearer {}",
            mint_jwt(serde_json::json!({"sub": "bob", "exp": 4102444800u64}))
        );
        let path = "/bedrock/model/anthropic.claude/converse-stream";

        // The mock STS numbers every AssumeRole; the access key id echoed
        // by the mock Bedrock is therefore an exchange counter.
        let r1 = http(p[2], "POST", path, &[("authorization", alice.as_str())], b"{}");
        let r2 = http(p[2], "POST", path, &[("authorization", alice.as_str())], b"{}");
        let r3 = http(p[2], "POST", path, &[("authorization", bob.as_str())], b"{}");
        assert_eq!(r1.status, 200, "{}", r1.body_text());
        assert_eq!(r2.status, 200);
        assert_eq!(r3.status, 200);
        let k1 = r1.header("x-echo-access-key-id").unwrap().to_string();
        let k2 = r2.header("x-echo-access-key-id").unwrap().to_string();
        let k3 = r3.header("x-echo-access-key-id").unwrap().to_string();
        assert_eq!(k1, k2, "same tag-set: cache hit, ONE exchange");
        assert_ne!(k1, k3, "different tag-set: its own credentials");
        assert_eq!(r3.header("x-echo-session-tag-user"), Some("bob"));
    });
}

#[test]
fn gb7_caller_raw_session_tag_rejected_at_config_load() {
    check("GB-7", "gb7_caller_raw_session_tag_rejected_at_config_load", || {
        let p = ports(2);
        // `user` demoted to a plain required key: caller-asserted. A
        // session tag from it must be refused before the gateway serves.
        let cfg = gb7_cfg(p[0], p[1])
            .replace("      from_claims: { user: sub }\n", "")
            .replace("auth:\n  jwt:\n    hs256_secret: conformance-secret\n", "");
        let path = temp_file("gb7c", &cfg);
        let out = Command::new(env!("CARGO_BIN_EXE_gatewayd"))
            .args(["--config", &path.display().to_string()])
            .output()
            .expect("run gatewayd");
        assert!(!out.status.success(), "a caller-raw session tag must fail startup");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("never caller-raw"), "{stderr}");
    });
}

// -------------------------------------------------------------- GB-8

fn gb8_cfg(vertex_port: u16) -> String {
    format!(
        r#"
providers:
  vertex-main:
    kind: vertex
    upstream: {{ host: 127.0.0.1, port: {vertex_port} }}
fleet:
  attribution:
    required_keys: [team]
    pinned: {{ env: prod }}
routes:
  - prefix: /vertex
    provider: vertex-main
    attribution:
      from_claims: {{ buyer: buyer_id }}
    labels:
      - key: cost_center
        value: platform
      - key: team
        from_attribution: team
      - key: env_channel
        expression: 'attribution["env"] + "-gw"'
routes_extra_marker: []
auth:
  jwt:
    hs256_secret: conformance-secret
rejections:
  missing_attribution:
    status: 428
    content_type: application/json
    body: '{{"error":"attribution_required","missing":"{{{{key}}}}","route":"{{{{route}}}}"}}'
  unknown_route:
    status: 404
    content_type: application/json
    body: '{{"error":"unknown_route","path":"{{{{route}}}}"}}'
"#
    )
    .replace("routes_extra_marker: []\n", "")
}

#[test]
fn gb8_operator_labels_merged_into_body_operator_wins() {
    check("GB-8", "gb8_operator_labels_merged_into_body_operator_wins", || {
        let p = ports(2);
        let _mock = spawn_mock(p[0], &demo_fixture("vertex.sse"), "vertex", false);
        let _gw = spawn_gatewayd(&gb8_cfg(p[0]), p[1], "gb8a");
        // The client tries to spoof cost_center and sends its own label.
        let body = br#"{"contents":[{"parts":[{"text":"hi"}]}],"labels":{"cost_center":"client-spoof","client_extra":"kept"}}"#;
        let resp = http(
            p[1],
            "POST",
            "/vertex/v1/models/gemini:streamGenerateContent",
            &[("x-attr-team", "ml-research"), ("content-type", "application/json")],
            body,
        );
        assert_eq!(resp.status, 200, "{}", resp.body_text());
        // Operator label wins the conflict…
        assert_eq!(resp.header("x-echo-label-cost_center"), Some("platform"));
        // …attribution-derived and CEL labels landed…
        assert_eq!(resp.header("x-echo-label-team"), Some("ml-research"));
        assert_eq!(resp.header("x-echo-label-env_channel"), Some("prod-gw"));
        // …and the client's non-conflicting label passed through unchanged.
        assert_eq!(resp.header("x-echo-label-client_extra"), Some("kept"));
        // The streamed vertex fixture came back through the tap intact.
        assert!(resp.body_text().contains("gemini-2.5-flash"));
    });
}

#[test]
fn gb8_unresolvable_label_fails_closed_with_gb4_template() {
    check("GB-8", "gb8_unresolvable_label_fails_closed_with_gb4_template", || {
        let p = ports(2);
        // A label sourced from the OPTIONAL claim-mapped key `buyer`: with
        // no verified token the key never resolves, and the request must
        // die on the operator's template — not reach Vertex unattributed.
        let cfg = gb8_cfg(p[0]).replace(
            "      - key: cost_center\n        value: platform\n",
            concat!(
                "      - key: cost_center\n        value: platform\n",
                "      - key: buyer\n        from_attribution: buyer\n",
            ),
        );
        let _mock = spawn_mock(p[0], &demo_fixture("vertex.sse"), "vertex", false);
        let _gw = spawn_gatewayd(&cfg, p[1], "gb8b");
        let resp = http(
            p[1],
            "POST",
            "/vertex/v1/models/gemini:streamGenerateContent",
            &[("x-attr-team", "ml-research")],
            br#"{"contents":[]}"#,
        );
        assert_eq!(resp.status, 428, "fail closed: {}", resp.body_text());
        let body = resp.body_text();
        assert!(body.contains(r#""missing":"buyer""#), "{body}");
        assert!(body.contains(r#""route":"/vertex""#), "{body}");
    });
}

// ------------------------------------------------- tier-1 CEL + scopes

#[test]
fn cel_route_condition_gates_matching_beyond_prefix() {
    check("tier1-cel", "cel_route_condition_gates_matching_beyond_prefix", || {
        let p = ports(2);
        let cfg = base_cfg(p[0]).replace(
            "routes:",
            concat!(
                "routes:\n",
                "  - prefix: /openai\n",
                "    provider: openai-main\n",
                "    match: 'request.headers[\"x-variant\"] == \"beta\"'\n",
                "    attribution:\n",
                "      pinned: { variant: beta }\n",
            ),
        );
        let _mock = spawn_mock(p[0], &spike_fixture("openai.sse"), "openai", false);
        let _gw = spawn_gatewayd(&cfg, p[1], "cel1");

        // The conditioned route wins when its predicate holds…
        let resp = http(
            p[1],
            "POST",
            "/openai/v1/chat",
            &[("x-attr-team", "ml"), ("x-variant", "beta")],
            b"{}",
        );
        assert_eq!(resp.status, 200);
        assert_eq!(resp.header("x-echo-attr-variant"), Some("beta"));

        // …and the unconditioned fallback serves everyone else (the
        // absent-header predicate ERRORS, which can never select a route).
        let resp = http(p[1], "POST", "/openai/v1/chat", &[("x-attr-team", "ml")], b"{}");
        assert_eq!(resp.status, 200);
        assert_eq!(resp.header("x-echo-attr-variant"), None);
    });
}

#[test]
fn cel_derived_attribution_value_from_claim_transform() {
    check("tier1-cel", "cel_derived_attribution_value_from_claim_transform", || {
        let p = ports(2);
        let cfg = base_cfg(p[0]).replace(
            "  - prefix: /openai\n    provider: openai-main",
            concat!(
                "  - prefix: /openai\n    provider: openai-main\n",
                "    attribution:\n",
                "      derived:\n",
                "        org: 'jwt.claims.team_id.split(\"-\")[0]'",
            ),
        );
        let _mock = spawn_mock(p[0], &spike_fixture("openai.sse"), "openai", false);
        let _gw = spawn_gatewayd(&cfg, p[1], "cel2");
        let token = mint_jwt(serde_json::json!({"team_id": "ml-research-42", "exp": 4102444800u64}));
        let auth = format!("Bearer {token}");
        let resp = http(
            p[1],
            "POST",
            "/openai/v1/chat",
            &[
                ("authorization", auth.as_str()),
                ("x-attr-team", "ml-research"),
                ("x-attr-org", "forged-org"), // derived keys are never believed
            ],
            b"{}",
        );
        assert_eq!(resp.status, 200);
        assert_eq!(resp.header("x-echo-attr-org"), Some("ml"), "transform, not copy; forgery ignored");
    });
}

#[test]
fn cel_comprehension_label_rejected_at_config_load() {
    check("tier1-cel", "cel_comprehension_label_rejected_at_config_load", || {
        let p = ports(1);
        // The adversarial probe that used to wedge a worker: a short,
        // shallow, load-acceptable label expression that is quadratic in a
        // caller-controlled header. CEL's only loops are the comprehension
        // macros, so the config must REFUSE to load — the request hot path
        // never evaluates one.
        let cfg = gb8_cfg(p[0]).replace(
            r#"expression: 'attribution["env"] + "-gw"'"#,
            r#"expression: 'string(size(request.headers["x-p"].split("").map(a, request.headers["x-p"].split(""))))'"#,
        );
        let path = temp_file("cel3", &cfg);
        let out = Command::new(env!("CARGO_BIN_EXE_gatewayd"))
            .args(["--config", &path.display().to_string()])
            .output()
            .expect("run gatewayd");
        assert!(
            !out.status.success(),
            "a comprehension label expression must fail startup"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("comprehension"), "{stderr}");
        assert!(stderr.contains("runtime cost limit"), "{stderr}");
    });
}

#[test]
fn scope_chain_composes_fleet_project_route_app() {
    check("scoped-chain", "scope_chain_composes_fleet_project_route_app", || {
        let p = ports(2);
        let cfg = format!(
            r#"
providers:
  openai-main:
    kind: openai
    upstream: {{ host: 127.0.0.1, port: {} }}
fleet:
  attribution:
    required_keys: [team]
    pinned: {{ env: fleet-prod, region: eu }}
projects:
  ml:
    attribution:
      pinned: {{ env: ml-prod }}
routes:
  - prefix: /openai
    provider: openai-main
    project: ml
    attribution:
      pinned: {{ cost: route-cost }}
apps:
  key: team
  overrides:
    ml-research:
      attribution:
        pinned: {{ cost: app-cost }}
rejections:
  missing_attribution:
    status: 428
    content_type: application/json
    body: '{{"error":"attribution_required","missing":"{{{{key}}}}"}}'
  unknown_route:
    status: 404
    content_type: application/json
    body: '{{"error":"unknown_route"}}'
"#,
            p[0]
        );
        let _mock = spawn_mock(p[0], &spike_fixture("openai.sse"), "openai", false);
        let _gw = spawn_gatewayd(&cfg, p[1], "scope1");

        // team=web: fleet pin overridden by project, route pin applies.
        let resp = http(p[1], "POST", "/openai/v1/chat", &[("x-attr-team", "web")], b"{}");
        assert_eq!(resp.status, 200);
        assert_eq!(resp.header("x-echo-attr-env"), Some("ml-prod"));
        assert_eq!(resp.header("x-echo-attr-region"), Some("eu"));
        assert_eq!(resp.header("x-echo-attr-cost"), Some("route-cost"));

        // team=ml-research: the app layer overrides the route's pin.
        let resp = http(
            p[1],
            "POST",
            "/openai/v1/chat",
            &[("x-attr-team", "ml-research")],
            b"{}",
        );
        assert_eq!(resp.status, 200);
        assert_eq!(resp.header("x-echo-attr-cost"), Some("app-cost"));
        assert_eq!(resp.header("x-echo-attr-env"), Some("ml-prod"));
    });
}

// ---------------------------------------------- GB-7: two-hop role chain

/// Bedrock + STS config with the WEB-IDENTITY BASE HOP and per-request role
/// identity: role_arn templated on the pinned `env`, RoleSessionName
/// composed from `env` and the claim-proven `user`. The chained AssumeRole
/// must be SigV4-signed with the hop-1 base credentials.
fn gb7_chain_cfg(bedrock_port: u16, sts_port: u16, token_file: &str) -> String {
    format!(
        r#"
providers:
  bedrock-main:
    kind: bedrock
    upstream: {{ host: 127.0.0.1, port: {bedrock_port} }}
    sts:
      endpoint: {{ host: 127.0.0.1, port: {sts_port} }}
      role_arn: arn:aws:iam::000000000000:role/bedrock-{{{{env}}}}
      session_name: '{{{{env}}}}-{{{{user}}}}'
      region: us-east-1
      base:
        web_identity_token: {{ file: {token_file} }}
        role_arn: arn:aws:iam::000000000000:role/gatewayd-base
        sts_region: us-east-1
      tags:
        - {{ key: user, from_attribution: user }}
        - {{ key: env, from_attribution: env }}
routes:
  - prefix: /bedrock
    provider: bedrock-main
    attribution:
      required_keys: [user]
      pinned: {{ env: prod }}
      from_claims: {{ user: sub }}
auth:
  jwt:
    hs256_secret: conformance-secret
rejections:
  missing_attribution:
    status: 428
    content_type: application/json
    body: '{{"error":"attribution_required","missing":"{{{{key}}}}","route":"{{{{route}}}}"}}'
  unknown_route:
    status: 404
    content_type: application/json
    body: '{{"error":"unknown_route","path":"{{{{route}}}}"}}'
"#
    )
}

#[test]
fn gb7_two_hop_chain_signs_the_assume_role_call() {
    check("GB-7", "gb7_two_hop_chain_signs_the_assume_role_call", || {
        let p = ports(3);
        let token_file = std::env::temp_dir().join(format!("gb7-web-identity-{}.jwt", p[1]));
        std::fs::write(&token_file, "mock-oidc-token\n").expect("write token file");
        // --require-signed-chain: an unsigned chained AssumeRole would 403,
        // so a 200 end-to-end PROVES hop 1 minted base creds from the token
        // file and hop 2 arrived correctly SigV4-signed by them, with the
        // per-request role (bedrock-prod) and session name (prod-alice).
        let _sts = spawn_sts_signed(p[1]);
        let _mock = spawn_mock(p[0], &spike_fixture("bedrock.jsonl"), "bedrock", true);
        let _gw = spawn_gatewayd(
            &gb7_chain_cfg(p[0], p[1], token_file.to_str().unwrap()),
            p[2],
            "gb7chain",
        );
        let token = mint_jwt(serde_json::json!({"sub": "alice", "exp": 4102444800u64}));
        let auth = format!("Bearer {token}");
        let resp = http(
            p[2],
            "POST",
            "/bedrock/model/anthropic.claude/converse-stream",
            &[("authorization", auth.as_str())],
            b"{}",
        );
        assert_eq!(resp.status, 200, "body: {}", resp.body_text());
        // The tags still ride the TEAM credentials to the mock Bedrock.
        assert_eq!(resp.header("x-echo-session-tag-user"), Some("alice"));
        assert_eq!(resp.header("x-echo-session-tag-env"), Some("prod"));
        let _ = std::fs::remove_file(&token_file);
    });
}

#[test]
fn gb7_unsigned_chain_is_refused_by_signed_sts() {
    check("GB-7", "gb7_unsigned_chain_is_refused_by_signed_sts", || {
        // The vacuity guard for the test above: the SAME signed-mode mock
        // STS refuses a single-hop config's unsigned AssumeRole, so the
        // two-hop 200 cannot be explained by the mock not checking.
        let p = ports(3);
        let _sts = spawn_sts_signed(p[1]);
        let _mock = spawn_mock(p[0], &spike_fixture("bedrock.jsonl"), "bedrock", true);
        let _gw = spawn_gatewayd(&gb7_cfg(p[0], p[1]), p[2], "gb7unsigned");
        let token = mint_jwt(serde_json::json!({"sub": "alice", "exp": 4102444800u64}));
        let auth = format!("Bearer {token}");
        let resp = http(
            p[2],
            "POST",
            "/bedrock/model/anthropic.claude/converse-stream",
            &[("authorization", auth.as_str())],
            b"{}",
        );
        // The exchange fails (STS 403) -> gatewayd surfaces a 502, loudly.
        assert_eq!(resp.status, 502, "body: {}", resp.body_text());
    });
}

// ------------------------------------- operator-forced injection (guardrails)

#[test]
fn forced_guardrail_headers_are_signed_and_operator_wins() {
    check("INJECT", "forced_guardrail_headers_are_signed_and_operator_wins", || {
        // The gb7 config + an inject block forcing two guardrail headers.
        // The caller sends a FORGED guardrail identifier. The mock Bedrock
        // verifies SigV4 over the received SignedHeaders, so a 200 proves
        // three things at once: the forced headers ARRIVED, the operator's
        // values OVERRODE the caller's (a surviving forged value would
        // change the recomputed signature -> 403), and they were IN the
        // signed set (they are listed in SignedHeaders; absence -> "signed
        // header missing" -> 403).
        let p = ports(3);
        let cfg = gb7_cfg(p[0], p[1]).replace(
            "    sts:",
            concat!(
                "    inject:\n",
                "      headers:\n",
                "        - { name: x-amzn-bedrock-guardrailidentifier, value: gr-abc123 }\n",
                "        - { name: x-amzn-bedrock-guardrailversion, value: '7' }\n",
                "      body:\n",
                "        - { path: guardrailConfig.guardrailIdentifier, value: gr-abc123, if_absent: true }\n",
                "    sts:",
            ),
        );
        let _sts = spawn_sts(p[1]);
        let _mock = spawn_mock(p[0], &spike_fixture("bedrock.jsonl"), "bedrock", true);
        let _gw = spawn_gatewayd(&cfg, p[2], "inject1");
        let token = mint_jwt(serde_json::json!({"sub": "alice", "exp": 4102444800u64}));
        let auth = format!("Bearer {token}");
        let resp = http(
            p[2],
            "POST",
            "/bedrock/model/anthropic.claude/converse-stream",
            &[
                ("authorization", auth.as_str()),
                // The forged value the operator must overwrite.
                ("x-amzn-bedrock-guardrailidentifier", "gr-evil"),
            ],
            b"{}",
        );
        assert_eq!(resp.status, 200, "body: {}", resp.body_text());
        assert_eq!(resp.header("x-echo-session-tag-user"), Some("alice"));
    });
}

// -------------------------------------------------- GB-8: the auth chain

/// Vertex + the WIF auth chain: the gateway mints the Google credential
/// itself (token exchange -> generateAccessToken -> Bearer) and the labels
/// still ride the body. Both endpoints point at the one mock_gcp process.
fn gb8_auth_cfg(vertex_port: u16, gcp_port: u16, token_file: &str) -> String {
    format!(
        r#"
providers:
  vertex-main:
    kind: vertex
    upstream: {{ host: 127.0.0.1, port: {vertex_port} }}
    auth:
      web_identity_token: {{ file: {token_file} }}
      wif:
        project_number: "123456789"
        pool_id: gw-pool
        provider_id: gw-provider
      service_account_email: gateway@example-project.iam.gserviceaccount.com
      sts_endpoint: {{ host: 127.0.0.1, port: {gcp_port} }}
      iam_endpoint: {{ host: 127.0.0.1, port: {gcp_port} }}
fleet:
  attribution:
    required_keys: [team]
    pinned: {{ env: prod }}
routes:
  - prefix: /vertex
    provider: vertex-main
    labels:
      - key: team
        from_attribution: team
rejections:
  missing_attribution:
    status: 428
    content_type: application/json
    body: '{{"error":"attribution_required","missing":"{{{{key}}}}","route":"{{{{route}}}}"}}'
  unknown_route:
    status: 404
    content_type: application/json
    body: '{{"error":"unknown_route","path":"{{{{route}}}}"}}'
"#
    )
}

#[test]
fn gb8_auth_chain_mints_the_bearer_and_caches_it() {
    check("GB-8", "gb8_auth_chain_mints_the_bearer_and_caches_it", || {
        let p = ports(3);
        let token_file = std::env::temp_dir().join(format!("gb8-web-identity-{}.jwt", p[1]));
        std::fs::write(&token_file, "mock-oidc-token\n").expect("write token file");
        // The upstream REFUSES anything that is not a gateway-minted SA
        // bearer, and mock_gcp refuses misspelled URNs / a bad audience /
        // an integer lifetime — so a 200 proves the whole WIF chain spoke
        // the real wire shapes, and the caller's own Authorization (below,
        // a JWT-shaped decoy) never reached the upstream.
        let _gcp = spawn_gcp(p[1]);
        let _mock = spawn_mock_bearer(p[0], &demo_fixture("vertex.sse"), "vertex", "sa-token-");
        let _gw = spawn_gatewayd(
            &gb8_auth_cfg(p[0], p[1], token_file.to_str().unwrap()),
            p[2],
            "gb8auth",
        );
        let body = br#"{"contents":[{"parts":[{"text":"hi"}]}]}"#;
        let headers = [
            ("x-attr-team", "ml-research"),
            ("content-type", "application/json"),
            ("authorization", "Bearer caller-own-token"),
        ];
        let path = "/vertex/v1/models/gemini:streamGenerateContent";

        let r1 = http(p[2], "POST", path, &headers, body);
        assert_eq!(r1.status, 200, "body: {}", r1.body_text());
        let b1 = r1.header("x-echo-bearer").expect("bearer echoed").to_string();
        assert!(b1.starts_with("sa-token-"), "{b1}");
        // Labels still ride the body alongside the minted auth.
        assert_eq!(r1.header("x-echo-label-team"), Some("ml-research"));

        // Second request: the SA token comes from the cache — same bearer,
        // no second mint.
        let r2 = http(p[2], "POST", path, &headers, body);
        assert_eq!(r2.status, 200, "body: {}", r2.body_text());
        assert_eq!(r2.header("x-echo-bearer"), Some(b1.as_str()), "token cache");
        let _ = std::fs::remove_file(&token_file);
    });
}

#[test]
fn vertex_location_gate_allows_listed_and_refuses_others() {
    check("GB-8", "vertex_location_gate_allows_listed_and_refuses_others", || {
        let p = ports(3);
        let token_file = std::env::temp_dir().join(format!("gb8-loc-{}.jwt", p[1]));
        std::fs::write(&token_file, "mock-oidc-token\n").expect("write token file");
        let cfg = gb8_auth_cfg(p[0], p[1], token_file.to_str().unwrap()).replace(
            "    auth:",
            "    locations: [eu]\n    auth:",
        );
        let _gcp = spawn_gcp(p[1]);
        let _mock = spawn_mock_bearer(p[0], &demo_fixture("vertex.sse"), "vertex", "sa-token-");
        let _gw = spawn_gatewayd(&cfg, p[2], "gb8loc");
        let body = br#"{"contents":[{"parts":[{"text":"hi"}]}]}"#;
        let headers = [("x-attr-team", "ml"), ("content-type", "application/json")];

        // `eu` is a multi-region: the derived host is the configured base
        // (the mock), so the request flows.
        let ok = http(
            p[2],
            "POST",
            "/vertex/v1/projects/x/locations/eu/publishers/google/models/g:streamGenerateContent",
            &headers,
            body,
        );
        assert_eq!(ok.status, 200, "body: {}", ok.body_text());

        // A location outside the operator's list is refused with the GB-4
        // unknown_route body before anything is minted or forwarded.
        let bad = http(
            p[2],
            "POST",
            "/vertex/v1/projects/x/locations/mars/publishers/google/models/g:streamGenerateContent",
            &headers,
            body,
        );
        assert_eq!(bad.status, 404, "body: {}", bad.body_text());
        assert!(bad.body_text().contains("unknown_route"), "{}", bad.body_text());
        let _ = std::fs::remove_file(&token_file);
    });
}

// GB-5 / GB-6 conformance lives in its own file (size budget).
mod gb5;
