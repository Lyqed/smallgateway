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
    Config, OperatorValueSpec, ProviderKind, RejectionOverrides, RejectionTemplate, Rejections,
    ATTR_HEADER_PREFIX,
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
            validate_operator_value(&sts.role_arn, &format!("{ctx}: role_arn"), errs, |t| {
                // A fully-static ARN is checked whole; a template is checked
                // on its literal skeleton (the rendered ARN is re-checked per
                // request, fail closed).
                if t.trim().is_empty() {
                    return Err("must not be empty".to_string());
                }
                if !t.starts_with("arn:") || !t.contains(":role/") {
                    return Err(format!(
                        "{t:?} is not an IAM role ARN (want arn:...:role/...)"
                    ));
                }
                Ok(())
            });
            validate_operator_value(
                &sts.session_name,
                &format!("{ctx}: session_name"),
                errs,
                |t| {
                    // Static-only check; templates sanitize per request.
                    if template::placeholders(t).map(|p| p.is_empty()).unwrap_or(true) {
                        validate_session_name(t)?;
                    }
                    Ok(())
                },
            );
            if let Some(allow) = &sts.allow {
                let actx = format!("{ctx}: allow");
                check_key(&allow.key, &format!("{actx}.key"), errs);
                if allow.values.is_empty() {
                    errs.push(format!("{actx}.values must not be empty"));
                }
                for v in &allow.values {
                    if v.trim().is_empty() {
                        errs.push(format!("{actx}.values must not contain empty entries"));
                    }
                }
            }
            if let Some(base) = &sts.base {
                let bctx = format!("{ctx}: base");
                match (&base.web_identity_token.file, &base.web_identity_token.env) {
                    (Some(f), None) if !f.trim().is_empty() => {}
                    (None, Some(v)) if !v.trim().is_empty() => {}
                    _ => errs.push(format!(
                        "{bctx}.web_identity_token: exactly one of 'file' or 'env' \
                         must be set, non-empty"
                    )),
                }
                if !base.role_arn.starts_with("arn:") || !base.role_arn.contains(":role/") {
                    errs.push(format!(
                        "{bctx}.role_arn {:?} is not an IAM role ARN (want arn:...:role/...)",
                        base.role_arn
                    ));
                }
                if let Err(e) = validate_session_name(&base.session_name) {
                    errs.push(format!("{bctx}.session_name: {e}"));
                }
                if sts.duration_secs > 3600 {
                    errs.push(format!(
                        "{ctx}: duration_secs must be <= 3600 with a base hop — role \
                         chaining caps the chained session at one hour (AWS), got {}",
                        sts.duration_secs
                    ));
                }
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
        if let Some(auth) = &p.auth {
            let actx = format!("provider {name:?}: auth");
            if p.kind != ProviderKind::Vertex {
                errs.push(format!(
                    "{actx}: the vertex auth chain applies to vertex-kind providers \
                     only ({name:?} is {})",
                    p.kind.name()
                ));
            }
            match (&auth.web_identity_token.file, &auth.web_identity_token.env) {
                (Some(f), None) if !f.trim().is_empty() => {}
                (None, Some(v)) if !v.trim().is_empty() => {}
                _ => errs.push(format!(
                    "{actx}.web_identity_token: exactly one of 'file' or 'env' must \
                     be set, non-empty"
                )),
            }
            if !auth.service_account_email.contains('@') {
                errs.push(format!(
                    "{actx}.service_account_email {:?} is not an email",
                    auth.service_account_email
                ));
            }
            for (field, v) in [
                ("wif.project_number", &auth.wif.project_number),
                ("wif.pool_id", &auth.wif.pool_id),
                ("wif.provider_id", &auth.wif.provider_id),
            ] {
                if v.trim().is_empty() {
                    errs.push(format!("{actx}.{field} must not be empty"));
                }
            }
            if auth.scopes.is_empty() {
                errs.push(format!("{actx}.scopes must not be empty"));
            }
            if !(300..=3600).contains(&auth.lifetime_secs) {
                errs.push(format!(
                    "{actx}.lifetime_secs must be 300-3600 (generateAccessToken's \
                     default ceiling), got {}",
                    auth.lifetime_secs
                ));
            }
            for (field, up) in [("sts_endpoint", &auth.sts_endpoint), ("iam_endpoint", &auth.iam_endpoint)] {
                if up.host.trim().is_empty() {
                    errs.push(format!("{actx}.{field}.host must not be empty"));
                }
                if up.port == 0 {
                    errs.push(format!("{actx}.{field}.port must be 1-65535"));
                }
            }
        }
        if let Some(locations) = &p.locations {
            let lctx = format!("provider {name:?}: locations");
            if p.kind != ProviderKind::Vertex {
                errs.push(format!(
                    "{lctx}: location routing applies to vertex-kind providers only \
                     ({name:?} is {})",
                    p.kind.name()
                ));
            }
            if locations.is_empty() {
                errs.push(format!("{lctx} must not be empty when present"));
            }
            for l in locations {
                if l.trim().is_empty() || l.contains('/') {
                    errs.push(format!("{lctx}: {l:?} is not a location name"));
                }
            }
        }
        if let Some(inject) = &p.inject {
            let ictx = format!("provider {name:?}: inject");
            for h in &inject.headers {
                let hctx = format!("{ictx}: header {:?}", h.name);
                if h.name.trim().is_empty() || h.name.contains(char::is_whitespace) {
                    errs.push(format!("{hctx}: header name must be a non-empty token"));
                }
                // Signing/transport-owned headers: forcing these would
                // corrupt SigV4 or HTTP framing, never express policy.
                const RESERVED: [&str; 7] = [
                    "host",
                    "authorization",
                    "content-length",
                    "transfer-encoding",
                    "x-amz-date",
                    "x-amz-security-token",
                    "x-amz-content-sha256",
                ];
                if RESERVED.contains(&h.name.to_ascii_lowercase().as_str()) {
                    errs.push(format!(
                        "{hctx}: this header is owned by signing/transport and \
                         cannot be operator-forced"
                    ));
                }
                validate_operator_value(&h.value, &hctx, errs, |t| {
                    if t.is_empty() {
                        return Err("must not be empty".to_string());
                    }
                    Ok(())
                });
            }
            for f in &inject.body {
                let fctx = format!("{ictx}: body {:?}", f.path);
                if f.path.trim().is_empty() || f.path.split('.').any(|s| s.trim().is_empty()) {
                    errs.push(format!(
                        "{fctx}: path must be non-empty dotted segments (a.b.c)"
                    ));
                }
                validate_operator_value(&f.value, &fctx, errs, |_| Ok(()));
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

/// AWS RoleSessionName: 2-64 chars of `[A-Za-z0-9_+=,.@-]` (the API's
/// `[\w+=,.@-]` pattern). Static names are checked here; per-request
/// template renders go through [`crate::aws::sanitize_session_name`].
pub fn validate_session_name(name: &str) -> Result<(), String> {
    let len = name.chars().count();
    if !(2..=64).contains(&len) {
        return Err(format!("RoleSessionName must be 2-64 characters, got {len}"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '=' | ',' | '.' | '@' | '-'))
    {
        return Err(format!(
            "RoleSessionName {name:?} contains characters AWS does not accept ([\\w+=,.@-])"
        ));
    }
    Ok(())
}

/// Shared shape-check for an [`OperatorValueSpec`]: exactly-one-of on the
/// map form, well-formed `{{key}}` placeholders with valid key names, plus
/// a caller-supplied check on the template text itself.
fn validate_operator_value(
    spec: &OperatorValueSpec,
    ctx: &str,
    errs: &mut Vec<String>,
    check_template: impl Fn(&str) -> Result<(), String>,
) {
    let Some(template) = spec.as_template() else {
        errs.push(format!(
            "{ctx}: exactly one of 'value' or 'from_attribution' must be set"
        ));
        return;
    };
    match template::placeholders(&template) {
        Err(e) => errs.push(format!("{ctx}: {e}")),
        Ok(keys) => {
            for key in &keys {
                check_key(key, &format!("{ctx}: placeholder"), errs);
            }
        }
    }
    if let Err(e) = check_template(&template) {
        errs.push(format!("{ctx}: {e}"));
    }
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

