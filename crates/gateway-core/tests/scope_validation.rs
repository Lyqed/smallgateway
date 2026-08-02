//! Validation-side tests for the scoped policy chain, driven through the
//! public `Config::from_yaml` surface (split out of `src/scope.rs` to keep
//! source files under the 800-line budget). The composition/precedence
//! suite lives in `tests/scope_precedence.rs`.

use gateway_core::config::{Config, ConfigError};

fn errors_of(yaml: &str) -> Vec<String> {
    match Config::from_yaml(yaml) {
        Err(ConfigError::Invalid(errs)) => errs,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

fn base_yaml() -> String {
    r#"
providers:
  openai-main:
    kind: openai
    upstream: { host: 127.0.0.1, port: 6190 }
routes:
  - prefix: /openai
    provider: openai-main
    attribution:
      required_keys: [team]
      pinned: { env: prod }
rejections:
  missing_attribution:
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
fn unknown_provider_ref_names_the_route_and_provider() {
    let yaml = base_yaml().replace("provider: openai-main", "provider: nope");
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("unknown provider \"nope\"")), "{errs:?}");
}

#[test]
fn unknown_project_ref_is_rejected() {
    let yaml = base_yaml().replace(
        "provider: openai-main",
        "provider: openai-main\n    project: ghost",
    );
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("unknown project \"ghost\"")), "{errs:?}");
}

#[test]
fn empty_required_key_is_rejected() {
    let yaml = base_yaml().replace("required_keys: [team]", "required_keys: [team, '']");
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("empty attribution key")), "{errs:?}");
}

#[test]
fn placeholder_typo_is_named() {
    let yaml = base_yaml().replace("{{key}} on {{route}}", "{{keys}} on {{route}}");
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("unknown placeholder {{keys}}")), "{errs:?}");
}

#[test]
fn key_placeholder_is_invalid_for_unknown_route() {
    let yaml = base_yaml().replace("no route for {{route}}", "no route for {{key}}");
    let errs = errors_of(&yaml);
    assert!(
        errs.iter().any(|e| e.contains("unknown_route.body") && e.contains("{{key}}")),
        "{errs:?}"
    );
}

#[test]
fn from_claims_without_auth_is_rejected() {
    let yaml = base_yaml().replace(
        "pinned: { env: prod }",
        "pinned: { env: prod }\n      from_claims: { user: sub }",
    );
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("requires auth.jwt")), "{errs:?}");
}

#[test]
fn same_scope_pinned_and_claim_mapped_key_conflict_is_rejected() {
    let yaml = base_yaml().replace(
        "pinned: { env: prod }",
        "pinned: { env: prod }\n      from_claims: { env: environment }",
    );
    let yaml = format!("{yaml}auth:\n  jwt:\n    hs256_secret: s\n");
    let errs = errors_of(&yaml);
    assert!(
        errs.iter().any(|e| e.contains("both pinned and claim-mapped")),
        "{errs:?}"
    );
}

#[test]
fn cross_scope_contradictory_pin_is_rejected() {
    // fleet pins env; the route claim-maps env: two origins, one key.
    let yaml = base_yaml().replace(
        "providers:",
        "fleet:\n  attribution:\n    pinned: { env: fleet-prod }\nproviders:",
    );
    let yaml = yaml.replace(
        "pinned: { env: prod }",
        "from_claims: { env: environment }",
    );
    let yaml = format!("{yaml}auth:\n  jwt:\n    hs256_secret: s\n");
    let errs = errors_of(&yaml);
    assert!(
        errs.iter().any(|e| e.contains("contradictory pin") && e.contains("\"env\"")),
        "{errs:?}"
    );
}

#[test]
fn uppercase_key_is_rejected_with_header_hint() {
    let yaml = base_yaml().replace("required_keys: [team]", "required_keys: [Team]");
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("x-attr-Team")), "{errs:?}");
}

