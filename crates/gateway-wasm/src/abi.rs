//! The host<->guest ABI — the narrow, serializable boundary (docs/02:
//! "Keep the host<->guest boundary narrow and serializable").
//!
//! A WASM policy module exposes hooks matching the data plane's existing
//! policy points — `on_request`, `on_response_event`, `on_response_end` —
//! and receives a BOUNDED view of the request/event/attribution context,
//! returning a [`Decision`]. Everything crossing the boundary is JSON, and
//! the context types below are the entire surface a guest can see: no
//! ambient handles, no pointers into host memory, nothing but the bytes the
//! host chose to marshal.
//!
//! Why JSON and not a richer bindgen ABI: the events are already
//! serde-shaped, the surface is tiny, and a plain length-prefixed byte
//! exchange over one linear-memory buffer keeps the host trivially auditable
//! (no reference types, no host-provided imports the guest can call). The
//! guest exports exactly three functions and imports nothing from the host —
//! that is the sandbox's first wall, before fuel and epochs.

use serde::{Deserialize, Serialize};

use gateway_core::event::Event;

/// Which hook the host is invoking. The guest exports one entrypoint per
/// variant (`on_request`, `on_response_event`, `on_response_end`); the host
/// picks by policy point. Kept as a Rust enum so a module manifest can
/// declare which hooks it implements and the host can skip the rest —
/// crucial for the hot path, where a module that only wants `on_request`
/// must never be called per event (docs/04: gate per-event hooks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hook {
    /// Once, before the request reaches the upstream. Sees request meta +
    /// resolved attribution. May mutate headers or reject.
    OnRequest,
    /// Per canonical [`Event`] on the streaming response path. The HOT
    /// path — the named risk this phase measures. May reject/cut.
    OnResponseEvent,
    /// Once, after the last event drains. Sees the terminal token counts.
    OnResponseEnd,
}

impl Hook {
    /// The exported guest function name the host calls for this hook.
    pub fn export_name(self) -> &'static str {
        match self {
            Hook::OnRequest => "on_request",
            Hook::OnResponseEvent => "on_response_event",
            Hook::OnResponseEnd => "on_response_end",
        }
    }
}

/// The bounded request view a guest sees in `on_request` — a copy, never a
/// live handle. Deliberately small: method, normalized path, lowercase
/// headers, and the RESOLVED attribution (not caller-raw — the gateway has
/// already adjudicated it), so a module reasons over the same adjudicated
/// facts the rest of the pipeline does.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RequestView {
    pub method: String,
    pub path: String,
    /// Lowercase header name -> first value.
    pub headers: std::collections::BTreeMap<String, String>,
    /// Resolved attribution key -> value.
    pub attribution: std::collections::BTreeMap<String, String>,
}

/// The bounded event view for `on_response_event`: one canonical event plus
/// the running estimated-output-token tally so a module can enforce its own
/// budget without owning the meter. This is the whole per-event payload; it
/// must stay cheap to build and serialize because it is built PER EVENT.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventView {
    pub event: WireEvent,
    /// Cumulative estimated output tokens so far (the Meter's running value).
    pub est_output_tokens: u64,
}

/// The terminal view for `on_response_end`: the reconciled token counts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EndView {
    pub est_output_tokens: u64,
    pub authoritative_input_tokens: Option<u64>,
    pub authoritative_output_tokens: Option<u64>,
}

/// A serializable mirror of [`gateway_core::event::Event`]. The core Event
/// is not `Serialize` (it is a pure data-plane type promoted from the
/// spike); rather than couple serde into the core event model, the ABI owns
/// this thin projection and the `From` below keeps them in lockstep. The
/// match is exhaustive on purpose: a new core Event variant fails to compile
/// here until the ABI is extended, so the boundary can never silently drop
/// an event kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireEvent {
    MessageStart {
        message_id: Option<String>,
        model: Option<String>,
    },
    ContentDelta {
        index: u32,
        text: String,
    },
    ToolCallDelta {
        index: u32,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: String,
    },
    UsageDelta {
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    },
    MessageEnd {
        stop_reason: Option<String>,
    },
    Error {
        code: String,
        message: String,
    },
}

impl From<&Event> for WireEvent {
    fn from(e: &Event) -> WireEvent {
        match e {
            Event::MessageStart { message_id, model } => WireEvent::MessageStart {
                message_id: message_id.clone(),
                model: model.clone(),
            },
            Event::ContentDelta { index, text } => WireEvent::ContentDelta {
                index: *index,
                text: text.clone(),
            },
            Event::ToolCallDelta {
                index,
                id,
                name,
                arguments_delta,
            } => WireEvent::ToolCallDelta {
                index: *index,
                id: id.clone(),
                name: name.clone(),
                arguments_delta: arguments_delta.clone(),
            },
            Event::UsageDelta {
                input_tokens,
                output_tokens,
            } => WireEvent::UsageDelta {
                input_tokens: *input_tokens,
                output_tokens: *output_tokens,
            },
            Event::MessageEnd { stop_reason } => WireEvent::MessageEnd {
                stop_reason: stop_reason.clone(),
            },
            Event::Error { code, message } => WireEvent::Error {
                code: code.clone(),
                message: message.clone(),
            },
        }
    }
}

