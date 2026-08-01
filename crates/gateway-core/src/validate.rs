//! Field-level validation for the config file: providers (GB-7 sts
//! blocks included), auth, attribution keys, GB-8 label constraints
//! (Google Cloud's rules), GB-7 session-tag constraints (AWS's rules),
//! and GB-4 rejection-template placeholders.
//!
//! Split from [`crate::scope`], which owns the chain COMPOSITION and the
//! cross-scope checks; everything here validates one field or one scope
//! in isolation. All errors accumulate — an operator fixes the file once.

use std::collections::BTreeSet;

use crate::config::{
    Config, ProviderKind, RejectionOverrides, RejectionTemplate, Rejections, ATTR_HEADER_PREFIX,
};
use crate::scope::{
    MAX_LABEL_KEY_LEN, MAX_LABEL_VALUE_LEN, MAX_SESSION_TAG_KEY_LEN, MAX_SESSION_TAG_VALUE_LEN,
};
use crate::template;

pub(crate) fn validate_providers(cfg: &Config, errs: &mut Vec<String>) {
    if cfg.providers.is_empty() {
        errs.push("providers: at least one provider is required".to_string());
    }
    for (name, p) in &cfg.providers {
        if name.trim().is_empty() {
            errs.push("providers: provider name must not be empty".to_string());
        }
        if p.upstream.host.trim().is_empty() {
            errs.push(format!("provider {name:?}: upstream.host must not be empty"));
        }
        if p.upstream.port == 0 {
            errs.push(format!("provider {name:?}: upstream.port must be 1-65535"));
        }
        if let Some(sts) = &p.sts {
            let ctx = format!("provider {name:?}: sts");
            if p.kind != ProviderKind::Bedrock {
                errs.push(format!(
                    "{ctx}: session-tag credentials apply to bedrock-kind providers only \
                     ({name:?} is {})",
                    p.kind.name()
                ));
            }
            if sts.role_arn.trim().is_empty() {
                errs.push(format!("{ctx}: role_arn must not be empty"));
            }
            if sts.endpoint.host.trim().is_empty() {
                errs.push(format!("{ctx}: endpoint.host must not be empty"));
            }
            if sts.endpoint.port == 0 {
                errs.push(format!("{ctx}: endpoint.port must be 1-65535"));
            }
            if !(900..=43_200).contains(&sts.duration_secs) {
                errs.push(format!(
                    "{ctx}: duration_secs must be 900-43200 (STS limits), got {}",
                    sts.duration_secs
                ));
            }
            if sts.tags.is_empty() {
                errs.push(format!(
                    "{ctx}: at least one session tag is required (GB-7 exists to \
                     put attribution ON the credentials)"
                ));
            }
            let mut seen = BTreeSet::new();
            for tag in &sts.tags {
                let tctx = format!("{ctx}: tag {:?}", tag.key);
                if let Err(e) = validate_session_tag_key(&tag.key) {
                    errs.push(format!("{tctx}: {e}"));
                }
                if !seen.insert(tag.key.as_str()) {
                    errs.push(format!("{tctx}: duplicate tag key"));
                }
                match (&tag.value, &tag.from_attribution) {
                    (Some(v), None) => {
                        if let Err(e) = validate_session_tag_value(v) {
                            errs.push(format!("{tctx}: {e}"));
                        }
                    }
                    (None, Some(k)) => check_key(k, &format!("{tctx}: from_attribution"), errs),
                    _ => errs.push(format!(
                        "{tctx}: exactly one of 'value' or 'from_attribution' must be set"
                    )),
                }
            }
        }
    }
}

pub(crate) fn validate_auth(cfg: &Config, errs: &mut Vec<String>) {
    if let Some(auth) = &cfg.auth {
        if auth.jwt.hs256_secret.is_empty() {
            errs.push("auth.jwt.hs256_secret must not be empty".to_string());
        }
        if auth.jwt.header.trim().is_empty() {
            errs.push("auth.jwt.header must not be empty".to_string());
        }
    }
}

/// Tier-2 (Phase 4): validate the declared WASM module set. The load-time
/// half of "no unsigned WASM module" (docs/02 admission slot): every module
/// MUST carry a signature and at least one hook, names must be unique, and a
/// module declaring `on_response_event` while per-event hooks are disabled is
/// admitted but flagged (its hook will not run — a warning, not an error, so
/// an operator can stage a module ahead of enabling the gate). The SIGNATURE
/// itself is verified by the data plane against the operator key
/// (`gateway_wasm::sig::verify`); this crate has no key and no wasm runtime,
/// so it enforces PRESENCE, and the control plane's admission gate verifies
/// the cryptographic match before render.
pub(crate) fn validate_wasm(cfg: &Config, errs: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    for module in &cfg.wasm.modules {
        let ctx = format!("wasm module {:?}", module.name);
        if module.name.trim().is_empty() {
            errs.push("wasm module has an empty name".to_string());
        } else if !seen.insert(module.name.as_str()) {
            errs.push(format!("{ctx}: duplicate module name"));
        }
        if module.source.trim().is_empty() {
            errs.push(format!("{ctx}: empty source path"));
        }
        // NOTE: signature PRESENCE and cryptographic MATCH are enforced by the
        // admission gate (`gatewayctl`, the "no unsigned WASM module" rule) and,
        // defense in depth, by the data-plane loader (`gateway_wasm::sig::verify`
        // against the operator key) — not here: this crate holds no signing key
        // and links no wasm runtime. Structural validity is all it can check.
        if module.hooks.is_empty() {
            errs.push(format!(
                "{ctx}: declares no hooks — a module must implement at least one of \
                 on_request/on_response_event/on_response_end"
            ));
        }
        if module.schema == 0 {
            errs.push(format!("{ctx}: schema version must be >= 1"));
        }
    }
}

