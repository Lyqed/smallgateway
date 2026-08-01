//! Normalizes OpenAI `chat.completion.chunk` SSE into canonical events.
//!
//! OpenAI quirk handled here: with `stream_options.include_usage`, the
//! terminal usage frame arrives *after* the finish_reason chunk. MessageEnd
//! is therefore held until `[DONE]`, which keeps the canonical ordering
//! contract (usage frame before MessageEnd, MessageEnd terminal).

use serde_json::Value;

use super::Adapter;
use crate::event::Event;
use crate::sse::SseParser;

#[derive(Default)]
pub struct OpenAiAdapter {
    sse: SseParser,
    started: bool,
    done: bool,
    stop_reason: Option<String>,
}

impl OpenAiAdapter {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Adapter for OpenAiAdapter {
    fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        let mut out = Vec::new();
        for sse in self.sse.feed(bytes) {
            if sse.data == "[DONE]" {
                if !self.done {
                    self.done = true;
                    out.push(Event::MessageEnd {
                        stop_reason: self.stop_reason.take(),
                    });
                }
                continue;
            }
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
                    message_id: chunk["id"].as_str().map(String::from),
                    model: chunk["model"].as_str().map(String::from),
                });
            }
            for choice in chunk["choices"].as_array().into_iter().flatten() {
                let index = choice["index"].as_u64().unwrap_or(0) as u32;
                let delta = &choice["delta"];
                if let Some(text) = delta["content"].as_str() {
                    if !text.is_empty() {
                        out.push(Event::ContentDelta {
                            index,
                            text: text.to_string(),
                        });
                    }
                }
                for tc in delta["tool_calls"].as_array().into_iter().flatten() {
                    out.push(Event::ToolCallDelta {
                        index: tc["index"].as_u64().unwrap_or(0) as u32,
                        id: tc["id"].as_str().map(String::from),
                        name: tc["function"]["name"].as_str().map(String::from),
                        arguments_delta: tc["function"]["arguments"]
                            .as_str()
                            .unwrap_or("")
                            .to_string(),
                    });
                }
                if let Some(reason) = choice["finish_reason"].as_str() {
                    self.stop_reason = Some(reason.to_string());
                }
            }
            let usage = &chunk["usage"];
            if usage.is_object() {
                out.push(Event::UsageDelta {
                    input_tokens: usage["prompt_tokens"].as_u64(),
                    output_tokens: usage["completion_tokens"].as_u64(),
                });
            }
        }
        out
    }
}
