//! `{{placeholder}}` substitution for GB-4 rejection templates.
//!
//! Deliberately tiny: only the exact `{{name}}` form is a placeholder, so
//! single braces pass through untouched — operator bodies are usually JSON
//! and full of them. Whitespace inside the braces is not trimmed; a
//! `{{ key }}` typo surfaces as an unknown placeholder at config load, not
//! as a silently unsubstituted body at 3am.

/// Every `{{name}}` placeholder in the template, in order of appearance
/// (duplicates included — callers that validate can dedup).
///
/// An opening `{{` without a closing `}}` is an error: the operator almost
/// certainly meant a placeholder, and fail-fast beats shipping the typo to
/// a caller as a response body.
pub fn placeholders(template: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            let snippet: String = rest[start..].chars().take(20).collect();
            return Err(format!("unterminated placeholder at {snippet:?}"));
        };
        out.push(after[..end].to_string());
        rest = &after[end + 2..];
    }
    Ok(out)
}

/// Substitute every `{{name}}` with its value. Config validation guarantees
/// (via [`placeholders`]) that a validated template only contains names the
/// caller supplies, so rendering never leaves a placeholder behind.
pub fn render(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (name, value) in vars {
        out = out.replace(&format!("{{{{{name}}}}}"), value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_placeholders_and_ignores_single_braces() {
        let t = r#"{"error":"missing {{key}} on {{route}}","weird":"{ok}"}"#;
        assert_eq!(placeholders(t).unwrap(), vec!["key", "route"]);
        assert_eq!(placeholders("no placeholders {json}").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn whitespace_inside_braces_is_not_trimmed() {
        // "{{ key }}" is the placeholder named " key " — validation will
        // reject it as unknown rather than half-matching.
        assert_eq!(placeholders("{{ key }}").unwrap(), vec![" key "]);
    }

    #[test]
    fn unterminated_placeholder_is_an_error() {
        let err = placeholders(r#"{"oops":"{{key"}"#).unwrap_err();
        assert!(err.contains("unterminated"), "{err}");
    }

    #[test]
    fn renders_all_occurrences() {
        let t = "{{key}} and {{key}} on {{route}}";
        assert_eq!(
            render(t, &[("key", "team"), ("route", "/openai")]),
            "team and team on /openai"
        );
    }

    #[test]
    fn render_leaves_plain_braces_alone() {
        let t = r#"{"missing":"{{key}}"}"#;
        assert_eq!(render(t, &[("key", "team")]), r#"{"missing":"team"}"#);
    }
}