#[test]
fn duplicate_unconditioned_prefixes_collide_even_with_trailing_slash() {
    let yaml = base_yaml().replace(
        "routes:",
        "routes:\n  - prefix: /openai/\n    provider: openai-main",
    );
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("duplicate prefix")), "{errs:?}");
}

#[test]
fn duplicate_prefix_with_condition_is_allowed() {
    let yaml = base_yaml().replace(
        "routes:",
        "routes:\n  - prefix: /openai\n    provider: openai-main\n    match: 'request.method == \"GET\"'",
    );
    assert!(Config::from_yaml(&yaml).is_ok());
}

#[test]
fn app_override_may_not_redefine_its_selector_key() {
    let yaml = base_yaml().replace(
        "rejections:",
        concat!(
            "apps:\n",
            "  key: team\n",
            "  overrides:\n",
            "    ml:\n",
            "      attribution:\n",
            "        pinned: { team: other }\n",
            "rejections:",
        ),
    );
    let errs = errors_of(&yaml);
    assert!(
        errs.iter().any(|e| e.contains("selector key")),
        "{errs:?}"
    );
}

#[test]
fn apps_selector_must_be_resolvable_on_every_route() {
    let yaml = base_yaml().replace(
        "rejections:",
        concat!(
            "apps:\n",
            "  key: costcenter\n",
            "  overrides:\n",
            "    cc1: {}\n",
            "rejections:",
        ),
    );
    let errs = errors_of(&yaml);
    assert!(
        errs.iter().any(|e| e.contains("never requires, pins")),
        "{errs:?}"
    );
}

#[test]
fn sts_on_non_bedrock_provider_is_rejected() {
    let yaml = base_yaml().replace(
        "    upstream: { host: 127.0.0.1, port: 6190 }",
        concat!(
            "    upstream: { host: 127.0.0.1, port: 6190 }\n",
            "    sts:\n",
            "      endpoint: { host: 127.0.0.1, port: 6199 }\n",
            "      role_arn: arn:aws:iam::1:role/gw\n",
            "      tags: [ { key: team, value: ml } ]",
        ),
    );
    let errs = errors_of(&yaml);
    assert!(
        errs.iter().any(|e| e.contains("bedrock-kind providers only")),
        "{errs:?}"
    );
}

#[test]
fn caller_raw_session_tag_is_rejected_statically() {
    // `team` is only a required key (caller origin) — a session tag
    // sourced from it is caller-raw and must fail validation.
    let yaml = base_yaml()
        .replace("kind: openai", "kind: bedrock")
        .replace(
            "    upstream: { host: 127.0.0.1, port: 6190 }",
            concat!(
                "    upstream: { host: 127.0.0.1, port: 6190 }\n",
                "    sts:\n",
                "      endpoint: { host: 127.0.0.1, port: 6199 }\n",
                "      role_arn: arn:aws:iam::1:role/gw\n",
                "      tags: [ { key: team, from_attribution: team } ]",
            ),
        );
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("never caller-raw")), "{errs:?}");
}

#[test]
fn labels_on_non_vertex_route_are_rejected() {
    let yaml = base_yaml().replace(
        "provider: openai-main",
        "provider: openai-main\n    labels: [ { key: team, value: ml } ]",
    );
    let errs = errors_of(&yaml);
    assert!(
        errs.iter().any(|e| e.contains("labels require a vertex-kind provider")),
        "{errs:?}"
    );
}

#[test]
fn label_referencing_unresolvable_attribution_key_is_rejected() {
    let yaml = base_yaml().replace("kind: openai", "kind: vertex").replace(
        "provider: openai-main",
        "provider: openai-main\n    labels: [ { key: costcenter, from_attribution: ghost } ]",
    );
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("could never resolve")), "{errs:?}");
}

#[test]
fn label_spec_must_set_exactly_one_source() {
    let yaml = base_yaml().replace("kind: openai", "kind: vertex").replace(
        "provider: openai-main",
        "provider: openai-main\n    labels: [ { key: k, value: v, from_attribution: team } ]",
    );
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("exactly one of")), "{errs:?}");

    let yaml = base_yaml().replace("kind: openai", "kind: vertex").replace(
        "provider: openai-main",
        "provider: openai-main\n    labels: [ { key: k } ]",
    );
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("exactly one of")), "{errs:?}");
}

