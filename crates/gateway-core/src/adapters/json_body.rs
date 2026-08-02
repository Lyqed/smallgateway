//! Bounded terminal-parse tap for non-streaming JSON responses.
//!
//! The response head decides the tap (docs/11, decision 1): when a metered
//! route answers `application/json` instead of a stream, the body is ONE
//! logical message and the provider's usage object — the authoritative
//! number the whole thesis rests on — is inside it. This adapter
//! accumulates the body up to a stated cap and emits the same canonical
//! sequence the streaming adapters produce, at `finish`: MessageStart, the
//! authoritative UsageDelta, MessageEnd. One contract for every policy,
//! both framings (Q2's written second half).
//!
//! This is the one place a whole body is legitimately held. The cap is the
//! stated invariant that keeps it honest: past it, the tap degrades LOUDLY
//! (an Error event, a meter with no authoritative frame) — never a silent
//! zero.

use serde_json::Value;

use super::Adapter;
use crate::config::ProviderKind;
use crate::event::Event;

/// The accumulation bound. Sized to hold the largest legal non-streaming
/// body whole: a max-batch float embeddings response (2048 inputs x 3072
/// dims, serialized as JSON floats at ~11-20 bytes each) reaches ~130 MB, so
/// the cap is 256 MiB. Past it the tap degrades LOUDLY (an Error event that
/// stamps the span), never a silent zero. Worst-case tap memory is a boring
/// `concurrency x cap`; the bound is deliberately generous because the whole
/// body must be held to reach the usage object, and under-sizing it turns a
/// legal response into a metering hole. Published in docs/11.
pub const JSON_BODY_CAP: usize = 256 * 1024 * 1024;

pub struct JsonBodyTap {
    kind: ProviderKind,
    cap: usize,
    buf: Vec<u8>,
    overflowed: bool,
}

impl JsonBodyTap {
    pub fn new(kind: ProviderKind) -> Self {
        Self::with_cap(kind, JSON_BODY_CAP)
    }

    pub fn with_cap(kind: ProviderKind, cap: usize) -> Self {
        JsonBodyTap {
            kind,
            cap,
            buf: Vec::new(),
            overflowed: false,
        }
    }
}

impl Adapter for JsonBodyTap {
    fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        if self.overflowed {
            return Vec::new();
        }
        if self.buf.len() + bytes.len() > self.cap {
            // A truncated document parses as nothing; stop paying for it.
            self.overflowed = true;
            self.buf = Vec::new();
            return Vec::new();
        }
        self.buf.extend_from_slice(bytes);
        Vec::new()
    }

    fn finish(&mut self) -> Vec<Event> {
        if self.overflowed {
            return vec![Event::Error {
                code: "json_body_overflow".into(),
                message: format!(
                    "response body exceeded the {}-byte terminal-parse bound; \
                     no authoritative usage was read",
                    self.cap
                ),
            }];
        }
        let parsed: Value = match serde_json::from_slice(&self.buf) {
            Ok(v) => v,
            Err(e) => {
                return vec![Event::Error {
                    code: "json_body_parse".into(),
                    message: format!("declared application/json did not parse: {e}"),
                }];
            }
        };
        // Vertex `streamGenerateContent` WITHOUT `alt=sse` (the REST default)
        // answers 200 application/json whose body is a JSON ARRAY of chunk
        // objects; the cumulative `usageMetadata` rides the LAST element. A
        // top-level array on any other dialect is not a shape we know how to
        // meter — treat that as a loud degradation below, not a silent zero.
        let body = match &parsed {
            Value::Array(chunks) => match (self.kind, chunks.last()) {
                (ProviderKind::Vertex, Some(last)) => last,
                _ => {
                    return vec![Event::Error {
                        code: "json_body_shape".into(),
                        message: format!(
                            "application/json body was a top-level array on a \
                             {} route; no authoritative usage could be read",
                            self.kind.name()
                        ),
                    }];
                }
            },
            other => other,
        };
        // A 2xx body carrying a provider error envelope (rare, but Anthropic
        // and OpenAI both define one) is an error, not a message.
        let err = &body["error"];
        if err.is_object() {
            return vec![Event::Error {
                code: err["type"]
                    .as_str()
                    .or_else(|| err["code"].as_str())
                    .unwrap_or("error")
                    .to_string(),
                message: err["message"].as_str().unwrap_or("").to_string(),
            }];
        }

        let (input, output) = usage(self.kind, body);
        let mut out = vec![Event::MessageStart {
            message_id: body["id"].as_str().map(String::from),
            model: model(self.kind, body),
        }];
        if input.is_some() || output.is_some() {
            out.push(Event::UsageDelta {
                input_tokens: input,
                output_tokens: output,
            });
        }
        out.push(Event::MessageEnd {
            stop_reason: stop_reason(self.kind, body),
        });
        out
    }
}

