//! Exhaustive precedence tests for the scoped policy chain
//! (fleet → project → route → app), driven through the public
//! `Config::from_yaml` surface — what an operator's file actually does.
//!
//! The semantics under test (docs/02 + `scope.rs` module docs):
//! - lists: absent inherits, present-without-`<base>` replaces,
//!   `<base>` splices the parent at the marker's position;
//! - maps: lower scope wins within one origin; cross-origin = error;
//! - rejection templates: per-reason override down the chain;
//! - labels: same list semantics, deeper scope wins a key clash;
//! - apps compose after route, keyed by a resolved attribution value.

use gateway_core::config::Config;
use gateway_core::scope::{EffectivePolicy, LabelValue};

/// Full four-scope config; individual tests override pieces via format
/// arguments kept minimal on purpose.
fn four_scope_yaml(project_required: &str, app_required: &str) -> String {
    r#"
providers:
  vertex-main:
    kind: vertex
    upstream: { host: 127.0.0.1, port: 6190 }
fleet:
  attribution:
    required_keys: [team]
    headers: { team: x-attr-team, costcenter: x-attr-costcenter, purpose: x-attr-purpose }
    pinned: { env: fleet-env, region: eu }
  labels:
    - key: env
      from_attribution: env
projects:
  ml:
    attribution:
      required_keys: @project_required@
      pinned: { env: project-env }
routes:
  - prefix: /vertex
    provider: vertex-main
    project: ml
    attribution:
      pinned: { cost: route-cost }
apps:
  key: team
  overrides:
    ml-research:
      attribution:
        required_keys: @app_required@
        pinned: { cost: app-cost }
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
    .replace("@project_required@", project_required)
    .replace("@app_required@", app_required)
}

fn policy(yaml: &str) -> EffectivePolicy {
    let cfg = Config::from_yaml(yaml).unwrap();
    cfg.routes[0].policy().clone()
}

// ---------------------------------------------------------------- lists

#[test]
fn absent_list_inherits_the_parent() {
    // Project and route specify no required_keys → fleet's [team] flows down.
    let p = policy(&four_scope_yaml("[]", "[]"));
    assert_eq!(p.required_keys, vec!["team"]);
}

#[test]
fn list_without_base_marker_replaces_the_parent() {
    // APIM semantics: leaving out <base> drops the inherited chain. (The
    // apps block is removed here: replacing [team] would otherwise trip
    // the — correct — "selector never resolvable" validation.)
    let yaml = four_scope_yaml("[costcenter]", "[]");
    let start = yaml.find("apps:").unwrap();
    let end = yaml.find("rejections:").unwrap();
    let yaml = format!("{}{}", &yaml[..start], &yaml[end..]);
    let p = policy(&yaml);
    assert_eq!(p.required_keys, vec!["costcenter"]);
}

#[test]
fn base_marker_splices_the_parent_at_its_position() {
    let p = policy(&four_scope_yaml("[costcenter, '<base>', purpose]", "[]"));
    assert_eq!(p.required_keys, vec!["costcenter", "team", "purpose"]);
}

#[test]
fn splice_dedups_keeping_the_first_occurrence() {
    // team appears both in the child and (via <base>) the parent.
    let p = policy(&four_scope_yaml("[team, '<base>']", "[]"));
    assert_eq!(p.required_keys, vec!["team"]);
}

#[test]
fn app_layer_list_composes_after_route() {
    let cfg = Config::from_yaml(&four_scope_yaml("[costcenter, '<base>']", "['<base>', purpose]"))
        .unwrap();
    let app = cfg.routes[0].app_policy("ml-research").unwrap();
    assert_eq!(app.required_keys, vec!["costcenter", "team", "purpose"]);
    // The base (no-app) chain is unchanged.
    assert_eq!(cfg.routes[0].policy().required_keys, vec!["costcenter", "team"]);
}

// ----------------------------------------------------------------- maps

#[test]
fn lower_scope_wins_within_the_same_origin() {
    let cfg = Config::from_yaml(&four_scope_yaml("[]", "[]")).unwrap();
    let p = cfg.routes[0].policy();
    // fleet pinned env=fleet-env, project overrode env=project-env.
    assert_eq!(p.pinned["env"], "project-env");
    // fleet's untouched pin survives.
    assert_eq!(p.pinned["region"], "eu");
    // route's own pin lands.
    assert_eq!(p.pinned["cost"], "route-cost");
    // app layer overrides the route's pin — deepest wins.
    let app = cfg.routes[0].app_policy("ml-research").unwrap();
    assert_eq!(app.pinned["cost"], "app-cost");
    assert_eq!(app.pinned["env"], "project-env");
}

#[test]
fn unknown_app_value_has_no_override_policy() {
    let cfg = Config::from_yaml(&four_scope_yaml("[]", "[]")).unwrap();
    assert!(cfg.routes[0].app_policy("some-other-team").is_none());
}

