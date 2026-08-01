//! The canonical event model. Every provider adapter normalizes its wire
//! format into this stream; every policy hook consumes it.
//!
//! Promoted unchanged from `spikes/event-model/src/event.rs` (Phase 0, Spike A).

/// One normalized streaming event.
///
/// Ordering contract, enforced by the adapters even where providers differ:
/// `MessageStart` first, `MessageEnd` last, and the terminal usage frame
/// (the last `UsageDelta`) always precedes `MessageEnd`.
///
/// `UsageDelta` carries cumulative totals as reported by the provider so
/// far, not increments; the last one before `MessageEnd` is authoritative.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
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
