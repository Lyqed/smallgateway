//! gateway-core: the data plane's provider-independent core.
//!
//! The event model, wire-format parsers, provider adapters, and streaming
//! token metering are promoted from Phase 0's Spike A
//! (`spikes/event-model/`) with the spike's behavior locked in by the same
//! tests. Phase 1 adds the static config model — providers, routes,
//! attribution rules (GB-1/2/3), and operator-defined rejection templates
//! (GB-4) — plus the pure resolution logic the proxy binds to headers.
//! Milestone 2 adds versioned snapshots (`snapshot`): load + validate +
//! stamp, the unit a data plane binds atomically per request.

pub mod adapters;
pub mod attribution;
pub mod config;
pub mod event;
pub mod eventstream;
pub mod jwt;
pub mod metering;
pub mod snapshot;
pub mod sse;
pub mod template;
