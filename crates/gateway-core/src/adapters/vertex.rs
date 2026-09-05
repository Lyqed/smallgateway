//! Normalizes Vertex AI Gemini `streamGenerateContent?alt=sse` SSE into
//! canonical events (GB-8's provider; Phase 1, milestone 3).
//!
//! Vertex quirks handled here:
//!
//! - No `[DONE]` sentinel: the terminal frame carries `finishReason` (and
//!   the authoritative `usageMetadata`). `MessageEnd` is emitted when the
//!   finishReason frame is seen, AFTER that frame's usage — preserving the
//!   canonical ordering contract (usage precedes MessageEnd, MessageEnd
//!   terminal).
//! - `usageMetadata` may appear as an empty object on intermediate frames;
//!   only frames with `candidatesTokenCount` produce a `UsageDelta`.
//! - Text arrives as `candidates[].content.parts[].text`; function calls
//!   as `parts[].functionCall {name, args}` (args is a complete JSON
//!   object per part, emitted as one `ToolCallDelta`).

use serde_json::Value;

use super::Adapter;
use crate::event::Event;
use crate::sse::SseParser;

#[derive(Default)]
pub struct VertexAdapter {
    sse: SseParser,
    started: bool,
    ended: bool,
}

impl VertexAdapter {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Adapter for VertexAdapter {
    fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        let mut out = Vec::new();
        for sse in self.sse.feed(bytes) {
            let chunk: Value = match serde_json::from_str(&sse.data) {
                Ok(v) => v,
                Err(e) => {
                    out.push(Event::Error {
                        code: "bad_chunk".into(),
                        message: e.to_string(),
                    });
                    continue;
                }
            };
            if !self.started {
                self.started = true;
                out.push(Event::MessageStart {
                    message_id: chunk["responseId"].as_str().map(String::from),
                    model: chunk["modelVersion"].as_str().map(String::from),
                });
            }
            let mut finish: Option<String> = None;
            for (index, candidate) in
                chunk["candidates"].as_array().into_iter().flatten().enumerate()
            {
                for part in candidate["content"]["parts"].as_array().into_iter().flatten() {
                    if let Some(text) = part["text"].as_str() {
                        if !text.is_empty() {
                            out.push(Event::ContentDelta {
                                index: index as u32,
                                text: text.to_string(),
                            });
                        }
                    }
                    let call = &part["functionCall"];
                    if call.is_object() {
                        out.push(Event::ToolCallDelta {
                            index: index as u32,
                            id: None, // Vertex does not assign tool-call ids
                            name: call["name"].as_str().map(String::from),
                            arguments_delta: call["args"].to_string(),
                        });
                    }
                }
                if let Some(reason) = candidate["finishReason"].as_str() {
                    finish = Some(reason.to_string());
                }
            }
            let usage = &chunk["usageMetadata"];
            if usage["candidatesTokenCount"].is_u64() || usage["promptTokenCount"].is_u64() {
                out.push(Event::UsageDelta {
                    input_tokens: usage["promptTokenCount"].as_u64(),
                    output_tokens: usage["candidatesTokenCount"].as_u64(),
                });
            }
            if let Some(reason) = finish {
                if !self.ended {
                    self.ended = true;
                    out.push(Event::MessageEnd {
                        stop_reason: Some(reason),
                    });
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metering::Meter;

    const FIXTURE: &str = concat!(
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"The Gateway \"}]}}],\"usageMetadata\":{},\"modelVersion\":\"gemini-2.5-flash\",\"responseId\":\"vx1\"}\n\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Project keeps \"}]}}],\"modelVersion\":\"gemini-2.5-flash\",\"responseId\":\"vx1\"}\n\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"truth in Git.\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":14,\"candidatesTokenCount\":9,\"totalTokenCount\":23},\"modelVersion\":\"gemini-2.5-flash\",\"responseId\":\"vx1\"}\n\n",
    );

    fn replay(chunk_size: usize) -> Vec<Event> {
        let mut adapter = VertexAdapter::new();
        let mut events = Vec::new();
        for chunk in FIXTURE.as_bytes().chunks(chunk_size) {
            events.extend(adapter.feed(chunk));
        }
        events
    }

    #[test]
    fn normalizes_to_the_canonical_shape() {
        let events = replay(FIXTURE.len());
        assert!(matches!(events.first(), Some(Event::MessageStart { model: Some(m), .. }) if m == "gemini-2.5-flash"));
        assert!(
            matches!(events.last(), Some(Event::MessageEnd { stop_reason: Some(r) }) if r == "STOP")
        );
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                Event::ContentDelta { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "The Gateway Project keeps truth in Git.");

        // Terminal usage frame precedes MessageEnd; the meter reconciles.
        let last_usage = events
            .iter()
            .rposition(|e| matches!(e, Event::UsageDelta { .. }))
            .expect("usage frame");
        let end = events
            .iter()
            .position(|e| matches!(e, Event::MessageEnd { .. }))
            .unwrap();
        assert!(last_usage < end);

        let mut meter = Meter::new();
        events.iter().for_each(|e| meter.observe(e));
        let report = meter.report();
        assert_eq!(report.authoritative_input_tokens, Some(14));
        assert_eq!(report.authoritative_output_tokens, Some(9));
    }

    #[test]
    fn chunking_never_changes_the_event_stream() {
        let whole = replay(FIXTURE.len());
        for chunk_size in [1, 7, 64] {
            assert_eq!(replay(chunk_size), whole, "chunk_size={chunk_size}");
        }
    }

    #[test]
    fn function_calls_normalize_to_tool_call_deltas() {
        let wire = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"location\":\"Tel Aviv\"}}}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":8,\"candidatesTokenCount\":6},\"responseId\":\"vx2\"}\n\n";
        let mut adapter = VertexAdapter::new();
        let events = adapter.feed(wire.as_bytes());
        let (name, args) = events
            .iter()
            .find_map(|e| match e {
                Event::ToolCallDelta { name, arguments_delta, .. } => {
                    Some((name.clone(), arguments_delta.clone()))
                }
                _ => None,
            })
            .expect("tool call");
        assert_eq!(name.as_deref(), Some("get_weather"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&args).unwrap(),
            serde_json::json!({"location": "Tel Aviv"})
        );
    }

    #[test]
    fn bad_json_yields_an_error_event_not_a_panic() {
        let mut adapter = VertexAdapter::new();
        let events = adapter.feed(b"data: {nope\n\n");
        assert!(matches!(events.last(), Some(Event::Error { code, .. }) if code == "bad_chunk"));
    }
}
