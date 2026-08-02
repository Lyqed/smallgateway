//! Provider adapters: raw wire bytes in, canonical events out.
//!
//! Each adapter is a synchronous push-parser with bounded internal state —
//! only the current partial frame or block, never the whole response.
//! Backpressure falls out of the shape: the caller feeds the next chunk only
//! after consuming the events from the previous one, so nothing accumulates.
//! The async wrapper (a `Stream` combinator over a bytes `Stream`) is
//! mechanical and deferred to the real data plane.
//!
//! Promoted unchanged from `spikes/event-model/src/adapters/` (Phase 0,
//! Spike A).

pub mod anthropic;
pub mod bedrock;
pub mod json_body;
pub mod openai;
pub mod vertex;

use crate::event::Event;

pub trait Adapter {
    fn feed(&mut self, bytes: &[u8]) -> Vec<Event>;

    /// Called exactly once, when the response body ends. Streaming adapters
    /// have nothing left by definition and keep this default; terminal-parse
    /// taps (a non-streaming JSON body is ONE logical message) emit their
    /// whole canonical sequence here, before the meter's report is read.
    fn finish(&mut self) -> Vec<Event> {
        Vec::new()
    }
}
