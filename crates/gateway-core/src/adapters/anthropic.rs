//! Normalizes the Anthropic Messages streaming protocol.
//!
//! `message_delta.usage.output_tokens` is already cumulative, which matches
//! the canonical `UsageDelta` semantics directly; `input_tokens` arrives up
//! front in `message_start` and is re-attached to later usage frames.
//!
//! Promoted unchanged from `spikes/event-model/src/adapters/anthropic.rs`.

use serde_json::Value;

use super::Adapter;
use crate::event::Event;
use crate::sse::SseParser;

#[derive(Default)]
pub struct AnthropicAdapter {
    sse: SseParser,
    input_tokens: Option<u64>,
    stop_reason: Option<String>,
}

impl AnthropicAdapter {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Adapter for AnthropicAdapter {
    fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        let mut out = Vec::new();
        for sse in self.sse.feed(bytes) {
            if sse.data.is_empty() {
                continue;
            }
            let data: Value = match serde_json::from_str(&sse.data) {
                Ok(v) => v,
                Err(e) => {
                    out.push(Event::Error {
                        code: "bad_chunk".into(),
                        message: e.to_string(),
                    });
                    continue;
                }
            };
            match data["type"].as_str().unwrap_or_default() {
                "message_start" => {
                    let msg = &data["message"];
                    self.input_tokens = msg["usage"]["input_tokens"].as_u64();
                    out.push(Event::MessageStart {
                        message_id: msg["id"].as_str().map(String::from),
                        model: msg["model"].as_str().map(String::from),
                    });
                    if self.input_tokens.is_some() {
                        out.push(Event::UsageDelta {
                            input_tokens: self.input_tokens,
                            output_tokens: msg["usage"]["output_tokens"].as_u64(),
                        });
                    }
                }
                "content_block_start" => {
                    let index = data["index"].as_u64().unwrap_or(0) as u32;
                    let block = &data["content_block"];
                    match block["type"].as_str() {
                        Some("text") => {
                            if let Some(text) = block["text"].as_str() {
                                if !text.is_empty() {
                                    out.push(Event::ContentDelta {
                                        index,
                                        text: text.to_string(),
                                    });
                                }
                            }
                        }
                        Some("tool_use") => {
                            out.push(Event::ToolCallDelta {
                                index,
                                id: block["id"].as_str().map(String::from),
                                name: block["name"].as_str().map(String::from),
                                arguments_delta: String::new(),
                            });
                        }
                        _ => {}
                    }
                }
                "content_block_delta" => {
                    let index = data["index"].as_u64().unwrap_or(0) as u32;
                    let delta = &data["delta"];
                    match delta["type"].as_str() {
                        Some("text_delta") => {
                            if let Some(text) = delta["text"].as_str() {
                                out.push(Event::ContentDelta {
                                    index,
                                    text: text.to_string(),
                                });
                            }
                        }
                        Some("input_json_delta") => {
                            out.push(Event::ToolCallDelta {
                                index,
                                id: None,
                                name: None,
                                arguments_delta: delta["partial_json"]
                                    .as_str()
                                    .unwrap_or("")
                                    .to_string(),
                            });
                        }
                        _ => {}
                    }
                }
                "message_delta" => {
                    if let Some(reason) = data["delta"]["stop_reason"].as_str() {
                        self.stop_reason = Some(reason.to_string());
                    }
                    if data["usage"].is_object() {
                        out.push(Event::UsageDelta {
                            input_tokens: self.input_tokens,
                            output_tokens: data["usage"]["output_tokens"].as_u64(),
                        });
                    }
                }
                "message_stop" => {
                    out.push(Event::MessageEnd {
                        stop_reason: self.stop_reason.take(),
                    });
                }
                "error" => {
                    out.push(Event::Error {
                        code: data["error"]["type"]
                            .as_str()
                            .unwrap_or("error")
                            .to_string(),
                        message: data["error"]["message"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    });
                }
                // ping, content_block_stop: nothing to normalize
                _ => {}
            }
        }
        out
    }
}