#[test]
fn cross_origin_conflict_across_scopes_is_a_contradictory_pin() {
    // fleet pins env; app claim-maps env → rejected with both scopes named.
    let yaml = four_scope_yaml("[]", "[]").replace(
        "pinned: { cost: app-cost }",
        "pinned: { cost: app-cost }\n        from_claims: { env: environment }",
    ) + "auth:\n  jwt:\n    hs256_secret: s\n";
    let err = Config::from_yaml(&yaml).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("contradictory pin"), "{msg}");
    assert!(msg.contains("\"env\""), "{msg}");
    assert!(msg.contains("app \"ml-research\""), "{msg}");
}

#[test]
fn derived_vs_pinned_across_scopes_is_also_contradictory() {
    let yaml = four_scope_yaml("[]", "[]").replace(
        "pinned: { cost: route-cost }",
        "pinned: { cost: route-cost }\n      derived: { region: 'request.path' }",
    );
    let err = Config::from_yaml(&yaml).unwrap_err().to_string();
    assert!(err.contains("contradictory pin"), "{err}");
    assert!(err.contains("\"region\""), "{err}");
}

// ------------------------------------------------------------- rejections

#[test]
fn rejection_templates_override_per_reason_down_the_chain() {
    let yaml = four_scope_yaml("[]", "[]")
        .replace(
            "projects:\n  ml:\n    attribution:",
            concat!(
                "projects:\n  ml:\n    rejections:\n",
                "      missing_attribution:\n",
                "        status: 451\n",
                "        content_type: text/plain\n",
                "        body: 'project says no: {{key}}'\n",
                "    attribution:",
            ),
        )
        .replace(
            "      attribution:\n        required_keys: []\n        pinned: { cost: app-cost }",
            concat!(
                "      rejections:\n",
                "        missing_attribution:\n",
                "          status: 452\n",
                "          content_type: text/plain\n",
                "          body: 'app says no: {{key}}'\n",
                "      attribution:\n",
                "        required_keys: []\n",
                "        pinned: { cost: app-cost }",
            ),
        );
    let cfg = Config::from_yaml(&yaml).unwrap();
    let p = cfg.routes[0].policy();
    // Project override applies to the route chain...
    assert_eq!(p.missing_attribution.status, 451);
    // ...the other reason still comes from fleet scope.
    assert_eq!(p.unknown_route.status, 404);
    // The app layer overrides again.
    let app = cfg.routes[0].app_policy("ml-research").unwrap();
    assert_eq!(app.missing_attribution.status, 452);
    assert_eq!(app.unknown_route.status, 404);
}

// ---------------------------------------------------------------- labels

#[test]
fn labels_inherit_replace_and_splice_like_required_keys() {
    // Route defines labels WITHOUT <base>: fleet's label list is replaced.
    let yaml = four_scope_yaml("[]", "[]").replace(
        "    attribution:\n      pinned: { cost: route-cost }",
        concat!(
            "    labels:\n",
            "      - key: costcenter\n",
            "        value: platform\n",
            "    attribution:\n      pinned: { cost: route-cost }",
        ),
    );
    let p = policy(&yaml);
    let keys: Vec<&str> = p.labels.iter().map(|l| l.key.as_str()).collect();
    assert_eq!(keys, vec!["costcenter"]);

    // With <base> the fleet label is spliced in after the route's own.
    let yaml = yaml.replace(
        "      - key: costcenter\n        value: platform\n",
        "      - key: costcenter\n        value: platform\n      - '<base>'\n",
    );
    let p = policy(&yaml);
    let keys: Vec<&str> = p.labels.iter().map(|l| l.key.as_str()).collect();
    assert_eq!(keys, vec!["costcenter", "env"]);
}

#[test]
fn deeper_scope_wins_a_label_key_clash() {
    // Fleet defines env→from_attribution; the route redefines env as a
    // static value and splices the base — the route's entry survives.
    let yaml = four_scope_yaml("[]", "[]").replace(
        "    attribution:\n      pinned: { cost: route-cost }",
        concat!(
            "    labels:\n",
            "      - key: env\n",
            "        value: route-static\n",
            "      - '<base>'\n",
            "    attribution:\n      pinned: { cost: route-cost }",
        ),
    );
    let p = policy(&yaml);
    assert_eq!(p.labels.len(), 1);
    assert_eq!(p.labels[0].key, "env");
    assert!(matches!(&p.labels[0].value, LabelValue::Static(v) if v == "route-static"));
}

// ------------------------------------------------------------ app scoping

#[test]
fn app_selector_value_keys_the_override_and_base_chain_is_untouched() {
    let cfg = Config::from_yaml(&four_scope_yaml("[]", "[]")).unwrap();
    let route = &cfg.routes[0];
    // Exactly one override, keyed by the attribution VALUE of `team`.
    assert!(route.app_policy("ml-research").is_some());
    assert!(route.app_policy("ml").is_none());
    assert_eq!(route.policy().pinned["cost"], "route-cost");
}