#[test]
fn google_label_constraints_are_enforced_at_load() {
    for bad in [
        "labels: [ { key: Upper, value: v } ]",
        "labels: [ { key: 1digit, value: v } ]",
        "labels: [ { key: ok, value: 'Bad Value' } ]",
    ] {
        let yaml = base_yaml().replace("kind: openai", "kind: vertex").replace(
            "provider: openai-main",
            &format!("provider: openai-main\n    {bad}"),
        );
        let errs = errors_of(&yaml);
        assert!(
            errs.iter().any(|e| e.contains("Google Cloud") || e.contains("lowercase letter")),
            "{bad}: {errs:?}"
        );
    }
}

#[test]
fn bad_derived_cel_fails_config_load_with_scope_named() {
    let yaml = base_yaml().replace(
        "pinned: { env: prod }",
        "pinned: { env: prod }\n      derived: { team2: 'jwt.claims.' }",
    );
    let errs = errors_of(&yaml);
    assert!(
        errs.iter().any(|e| e.contains("derived \"team2\"") && e.contains("parse error")),
        "{errs:?}"
    );
}

// ---- GB-7 role material (role_arn / session_name templates + allow) ----

/// Swap the base config to a bedrock provider with an sts block whose body
/// is supplied by the caller (indented 6, under `sts:`).
fn bedrock_sts_yaml(sts_body: &str) -> String {
    base_yaml().replace("kind: openai", "kind: bedrock").replace(
        "    upstream: { host: 127.0.0.1, port: 6190 }",
        &format!(
            "    upstream: {{ host: 127.0.0.1, port: 6190 }}\n    sts:\n{sts_body}"
        ),
    )
}

#[test]
fn sts_role_template_on_gateway_established_key_is_accepted() {
    // `env` is pinned (gateway-established): a role/session template over it
    // is fine, and the bare-string template form parses like a plain string.
    let yaml = bedrock_sts_yaml(concat!(
        "      endpoint: { host: 127.0.0.1, port: 6199 }\n",
        "      role_arn: arn:aws:iam::1:role/gw-{{env}}\n",
        "      session_name: '{{env}}-batch'\n",
        "      tags: [ { key: env, from_attribution: env } ]",
    ));
    Config::from_yaml(&yaml).unwrap();
}

#[test]
fn sts_templates_privilege_no_key_names() {
    // The generality invariant: cost_center / tenant work exactly like any
    // other operator-chosen key — nothing hardcodes team/app/user.
    let yaml = bedrock_sts_yaml(concat!(
        "      endpoint: { host: 127.0.0.1, port: 6199 }\n",
        "      role_arn: arn:aws:iam::1:role/bedrock-{{cost_center}}\n",
        "      session_name: '{{tenant}}-{{cost_center}}'\n",
        "      tags: [ { key: cc, from_attribution: cost_center } ]",
    ))
    .replace(
        "pinned: { env: prod }",
        "pinned: { env: prod, cost_center: research, tenant: acme }",
    );
    Config::from_yaml(&yaml).unwrap();
}

#[test]
fn sts_role_template_on_caller_key_without_allow_is_rejected() {
    // `team` is required-only (caller-asserted): without an allow-list the
    // caller could steer which role is assumed. Statically refused.
    let yaml = bedrock_sts_yaml(concat!(
        "      endpoint: { host: 127.0.0.1, port: 6199 }\n",
        "      role_arn: arn:aws:iam::1:role/bedrock-{{team}}\n",
        "      tags: [ { key: env, from_attribution: env } ]",
    ));
    let errs = errors_of(&yaml);
    assert!(
        errs.iter().any(|e| e.contains("must never steer")),
        "{errs:?}"
    );
}