pub(crate) fn validate_rejections(r: &Rejections, ctx: &str, errs: &mut Vec<String>) {
    validate_template(
        &r.missing_attribution,
        &format!("{ctx}.missing_attribution"),
        // GB-1/GB-4: {{key}}/{{route}}. GB-5 adds the optional {{cap}}/{{spend}}
        // (tokens) so a budget rejection or a cut stream's terminal event can
        // name the exhausted limit; a template that omits them is still valid.
        &["key", "route", "cap", "spend"],
        errs,
    );
    validate_template(&r.unknown_route, &format!("{ctx}.unknown_route"), &["route"], errs);
}

pub(crate) fn validate_rejection_overrides(o: &RejectionOverrides, ctx: &str, errs: &mut Vec<String>) {
    if let Some(t) = &o.missing_attribution {
        validate_template(
            t,
            &format!("{ctx}.missing_attribution"),
            &["key", "route", "cap", "spend"],
            errs,
        );
    }
    if let Some(t) = &o.unknown_route {
        validate_template(t, &format!("{ctx}.unknown_route"), &["route"], errs);
    }
}

/// Keys become `x-attr-<key>` header names, so the charset is the safe
/// header subset — reject at load rather than panic at request time.
pub(crate) fn check_key(key: &str, ctx: &str, errs: &mut Vec<String>) {
    if key.is_empty() {
        errs.push(format!("{ctx}: empty attribution key"));
        return;
    }
    let ok = key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !ok {
        errs.push(format!(
            "{ctx}: attribution key {key:?} must be lowercase [a-z0-9_-] \
             (it becomes the {ATTR_HEADER_PREFIX}{key} header)"
        ));
    }
}

/// Google Cloud label key: 1-63 chars, lowercase letter first, then
/// lowercase letters, digits, `_`, `-`.
pub fn validate_label_key(key: &str) -> Result<(), String> {
    let len = key.chars().count();
    if !(1..=MAX_LABEL_KEY_LEN).contains(&len) {
        return Err(format!("label key must be 1-{MAX_LABEL_KEY_LEN} characters, got {len}"));
    }
    let mut chars = key.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() {
        return Err(format!("label key {key:?} must start with a lowercase letter"));
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
        return Err(format!(
            "label key {key:?} contains characters Google Cloud does not accept"
        ));
    }
    Ok(())
}

/// Google Cloud label value: 0-63 chars of lowercase letters, digits, `_`,
/// `-`. Applied to static values at load and to attribution-derived / CEL
/// values per request (fail closed).
pub fn validate_label_value(value: &str) -> Result<(), String> {
    let len = value.chars().count();
    if len > MAX_LABEL_VALUE_LEN {
        return Err(format!(
            "label value must be at most {MAX_LABEL_VALUE_LEN} characters, got {len}"
        ));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return Err(format!(
            "label value {value:?} contains characters Google Cloud does not accept"
        ));
    }
    Ok(())
}

/// AWS session tag key: 1-128 chars of the AWS tag charset.
pub fn validate_session_tag_key(key: &str) -> Result<(), String> {
    let len = key.chars().count();
    if !(1..=MAX_SESSION_TAG_KEY_LEN).contains(&len) {
        return Err(format!(
            "session tag key must be 1-{MAX_SESSION_TAG_KEY_LEN} characters, got {len}"
        ));
    }
    if !key.chars().all(aws_tag_char) {
        return Err(format!("session tag key {key:?} contains characters AWS does not accept"));
    }
    Ok(())
}

/// AWS session tag value: 0-256 chars of the AWS tag charset. Applied to
/// static values at load and attribution-derived values per request.
pub fn validate_session_tag_value(value: &str) -> Result<(), String> {
    let len = value.chars().count();
    if len > MAX_SESSION_TAG_VALUE_LEN {
        return Err(format!(
            "session tag value must be at most {MAX_SESSION_TAG_VALUE_LEN} characters, got {len}"
        ));
    }
    if !value.chars().all(aws_tag_char) {
        return Err(format!(
            "session tag value {value:?} contains characters AWS does not accept"
        ));
    }
    Ok(())
}

fn aws_tag_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | '=' | '+' | '-' | '@' | ' ')
}

fn validate_template(t: &RejectionTemplate, reason: &str, allowed: &[&str], errs: &mut Vec<String>) {
    if !(100..=599).contains(&t.status) {
        errs.push(format!("{reason}: status {} is not a valid HTTP status", t.status));
    }
    if t.content_type.trim().is_empty() {
        errs.push(format!("{reason}: content_type must not be empty"));
    }
    check_placeholders(&t.body, &format!("{reason}.body"), allowed, errs);
    if let Some(streaming) = &t.streaming {
        check_placeholders(&streaming.data, &format!("{reason}.streaming.data"), allowed, errs);
    }
}

fn check_placeholders(text: &str, ctx: &str, allowed: &[&str], errs: &mut Vec<String>) {
    match template::placeholders(text) {
        Ok(names) => {
            for name in names {
                if !allowed.contains(&name.as_str()) {
                    errs.push(format!(
                        "{ctx}: unknown placeholder {{{{{name}}}}} (allowed: {})",
                        allowed
                            .iter()
                            .map(|a| format!("{{{{{a}}}}}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
        }
        Err(e) => errs.push(format!("{ctx}: {e}")),
    }
}