/// Each dialect names its usage fields differently; the response body's own
/// names are the spec (proxy, not a format).
fn usage(kind: ProviderKind, body: &Value) -> (Option<u64>, Option<u64>) {
    match kind {
        // Embeddings bodies carry prompt_tokens with no completion_tokens;
        // both fields stay independently optional for exactly that shape.
        ProviderKind::OpenAi => (
            body["usage"]["prompt_tokens"].as_u64(),
            body["usage"]["completion_tokens"].as_u64(),
        ),
        ProviderKind::Anthropic => (
            body["usage"]["input_tokens"].as_u64(),
            body["usage"]["output_tokens"].as_u64(),
        ),
        ProviderKind::Bedrock => (
            body["usage"]["inputTokens"].as_u64(),
            body["usage"]["outputTokens"].as_u64(),
        ),
        ProviderKind::Vertex => (
            body["usageMetadata"]["promptTokenCount"].as_u64(),
            body["usageMetadata"]["candidatesTokenCount"].as_u64(),
        ),
    }
}

fn model(kind: ProviderKind, body: &Value) -> Option<String> {
    let field = match kind {
        ProviderKind::OpenAi | ProviderKind::Anthropic | ProviderKind::Bedrock => "model",
        ProviderKind::Vertex => "modelVersion",
    };
    body[field].as_str().map(String::from)
}