#[test]
fn sts_role_template_on_caller_key_with_allow_list_is_accepted() {
    // The APIM-parity affordance: the allow-list closes the value set, so a
    // caller picks WHICH operator-built role, never a new one.
    let yaml = bedrock_sts_yaml(concat!(
        "      endpoint: { host: 127.0.0.1, port: 6199 }\n",
        "      role_arn: arn:aws:iam::1:role/bedrock-{{team}}\n",
        "      allow: { key: team, values: [ml, web] }\n",
        "      tags: [ { key: env, from_attribution: env } ]",
    ));
    Config::from_yaml(&yaml).unwrap();
}

#[test]
fn sts_role_template_on_unknown_key_is_rejected() {
    let yaml = bedrock_sts_yaml(concat!(
        "      endpoint: { host: 127.0.0.1, port: 6199 }\n",
        "      role_arn: arn:aws:iam::1:role/gw-{{ghost}}\n",
        "      tags: [ { key: env, from_attribution: env } ]",
    ));
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("could never resolve")), "{errs:?}");
}

#[test]
fn sts_allow_key_unknown_is_rejected() {
    let yaml = bedrock_sts_yaml(concat!(
        "      endpoint: { host: 127.0.0.1, port: 6199 }\n",
        "      role_arn: arn:aws:iam::1:role/gw\n",
        "      allow: { key: ghost, values: [x] }\n",
        "      tags: [ { key: env, from_attribution: env } ]",
    ));
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("could never resolve")), "{errs:?}");
}

#[test]
fn sts_static_role_arn_must_look_like_a_role_arn() {
    let yaml = bedrock_sts_yaml(concat!(
        "      endpoint: { host: 127.0.0.1, port: 6199 }\n",
        "      role_arn: not-an-arn\n",
        "      tags: [ { key: env, from_attribution: env } ]",
    ));
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("not an IAM role ARN")), "{errs:?}");
}

#[test]
fn sts_static_session_name_charset_is_validated() {
    let yaml = bedrock_sts_yaml(concat!(
        "      endpoint: { host: 127.0.0.1, port: 6199 }\n",
        "      role_arn: arn:aws:iam::1:role/gw\n",
        "      session_name: 'bad name!'\n",
        "      tags: [ { key: env, from_attribution: env } ]",
    ));
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("AWS does not accept")), "{errs:?}");
}

#[test]
fn sts_operator_value_map_forms_parse() {
    // Explicit map forms: value on role_arn, from_attribution on
    // session_name (env is pinned, so gateway-established).
    let yaml = bedrock_sts_yaml(concat!(
        "      endpoint: { host: 127.0.0.1, port: 6199 }\n",
        "      role_arn: { value: 'arn:aws:iam::1:role/gw' }\n",
        "      session_name: { from_attribution: env }\n",
        "      tags: [ { key: env, from_attribution: env } ]",
    ));
    Config::from_yaml(&yaml).unwrap();
}

#[test]
fn sts_operator_value_with_both_sources_is_rejected() {
    let yaml = bedrock_sts_yaml(concat!(
        "      endpoint: { host: 127.0.0.1, port: 6199 }\n",
        "      role_arn: { value: 'arn:aws:iam::1:role/gw', from_attribution: env }\n",
        "      tags: [ { key: env, from_attribution: env } ]",
    ));
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("exactly one of")), "{errs:?}");
}

#[test]
fn sts_base_hop_caps_duration_at_one_hour() {
    // Role chaining: AWS caps a chained session at 3600s. A base hop with a
    // longer requested duration is a guaranteed live-STS ValidationError,
    // refused at config load instead.
    let yaml = bedrock_sts_yaml(concat!(
        "      endpoint: { host: 127.0.0.1, port: 6199 }\n",
        "      role_arn: arn:aws:iam::1:role/gw\n",
        "      duration_secs: 7200\n",
        "      base:\n",
        "        web_identity_token: { env: GW_TOKEN }\n",
        "        role_arn: arn:aws:iam::1:role/gw-base\n",
        "      tags: [ { key: env, from_attribution: env } ]",
    ));
    let errs = errors_of(&yaml);
    assert!(
        errs.iter().any(|e| e.contains("role") && e.contains("one hour")),
        "{errs:?}"
    );
}