/// The decision a guest returns from any hook. Every variant maps onto a
/// data-plane primitive that already exists (docs/04: "reuse the GB-4
/// templates rather than parallel machinery"):
///
/// - [`Decision::Continue`]  — proceed unchanged.
/// - [`Decision::MutateHeaders`] — set/remove upstream headers (on_request).
/// - [`Decision::Reject`] — refuse the request with the operator's GB-4
///   `default_response` template (on_request); the `reason` names why.
/// - [`Decision::CutStream`] — cut the in-flight stream with the operator's
///   GB-4 streaming terminal event (on_response_event) — the exact
///   machinery GB-5 mid-stream enforcement already uses.
///
/// A guest that traps, exceeds fuel, or blows the epoch deadline never
/// returns a `Decision` at all; the host treats that as fail-closed (a
/// `Reject`/`CutStream` to the operator template), never as `Continue`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    Continue,
    MutateHeaders {
        /// Header name -> value to set on the upstream request.
        set: std::collections::BTreeMap<String, String>,
        /// Header names to remove from the upstream request.
        #[serde(default)]
        remove: Vec<String>,
    },
    Reject {
        /// Surfaced in the log line and available to the GB-4 template as
        /// the `{{key}}` substitution (the "why").
        reason: String,
    },
    CutStream {
        reason: String,
    },
}

impl Decision {
    /// The fail-closed decision the HOST substitutes when a guest cannot be
    /// trusted to have produced one — a trap, a fuel exhaustion, an epoch
    /// deadline, a boundary/serialization fault. On the request path this is
    /// a reject; on the streaming path a cut. NEVER `Continue`: unbounded or
    /// broken guest code must fail the route to the operator template, the
    /// generalized CEL-DoS lesson (docs/04).
    pub fn fail_closed(hook: Hook, reason: String) -> Decision {
        match hook {
            Hook::OnResponseEvent => Decision::CutStream { reason },
            // on_request and on_response_end both refuse the request.
            _ => Decision::Reject { reason },
        }
    }

    pub fn is_continue(&self) -> bool {
        matches!(self, Decision::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_event_round_trips_every_core_variant() {
        // The From is exhaustive; this proves each variant serializes and the
        // tag names are stable (the guest SDK depends on them).
        let events = [
            Event::MessageStart {
                message_id: Some("m1".into()),
                model: Some("gpt".into()),
            },
            Event::ContentDelta {
                index: 0,
                text: "hi".into(),
            },
            Event::ToolCallDelta {
                index: 1,
                id: Some("t".into()),
                name: Some("f".into()),
                arguments_delta: "{".into(),
            },
            Event::UsageDelta {
                input_tokens: Some(3),
                output_tokens: Some(4),
            },
            Event::MessageEnd {
                stop_reason: Some("stop".into()),
            },
            Event::Error {
                code: "e".into(),
                message: "m".into(),
            },
        ];
        for e in &events {
            let wire = WireEvent::from(e);
            let json = serde_json::to_string(&wire).unwrap();
            let back: WireEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(wire, back);
            assert!(json.contains("\"kind\":"), "tagged: {json}");
        }
    }

    #[test]
    fn fail_closed_is_never_continue_and_matches_the_hook() {
        assert!(matches!(
            Decision::fail_closed(Hook::OnRequest, "x".into()),
            Decision::Reject { .. }
        ));
        assert!(matches!(
            Decision::fail_closed(Hook::OnResponseEnd, "x".into()),
            Decision::Reject { .. }
        ));
        // The hot path fails closed by CUTTING, not rejecting a completed head.
        assert!(matches!(
            Decision::fail_closed(Hook::OnResponseEvent, "x".into()),
            Decision::CutStream { .. }
        ));
    }

    #[test]
    fn decision_json_tags_are_snake_case_and_stable() {
        let d = Decision::MutateHeaders {
            set: [("x-a".to_string(), "1".to_string())].into(),
            remove: vec!["x-b".to_string()],
        };
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("\"decision\":\"mutate_headers\""), "{json}");
        let back: Decision = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn hook_export_names_are_the_wire_contract() {
        assert_eq!(Hook::OnRequest.export_name(), "on_request");
        assert_eq!(Hook::OnResponseEvent.export_name(), "on_response_event");
        assert_eq!(Hook::OnResponseEnd.export_name(), "on_response_end");
    }
}
