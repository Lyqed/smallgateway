//! Provider adapters: raw wire bytes in, canonical events out.
//!
//! Each adapter is a synchronous push-parser with bounded internal state —
//! only the current partial frame or block, never the whole response.
//! Backpressure falls out of the shape: the caller feeds the next chunk only
//! after consuming the events from the previous one, so nothing accumulates.
//! The async wrapper (a `Stream` combinator over a bytes `Stream`) is
//! mechanical and deferred to the real data plane.

pub mod anthropic;
pub mod bedrock;
pub mod openai;

use crate::event::Event;

pub trait Adapter {
    fn feed(&mut self, bytes: &[u8]) -> Vec<Event>;
}