#[test]
fn sts_live_aws_endpoint_without_base_is_rejected() {
    // The unsigned single hop only ever works against the mock pair; a
    // fleet pointing at real STS without the signed chain must hear it at
    // load, not as a 502 on every production request.
    let yaml = bedrock_sts_yaml(concat!(
        "      endpoint: { host: sts.us-east-1.amazonaws.com, port: 443, tls: true }\n",
        "      role_arn: arn:aws:iam::1:role/gw\n",
        "      tags: [ { key: env, from_attribution: env } ]",
    ));
    let errs = errors_of(&yaml);
    assert!(
        errs.iter()
            .any(|e| e.contains("live AWS STS") && e.contains("base")),
        "{errs:?}"
    );
}

#[test]
fn sts_base_token_source_is_exactly_one_of() {
    let yaml = bedrock_sts_yaml(concat!(
        "      endpoint: { host: 127.0.0.1, port: 6199 }\n",
        "      role_arn: arn:aws:iam::1:role/gw\n",
        "      base:\n",
        "        web_identity_token: { file: /run/token, env: GW_TOKEN }\n",
        "        role_arn: arn:aws:iam::1:role/gw-base\n",
        "      tags: [ { key: env, from_attribution: env } ]",
    ));
    let errs = errors_of(&yaml);
    assert!(
        errs.iter().any(|e| e.contains("exactly one of 'file' or 'env'")),
        "{errs:?}"
    );
}

// ---- Operator-forced injection (guardrails) ----

fn inject_yaml(inject_body: &str) -> String {
    base_yaml().replace(
        "    upstream: { host: 127.0.0.1, port: 6190 }",
        &format!("    upstream: {{ host: 127.0.0.1, port: 6190 }}\n    inject:\n{inject_body}"),
    )
}

#[test]
fn inject_static_and_established_template_values_are_accepted() {
    let yaml = inject_yaml(concat!(
        "      headers:\n",
        "        - { name: x-amzn-bedrock-guardrailidentifier, value: gr-abc }\n",
        "        - { name: x-policy-env, value: 'env-{{env}}' }\n",
        "      body:\n",
        "        - { path: guardrailConfig.guardrailIdentifier, value: gr-abc, if_absent: true }",
    ));
    Config::from_yaml(&yaml).unwrap();
}

#[test]
fn inject_template_on_caller_key_is_rejected() {
    // `team` is required-only (caller-asserted): a guardrail value derived
    // from it would let the caller pick the guardrail. No allow exception.
    let yaml = inject_yaml(concat!(
        "      headers:\n",
        "        - { name: x-guardrail, value: 'gr-{{team}}' }",
    ));
    let errs = errors_of(&yaml);
    assert!(
        errs.iter().any(|e| e.contains("never caller-steerable")),
        "{errs:?}"
    );
}

#[test]
fn inject_template_on_unknown_key_is_rejected() {
    let yaml = inject_yaml(concat!(
        "      body:\n",
        "        - { path: a.b, value: '{{ghost}}' }",
    ));
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("could never resolve")), "{errs:?}");
}

#[test]
fn inject_on_passthrough_bedrock_is_rejected() {
    // Shape 1 (no sts) is caller-signed: rewriting the body would break the
    // caller's payload hash at AWS, and forced headers would land outside
    // the signed set. The loader refuses the combination outright.
    let yaml = base_yaml().replace(
        "    kind: openai\n    upstream: { host: 127.0.0.1, port: 6190 }",
        concat!(
            "    kind: bedrock\n",
            "    upstream: { host: 127.0.0.1, port: 6190 }\n",
            "    inject:\n",
            "      headers:\n",
            "        - { name: x-amzn-bedrock-guardrailidentifier, value: gr-abc }",
        ),
    );
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("requires sts")), "{errs:?}");
}

