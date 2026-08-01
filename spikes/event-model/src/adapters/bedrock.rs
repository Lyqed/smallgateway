//! Normalizes Bedrock ConverseStream: AWS event-stream binary framing
//! outside, JSON event payloads inside.
//!
//! Bedrock quirk handled here: the authoritative usage arrives in a
//! `metadata` event *after* `messageStop`, so MessageEnd is held until
//! metadata lands — same normalization move as the OpenAI adapter, keeping
//! the canonical ordering contract identical across providers.

use serde_json::Value;

use super::Adapter;
use crate::event::Event;
use crate::eventstream::{encode_frame, FrameDecoder};

#[derive(Default)]
pub struct BedrockAdapter {
    decoder: FrameDecoder,
    started: bool,
    ended: bool,
    stop_reason: Option<String>,
}

impl BedrockAdapter {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Adapter for BedrockAdapter {
    fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        let frames = match self.decoder.feed(bytes) {
            Ok(frames) => frames,
            Err(message) => {
                return vec![Event::Error {
                    code: "bad_frame".into(),
                    message,
                }]
            }
        };
        let mut out = Vec::new();
        for frame in frames {
            let event_type = frame
                .headers
                .get(":event-type")
                .cloned()
                .unwrap_or_default();
            let payload: Value = match serde_json::from_slice(&frame.payload) {
                Ok(v) => v,
                Err(e) => {
                    out.push(Event::Error {
                        code: "bad_payload".into(),
                        message: e.to_string(),
                    });
                    continue;
                }
            };
            match event_type.as_str() {
                "messageStart" => {
                    if !self.started {
                        self.started = true;
                        // ConverseStream carries no message id or model on
                        // the wire; they come from the request context.
                        out.push(Event::MessageStart {
                            message_id: None,
                            model: None,
                        });
                    }
                }
                "contentBlockStart" => {
                    let index = payload["contentBlockIndex"].as_u64().unwrap_or(0) as u32;
                    let tool = &payload["start"]["toolUse"];
                    if tool.is_object() {
                        out.push(Event::ToolCallDelta {
                            index,
                            id: tool["toolUseId"].as_str().map(String::from),
                            name: tool["name"].as_str().map(String::from),
                            arguments_delta: String::new(),
                        });
                    }
                }
                "contentBlockDelta" => {
                    let index = payload["contentBlockIndex"].as_u64().unwrap_or(0) as u32;
                    let delta = &payload["delta"];
                    if let Some(text) = delta["text"].as_str() {
                        out.push(Event::ContentDelta {
                            index,
                            text: text.to_string(),
                        });
                    }
                    if let Some(args) = delta["toolUse"]["input"].as_str() {
                        out.push(Event::ToolCallDelta {
                            index,
                            id: None,
                            name: None,
                            arguments_delta: args.to_string(),
                        });
                    }
                    // Reasoning models (e.g. gpt-oss on Bedrock) stream
                    // reasoning text as its own content block; those tokens
                    // are billed output, so the meter must see them.
                    if let Some(text) = delta["reasoningContent"]["text"].as_str() {
                        out.push(Event::ContentDelta {
                            index,
                            text: text.to_string(),
                        });
                    }
                }
                "messageStop" => {
                    self.stop_reason = payload["stopReason"].as_str().map(String::from);
                }
                "metadata" => {
                    let usage = &payload["usage"];
                    if usage.is_object() {
                        out.push(Event::UsageDelta {
                            input_tokens: usage["inputTokens"].as_u64(),
                            output_tokens: usage["outputTokens"].as_u64(),
                        });
                    }
                    if !self.ended {
                        self.ended = true;
                        out.push(Event::MessageEnd {
                            stop_reason: self.stop_reason.take(),
                        });
                    }
                }
                _ => {}
            }
        }
        out
    }
}

/// Encode a JSONL fixture (`{"event": "...", "payload": {...}}` per line)
/// into real event-stream frames, so tests and the CLI exercise the full
/// binary decode path rather than skipping the framing.
pub fn encode_jsonl_fixture(jsonl: &str) -> Result<Vec<u8>, String> {
    let mut wire = Vec::new();
    for line in jsonl.lines().filter(|l| !l.trim().is_empty()) {
        let value: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        let event = value["event"]
            .as_str()
            .ok_or_else(|| format!("fixture line missing event name: {line}"))?;
        let payload = serde_json::to_vec(&value["payload"]).map_err(|e| e.to_string())?;
        wire.extend(encode_frame(
            &[
                (":message-type", "event"),
                (":event-type", event),
                (":content-type", "application/json"),
            ],
            &payload,
        ));
    }
    Ok(wire)
}
