//! gateway-wasm: tier-2 extensibility — signed WASM policy modules on
//! wasmtime (Phase 4, docs/02 two-tier extensibility, docs/04 Phase 4).
//!
//! This is a LIBRARY, not a binary: the two-binary budget (gatewayd +
//! gatewayctl) holds. gatewayd embeds the [`host`] to run modules on the
//! request/response path; gatewayctl calls [`sig::verify`] at admission.
//!
//! The four walls that make WASM the right tier-2 (vs a native plugin):
//!
//! 1. **No ambient authority** ([`host`]): the engine defines no imports —
//!    no WASI, no clock, no filesystem, no network. A module is a pure
//!    function over the bounded [`abi`] context.
//! 2. **Per-invocation fuel** ([`host::Limits::fuel`]): a module cannot burn
//!    unbounded CPU; exhaustion traps and fails the route CLOSED.
//! 3. **Epoch preemption** ([`host::Watchdog`]): a stuck/looping module is
//!    interrupted at a wall-clock deadline.
//! 4. **Signed modules only** ([`sig`]): an unsigned or tampered module is
//!    rejected — at admission ([`sig::verify`] in gatewayctl) and again at
//!    load ([`module::ModuleSet::load`]).
//!
//! GB-9 hot-swap ([`bind`]): a module set binds atomically with a config
//! snapshot (reusing the data plane's `Arc<Snapshot>` per-request binding),
//! drains per stream, and migrates stateful modules across a version bump
//! ([`state`]) with a stated bounded reset window.
//!
//! The measured per-event hot-path cost — the named risk this phase exists
//! to validate — is in `benches/hotpath.rs` and reported in `README.md`.

pub mod abi;
pub mod bind;
pub mod host;
pub mod module;
pub mod sig;
pub mod state;

pub use abi::{Decision, EndView, EventView, Hook, RequestView, WireEvent};
pub use bind::{BoundModules, ModuleBreakGlass, ModulePlan, DEFAULT_RESET_WINDOW_SECS};
pub use host::{HostError, Limits, PolicyModule, Watchdog};
pub use module::{LoadError, LoadedModule, ModuleManifest, ModuleSet, SharedModuleSet};
pub use sig::{sign, verify, SigError};
pub use state::{
    Counters, Migration, MigrationOutcome, ModuleState, SchemaVersion,
};