#[test]
fn inject_reserved_headers_are_rejected() {
    for name in ["authorization", "Host", "x-amz-date", "content-length"] {
        let yaml = inject_yaml(&format!(
            "      headers:\n        - {{ name: {name}, value: v }}"
        ));
        let errs = errors_of(&yaml);
        assert!(
            errs.iter().any(|e| e.contains("owned by signing/transport")),
            "{name}: {errs:?}"
        );
    }
}

#[test]
fn inject_empty_path_segment_is_rejected() {
    let yaml = inject_yaml(concat!(
        "      body:\n",
        "        - { path: 'a..b', value: v }",
    ));
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("dotted segments")), "{errs:?}");
}

// ---- GB-5 windows + alert_at ----

// ---- Dedicated refusal templates (value_not_allowed / cap_exceeded) ----

#[test]
fn dedicated_refusal_templates_compose_and_absent_means_fallback() {
    let yaml = base_yaml().replace(
        "rejections:",
        concat!(
            "rejections:\n",
            "  value_not_allowed:\n",
            "    status: 403\n",
            "    content_type: application/json\n",
            "    body: '{\"key\":\"{{key}}\",\"value\":\"{{value}}\",\"route\":\"{{route}}\"}'\n",
            "  cap_exceeded:\n",
            "    status: 429\n",
            "    content_type: application/json\n",
            "    body: '{\"who\":\"{{key}}\",\"spent\":\"{{spend}}\",\"cap\":\"{{cap}}\"}'",
        ),
    );
    let cfg = Config::from_yaml(&yaml).unwrap();
    let policy = cfg.routes[0].policy();
    assert_eq!(policy.value_not_allowed.as_ref().unwrap().status, 403);
    assert_eq!(policy.cap_exceeded.as_ref().unwrap().status, 429);

    // Absent, both stay None: the enforcement sites fall back to
    // missing_attribution (exercised end-to-end in conformance).
    let plain = Config::from_yaml(&base_yaml()).unwrap();
    let policy = plain.routes[0].policy();
    assert!(policy.value_not_allowed.is_none());
    assert!(policy.cap_exceeded.is_none());
}

#[test]
fn dedicated_templates_refuse_foreign_placeholders() {
    // {{value}} belongs to value_not_allowed; cap_exceeded must refuse it.
    let yaml = base_yaml().replace(
        "rejections:",
        concat!(
            "rejections:\n",
            "  cap_exceeded:\n",
            "    status: 429\n",
            "    content_type: application/json\n",
            "    body: '{\"v\":\"{{value}}\"}'",
        ),
    );
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("cap_exceeded")), "{errs:?}");
}

#[test]
fn spend_cap_window_and_alert_at_parse() {
    let yaml = base_yaml().replace(
        "pinned: { env: prod }",
        "pinned: { env: prod }\n      spend_caps: { team: { default: 120000, window: minute, alert_at: 70 } }",
    );
    let cfg = Config::from_yaml(&yaml).unwrap();
    let policy = cfg.routes[0].policy();
    let terms = policy.terms_for("team", "anything").unwrap();
    assert_eq!(terms.cap, 120000);
    assert_eq!(terms.window, Some(gateway_core::budget::Window::Minute));
    assert!((terms.alert_fraction - 0.7).abs() < 1e-9);
}

#[test]
fn spend_cap_alert_at_out_of_range_is_rejected() {
    for bad in ["0", "101"] {
        let yaml = base_yaml().replace(
            "pinned: { env: prod }",
            &format!(
                "pinned: {{ env: prod }}\n      spend_caps: {{ team: {{ default: 1000, alert_at: {bad} }} }}"
            ),
        );
        let errs = errors_of(&yaml);
        assert!(errs.iter().any(|e| e.contains("alert_at must be 1-100")), "{bad}: {errs:?}");
    }
}

