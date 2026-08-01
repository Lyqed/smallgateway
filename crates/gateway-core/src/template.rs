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

/// GB-4's streaming half: render an operator [`StreamingRejection`] into an SSE
/// `event:`/`data:` block ready to write onto the response wire when a stream is
/// cut mid-generation (budget exhausted). The `data` template is substituted
/// with the same `{{name}}` placeholders as the sibling `body`. An event name
/// produces an `event:` line; its absence yields a bare `data:` frame. The block
/// is terminated with the SSE `\n\n` so it parses as one complete event.
///
/// [`StreamingRejection`]: crate::config::StreamingRejection
pub fn render_terminal_event(
    streaming: &crate::config::StreamingRejection,
    vars: &[(&str, &str)],
) -> String {
    let data = render(&streaming.data, vars);
    let mut out = String::new();
    if let Some(event) = &streaming.event {
        out.push_str("event: ");
        out.push_str(event);
        out.push('\n');
    }
    // Multi-line data payloads get one `data:` prefix per line, per SSE.
    for line in data.split('\n') {
        out.push_str("data: ");
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_terminal_event_as_an_sse_block() {
        let streaming = crate::config::StreamingRejection {
            event: Some("error".to_string()),
            data: r#"{"error":"budget exhausted for {{key}}","cap":{{cap}}}"#.to_string(),
        };
        let block = render_terminal_event(
            &streaming,
            &[("key", "team=ml-research"), ("cap", "100000")],
        );
        assert_eq!(
            block,
            "event: error\ndata: {\"error\":\"budget exhausted for team=ml-research\",\"cap\":100000}\n\n"
        );
    }

    #[test]
    fn renders_bare_data_frame_when_no_event_name() {
        let streaming = crate::config::StreamingRejection {
            event: None,
            data: r#"{"done":true}"#.to_string(),
        };
        assert_eq!(
            render_terminal_event(&streaming, &[]),
            "data: {\"done\":true}\n\n"
        );
    }

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
