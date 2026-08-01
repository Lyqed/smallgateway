//! The tier-2 WASM config block (Phase 4) — the DECLARATIVE half of signed
//! WASM policy modules. It lives here, not coupled to a wasm runtime: this
//! crate holds only the manifest (name, source, signature, hooks, schema) so
//! gateway-core and the control-plane admission gate can reason about the
//! module set with NO wasmtime dependency. The data plane (`gatewayd`, via
//! `gateway-wasm`) verifies the signature and compiles the bytes; this crate
//! never links a wasm runtime, keeping the two-binary budget intact.
//!
//! Re-exported from [`crate::config`]; [`Config::wasm`](crate::config::Config)
//! carries a [`WasmConfig`]. Structural validation is in
//! [`crate::validate::validate_wasm`].

use serde::Deserialize;

/// The tier-2 WASM block: the modules and where per-event hooks stand.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WasmConfig {
    /// Declared modules, in order. The chain runs in this order; the first
    /// module to return a non-continue decision wins.
    #[serde(default)]
    pub modules: Vec<WasmModule>,
    /// Whether per-event streaming hooks (`on_response_event`) are ENABLED on
    /// this data plane. Default `false`: per the Phase 4 performance
    /// measurement (see `crates/gateway-wasm/README.md`), per-event WASM hooks
    /// on the hot streaming path cost ~12.7us/event with the fresh-instance
    /// isolation model — too much to promise by default. `on_request` and
    /// `on_response_end` are always available; a module that declares
    /// `on_response_event` while this is `false` is admitted but its per-event
    /// hook is NOT invoked (the gate the measured budget demands, docs/04).
    #[serde(default)]
    pub per_event_hooks: bool,
}

/// One declared WASM policy module (the manifest gateway-core validates; the
/// data plane loads it via `gateway-wasm`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WasmModule {
    pub name: String,
    /// Filesystem path to the module bytes (`.wasm` or `.wat`), relative to
    /// the config. In control-plane mode the control plane inlines the bytes
    /// into the rendered snapshot; the path is the source-of-truth reference.
    pub source: String,
    /// Hex HMAC-SHA256 signature over the module bytes. REQUIRED — an
    /// unsigned module (absent or empty) is rejected at admission and at load
    /// (`no unsigned WASM module`, docs/02 admission slot).
    #[serde(default)]
    pub signature: Option<String>,
    /// Which hooks this module implements: any of `on_request`,
    /// `on_response_event`, `on_response_end`.
    pub hooks: Vec<WasmHook>,
    /// The counter schema version for stateful migration (docs/03 limitation
    /// 3). Defaults to 1.
    #[serde(default = "default_schema")]
    pub schema: u32,
    /// The operator-declared bounded reset window (seconds) applied if a
    /// schema bump has no migration path. Defaults to the control-plane
    /// rebalance interval.
    #[serde(default)]
    pub reset_window_secs: Option<u64>,
}

fn default_schema() -> u32 {
    1
}

/// The three policy points a WASM module may hook, mirroring the data plane's
/// existing `on_request` / `on_response_event` / `on_response_end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WasmHook {
    OnRequest,
    OnResponseEvent,
    OnResponseEnd,
}

impl WasmHook {
    pub fn name(self) -> &'static str {
        match self {
            WasmHook::OnRequest => "on_request",
            WasmHook::OnResponseEvent => "on_response_event",
            WasmHook::OnResponseEnd => "on_response_end",
        }
    }
}
