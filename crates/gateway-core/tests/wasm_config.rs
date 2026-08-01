//! The tier-2 WASM config block (Phase 4): parse + structural validation.
//! The types live in `gateway_core::wasm_config`; validation in
//! `gateway_core::validate::validate_wasm`. Signature PRESENCE/MATCH is an
//! admission/load concern, not a structural one, so it is NOT checked here.

use gateway_core::config::{Config, ConfigError, WasmHook};

/// A minimal valid config the tests splice a `wasm:` block into.
fn valid_yaml() -> String {
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
fn wasm_block_parses_and_defaults() {
    let yaml = valid_yaml().replace(
        "rejections:",
        concat!(
            "wasm:\n",
            "  modules:\n",
            "    - name: header-policy\n",
            "      source: modules/header.wat\n",
            "      signature: \"abc123\"\n",
            "      hooks: [on_request, on_response_end]\n",
            "rejections:",
        ),
    );
    let cfg = Config::from_yaml(&yaml).unwrap();
    assert_eq!(cfg.wasm.modules.len(), 1);
    let m = &cfg.wasm.modules[0];
    assert_eq!(m.name, "header-policy");
    assert_eq!(m.schema, 1, "schema defaults to 1");
    assert!(!cfg.wasm.per_event_hooks, "per-event hooks default OFF (measured gate)");
    assert_eq!(m.hooks, vec![WasmHook::OnRequest, WasmHook::OnResponseEnd]);
}

#[test]
fn wasm_module_with_no_hooks_fails_validation() {
    let yaml = valid_yaml().replace(
        "rejections:",
        concat!(
            "wasm:\n",
            "  modules:\n",
            "    - name: empty\n",
            "      source: modules/x.wat\n",
            "      signature: \"abc\"\n",
            "      hooks: []\n",
            "rejections:",
        ),
    );
    match Config::from_yaml(&yaml) {
        Err(ConfigError::Invalid(errs)) => {
            assert!(errs.iter().any(|e| e.contains("no hooks")), "{errs:?}")
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn duplicate_wasm_module_names_fail_validation() {
    let yaml = valid_yaml().replace(
        "rejections:",
        concat!(
            "wasm:\n",
            "  modules:\n",
            "    - name: dup\n",
            "      source: a.wat\n",
            "      signature: s\n",
            "      hooks: [on_request]\n",
            "    - name: dup\n",
            "      source: b.wat\n",
            "      signature: s\n",
            "      hooks: [on_request]\n",
            "rejections:",
        ),
    );
    match Config::from_yaml(&yaml) {
        Err(ConfigError::Invalid(errs)) => {
            assert!(errs.iter().any(|e| e.contains("duplicate module name")), "{errs:?}")
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}