#[test]
fn spend_cap_bad_window_is_a_parse_error() {
    let yaml = base_yaml().replace(
        "pinned: { env: prod }",
        "pinned: { env: prod }\n      spend_caps: { team: { default: 1000, window: fortnight } }",
    );
    assert!(matches!(Config::from_yaml(&yaml), Err(ConfigError::Parse(_))));
}

// ---- GB-8 auth chain ----

#[test]
fn vertex_auth_on_non_vertex_provider_is_rejected() {
    let yaml = base_yaml().replace(
        "    upstream: { host: 127.0.0.1, port: 6190 }",
        concat!(
            "    upstream: { host: 127.0.0.1, port: 6190 }\n",
            "    auth:\n",
            "      web_identity_token: { env: GW_TOKEN }\n",
            "      wif: { project_number: '1', pool_id: p, provider_id: pr }\n",
            "      service_account_email: gw@x.iam.gserviceaccount.com\n",
            "      sts_endpoint: { host: 127.0.0.1, port: 6197 }\n",
            "      iam_endpoint: { host: 127.0.0.1, port: 6197 }",
        ),
    );
    let errs = errors_of(&yaml);
    assert!(
        errs.iter().any(|e| e.contains("vertex-kind providers only")),
        "{errs:?}"
    );
}

#[test]
fn vertex_auth_lifetime_and_email_are_validated() {
    let yaml = base_yaml().replace("kind: openai", "kind: vertex").replace(
        "    upstream: { host: 127.0.0.1, port: 6190 }",
        concat!(
            "    upstream: { host: 127.0.0.1, port: 6190 }\n",
            "    auth:\n",
            "      web_identity_token: { env: GW_TOKEN }\n",
            "      wif: { project_number: '1', pool_id: p, provider_id: pr }\n",
            "      service_account_email: not-an-email\n",
            "      lifetime_secs: 7200\n",
            "      sts_endpoint: { host: 127.0.0.1, port: 6197 }\n",
            "      iam_endpoint: { host: 127.0.0.1, port: 6197 }",
        ),
    );
    let errs = errors_of(&yaml);
    assert!(errs.iter().any(|e| e.contains("not an email")), "{errs:?}");
    assert!(errs.iter().any(|e| e.contains("lifetime_secs must be 300-3600")), "{errs:?}");
}

// ---- Vertex location routing ----

#[test]
fn vertex_host_derivation_is_multi_region_aware() {
    use gateway_core::config::{derive_vertex_host, vertex_path_location};
    assert_eq!(derive_vertex_host("eu", "aiplatform.googleapis.com"), "aiplatform.googleapis.com");
    assert_eq!(derive_vertex_host("global", "aiplatform.googleapis.com"), "aiplatform.googleapis.com");
    assert_eq!(
        derive_vertex_host("europe-west3", "aiplatform.googleapis.com"),
        "europe-west3-aiplatform.googleapis.com"
    );
    assert_eq!(
        vertex_path_location("/v1/projects/p/locations/europe-west3/publishers/google/models/g:streamGenerateContent"),
        Some("europe-west3")
    );
    assert_eq!(vertex_path_location("/v1/projects/p/locations//x"), None);
    assert_eq!(vertex_path_location("/v1/no-location-here"), None);
}

#[test]
fn locations_on_non_vertex_provider_is_rejected() {
    let yaml = base_yaml().replace(
        "    upstream: { host: 127.0.0.1, port: 6190 }",
        "    upstream: { host: 127.0.0.1, port: 6190 }\n    locations: [eu, europe-west3]",
    );
    let errs = errors_of(&yaml);
    assert!(
        errs.iter().any(|e| e.contains("vertex-kind providers only")),
        "{errs:?}"
    );
}

// ---- GB-2 auth sources ----

