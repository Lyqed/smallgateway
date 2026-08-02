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