fn stop_reason(kind: ProviderKind, body: &Value) -> Option<String> {
    let v = match kind {
        ProviderKind::OpenAi => &body["choices"][0]["finish_reason"],
        ProviderKind::Anthropic => &body["stop_reason"],
        ProviderKind::Bedrock => &body["stopReason"],
        ProviderKind::Vertex => &body["candidates"][0]["finishReason"],
    };
    v.as_str().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(kind: ProviderKind, body: &str) -> Vec<Event> {
        let mut tap = JsonBodyTap::new(kind);
        assert!(tap.feed(body.as_bytes()).is_empty(), "feed only accumulates");
        tap.finish()
    }

    fn usage_of(events: &[Event]) -> (Option<u64>, Option<u64>) {
        events
            .iter()
            .find_map(|e| match e {
                Event::UsageDelta {
                    input_tokens,
                    output_tokens,
                } => Some((*input_tokens, *output_tokens)),
                _ => None,
            })
            .expect("a UsageDelta")
    }

    #[test]
    fn openai_chat_completion_body() {
        let events = run(
            ProviderKind::OpenAi,
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"index":0,
                "message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":12,"completion_tokens":34,"total_tokens":46}}"#,
        );
        assert!(matches!(&events[0], Event::MessageStart { model: Some(m), .. } if m == "gpt-4o"));
        assert_eq!(usage_of(&events), (Some(12), Some(34)));
        assert!(
            matches!(events.last(), Some(Event::MessageEnd { stop_reason: Some(r) }) if r == "stop")
        );
    }

    #[test]
    fn openai_embeddings_body_meters_input_only() {
        let events = run(
            ProviderKind::OpenAi,
            r#"{"object":"list","data":[{"embedding":[0.1,0.2]}],
                "model":"text-embedding-3-small",
                "usage":{"prompt_tokens":8,"total_tokens":8}}"#,
        );
        assert_eq!(usage_of(&events), (Some(8), None));
    }

    #[test]
    fn anthropic_messages_body() {
        let events = run(
            ProviderKind::Anthropic,
            r#"{"id":"msg_1","model":"claude-sonnet-4-6","stop_reason":"end_turn",
                "usage":{"input_tokens":100,"output_tokens":25}}"#,
        );
        assert_eq!(usage_of(&events), (Some(100), Some(25)));
    }

    #[test]
    fn bedrock_converse_body() {
        let events = run(
            ProviderKind::Bedrock,
            r#"{"output":{"message":{"role":"assistant"}},"stopReason":"end_turn",
                "usage":{"inputTokens":40,"outputTokens":9,"totalTokens":49}}"#,
        );
        assert_eq!(usage_of(&events), (Some(40), Some(9)));
        assert!(
            matches!(events.last(), Some(Event::MessageEnd { stop_reason: Some(r) }) if r == "end_turn")
        );
    }

    #[test]
    fn vertex_generate_content_body() {
        let events = run(
            ProviderKind::Vertex,
            r#"{"candidates":[{"finishReason":"STOP"}],"modelVersion":"gemini-2.5-pro",
                "usageMetadata":{"promptTokenCount":7,"candidatesTokenCount":3}}"#,
        );
        assert_eq!(usage_of(&events), (Some(7), Some(3)));
        assert!(matches!(&events[0], Event::MessageStart { model: Some(m), .. } if m == "gemini-2.5-pro"));
    }

    #[test]
    fn chunk_boundaries_do_not_matter() {
        let body = r#"{"id":"msg_1","model":"claude-sonnet-4-6","stop_reason":"end_turn",
            "usage":{"input_tokens":100,"output_tokens":25}}"#;
        let mut bytewise = JsonBodyTap::new(ProviderKind::Anthropic);
        for b in body.as_bytes() {
            assert!(bytewise.feed(&[*b]).is_empty());
        }
        assert_eq!(bytewise.finish(), run(ProviderKind::Anthropic, body));
    }

    #[test]
    fn error_envelope_becomes_error_event() {
        let events = run(
            ProviderKind::OpenAi,
            r#"{"error":{"message":"quota","type":"insufficient_quota","code":"insufficient_quota"}}"#,
        );
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], Event::Error { code, message } if code == "insufficient_quota" && message == "quota")
        );
    }

    #[test]
    fn overflow_degrades_loudly_and_stops_accumulating() {
        let mut tap = JsonBodyTap::with_cap(ProviderKind::OpenAi, 16);
        tap.feed(b"{\"usage\":{\"promp");
        tap.feed(b"t_tokens\":12345678}}");
        let events = tap.finish();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], Event::Error { code, .. } if code == "json_body_overflow"));
    }

    #[test]
    fn non_json_body_is_a_parse_error_not_a_silent_zero() {
        let events = run(ProviderKind::Anthropic, "<html>upstream lied</html>");
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], Event::Error { code, .. } if code == "json_body_parse"));
    }

    #[test]
    fn parsed_body_without_usage_still_closes_the_contract() {
        // The adapter emits no UsageDelta (there is none); the PROXY decides
        // whether a usage-free 2xx on a capped route is a degradation. The
        // ordering contract holds either way.
        let events = run(ProviderKind::OpenAi, r#"{"object":"list","data":[]}"#);
        assert!(matches!(&events[0], Event::MessageStart { .. }));
        assert!(matches!(events.last(), Some(Event::MessageEnd { .. })));
        assert!(!events.iter().any(|e| matches!(e, Event::UsageDelta { .. })));
    }

    #[test]
    fn vertex_non_sse_streaming_array_reads_the_last_elements_usage() {
        // streamGenerateContent WITHOUT alt=sse: a JSON array of chunks, the
        // cumulative usageMetadata on the final element.
        let events = run(
            ProviderKind::Vertex,
            r#"[{"candidates":[{"content":{"parts":[{"text":"hel"}]}}]},
                {"candidates":[{"content":{"parts":[{"text":"lo"}]},"finishReason":"STOP"}],
                 "modelVersion":"gemini-2.5-pro",
                 "usageMetadata":{"promptTokenCount":7,"candidatesTokenCount":900}}]"#,
        );
        assert_eq!(usage_of(&events), (Some(7), Some(900)));
        assert!(
            matches!(events.last(), Some(Event::MessageEnd { stop_reason: Some(r) }) if r == "STOP")
        );
    }

    #[test]
    fn top_level_array_on_a_non_vertex_route_degrades_loudly() {
        // The shape that used to parse to a clean zero on non-Vertex dialects.
        let events = run(ProviderKind::OpenAi, r#"[{"a":1},{"b":2}]"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], Event::Error { code, .. } if code == "json_body_shape"));
    }
}