#[test]
fn auth_requires_exactly_one_of_secret_or_jwks() {
    let both = base_yaml()
        + "auth:\n  jwt:\n    hs256_secret: s\n    jwks: '{\"keys\":[]}'\n";
    let errs = errors_of(&both);
    assert!(errs.iter().any(|e| e.contains("exactly one of 'hs256_secret'")), "{errs:?}");

    let bad_jwks = base_yaml() + "auth:\n  jwt:\n    jwks: 'not json'\n";
    let errs = errors_of(&bad_jwks);
    assert!(errs.iter().any(|e| e.contains("jwks is not JSON")), "{errs:?}");
}

// ---- Model gate ----

#[test]
fn model_matching_and_extraction() {
    use gateway_core::config::{model_allowed, model_from_body, model_from_path, ProviderKind};
    let allow = vec!["gpt-4o".to_string(), "claude-3*".to_string()];
    assert!(model_allowed(&allow, "gpt-4o"));
    assert!(model_allowed(&allow, "claude-3-haiku"));
    assert!(!model_allowed(&allow, "gpt-4o-mini"));
    assert!(!model_allowed(&allow, "o1"));

    assert_eq!(
        model_from_path(ProviderKind::Bedrock, "/model/anthropic.claude-3/converse-stream"),
        Some("anthropic.claude-3".to_string())
    );
    assert_eq!(
        model_from_path(ProviderKind::Vertex, "/v1/projects/p/locations/eu/publishers/google/models/gemini-pro:streamGenerateContent"),
        Some("gemini-pro".to_string())
    );
    assert_eq!(model_from_path(ProviderKind::OpenAi, "/v1/chat/completions"), None);
    assert_eq!(model_from_body(br#"{"model":"gpt-4o","messages":[]}"#), Some("gpt-4o".to_string()));
    assert_eq!(model_from_body(br#"{"messages":[]}"#), None);
    assert_eq!(model_from_body(b"not json"), None);
}

#[test]
fn models_list_composes_by_replacement_and_validates() {
    // Route's list REPLACES the fleet's (narrowing must not merge).
    let yaml = base_yaml().replace(
        "routes:",
        "fleet:\n  attribution:\n    models: [gpt-4o, claude-3*]\nroutes:",
    ).replace(
        "      pinned: { env: prod }",
        "      pinned: { env: prod }\n      models: [gpt-4o]",
    );
    let cfg = Config::from_yaml(&yaml).unwrap();
    let policy = cfg.routes[0].policy();
    assert_eq!(policy.models.as_deref(), Some(&["gpt-4o".to_string()][..]));
    // The built-in refusal default applies when the operator sets none.
    assert_eq!(policy.model_not_allowed.status, 403);
    assert!(policy.model_not_allowed.body.contains("{{model}}"));

    // Bad entries are refused at load.
    let bad = base_yaml().replace(
        "      pinned: { env: prod }",
        "      pinned: { env: prod }\n      models: ['a*b']",
    );
    let errs = errors_of(&bad);
    assert!(errs.iter().any(|e| e.contains("trailing-* family pattern")), "{errs:?}");
}

// ---- The Getting Started examples must never rot ----

/// Extract the full `gateway.yaml` fenced blocks from the doc (the ones
/// whose first line is a `# gateway.yaml` comment) and load-validate each
/// through the real pipeline. A doc example a reader cannot paste and run
/// is worse than no example.
#[test]
fn getting_started_examples_load_and_validate() {
    let doc = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/10-getting-started.md"),
    )
    .expect("read docs/10-getting-started.md");
    let mut found = 0;
    for block in doc.split("```yaml").skip(1) {
        let yaml = block.split("```").next().unwrap_or("");
        // Native flat configs only: the k8s CR example also carries a
        // `# gateway.yaml` comment but is an apiVersion'd manifest the
        // operator (not Config::from_yaml) consumes.
        if !yaml.trim_start().starts_with("# gateway.yaml") || yaml.contains("apiVersion:") {
            continue;
        }
        found += 1;
        if let Err(e) = Config::from_yaml(yaml) {
            panic!("getting-started example {found} does not validate: {e:?}");
        }
    }
    assert!(found >= 2, "expected the Bedrock and Vertex examples, found {found}");
}
