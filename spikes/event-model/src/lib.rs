//! Spike A: the canonical event model (Phase 0 of the build plan).
//!
//! Bytes from three provider wire formats — OpenAI SSE deltas, Anthropic
//! events, Bedrock event-stream — normalize into one internal event stream,
//! and streaming token metering reconciles an incremental estimate against
//! the provider's terminal usage frame.

pub mod adapters;
pub mod event;
pub mod eventstream;
pub mod metering;
pub mod sse;
