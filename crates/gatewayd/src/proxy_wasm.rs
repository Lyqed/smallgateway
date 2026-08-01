//! Tier-2 WASM enforcement helpers pulled out of the pingora hooks in
//! `proxy.rs` (Phase 4). The hooks own the `Session`/`&mut ctx` plumbing;
//! everything here is a pure-ish helper over the bounded WASM ABI so the
//! proxy module stays focused and under the file-size budget.

use std::collections::BTreeMap;

use bytes::Bytes;
use gateway_core::budget::CapId;
use gateway_core::config::StreamingRejection;
use gateway_core::event::Event;
use gateway_core::metering::MeterReport;
use gateway_wasm::abi::{EndView, EventView, RequestView};
use gateway_wasm::{BoundModules, Decision as WasmDecision, Hook, WireEvent};
use log::info;

/// Build the bounded `RequestView` a WASM `on_request` hook sees: method,
/// normalized path, the lowercase header map (the same one CEL sees), and the
/// RESOLVED attribution (key -> value). A copy, never a live handle.
pub(crate) fn build_request_view(
    method: &str,
    path: &str,
    headers: &BTreeMap<String, String>,
    tags: &[gateway_core::attribution::Tag],
) -> RequestView {
    RequestView {
        method: method.to_string(),
        path: path.to_string(),
        headers: headers.clone(),
        attribution: tags
            .iter()
            .map(|t| (t.key.clone(), t.value.clone()))
            .collect(),
    }
}

/// What the `on_request` WASM chain decided, reduced for the hook to apply.
pub(crate) enum OnRequestAction {
    /// Proceed; apply these header mutations upstream (empty for a plain
    /// continue).
    Proceed {
        set: BTreeMap<String, String>,
        remove: Vec<String>,
    },
    /// Reject the request to the GB-4 template; `reason` names why.
    Reject { reason: String },
}

/// Run the `on_request` WASM chain and reduce it to an [`OnRequestAction`] the
/// pingora hook applies (the hook owns the async `respond_rejection` and the
/// ctx header fields). A fault already fails closed inside `on_request` to a
/// reject decision, so this never returns "proceed" for a broken module.
pub(crate) fn run_on_request(
    modules: &BoundModules,
    view: &RequestView,
) -> OnRequestAction {
    match modules.on_request(view) {
        WasmDecision::Continue => OnRequestAction::Proceed {
            set: BTreeMap::new(),
            remove: Vec::new(),
        },
        WasmDecision::MutateHeaders { set, remove } => OnRequestAction::Proceed { set, remove },
        WasmDecision::Reject { reason } | WasmDecision::CutStream { reason } => {
            OnRequestAction::Reject { reason }
        }
    }
}

/// The reduced result of the `on_request` WASM chain for the hook to apply:
/// proceed (with header mutations to carry to `upstream_request_filter`) or
/// reject (fail closed to the GB-4 template).
pub(crate) enum RequestOutcome {
    Proceed {
        header_set: BTreeMap<String, String>,
        header_remove: Vec<String>,
    },
    Reject {
        reason: String,
    },
}

/// Run the `on_request` WASM chain and reduce it to a [`RequestOutcome`] the
/// hook applies (avoiding a split borrow of ctx: the hook writes the header
/// fields from the returned value). `None` when no module implements
/// `on_request` — the hook does nothing. Extracted whole from the request hook.
pub(crate) fn apply_on_request(
    modules: &BoundModules,
    method: &str,
    path: &str,
    headers: &BTreeMap<String, String>,
    tags: &[gateway_core::attribution::Tag],
    route_prefix: &str,
    cfg_version: u64,
) -> Option<RequestOutcome> {
    if !modules.wants(gateway_wasm::Hook::OnRequest) {
        return None;
    }
    let view = build_request_view(method, path, headers, tags);
    Some(match run_on_request(modules, &view) {
        OnRequestAction::Proceed { set, remove } => {
            if !set.is_empty() || !remove.is_empty() {
                info!(
                    "[wasm {route_prefix}] on_request -> mutate headers (set {}, remove {}) \
                     cfg=v{cfg_version}",
                    set.len(),
                    remove.len()
                );
            }
            RequestOutcome::Proceed {
                header_set: set,
                header_remove: remove,
            }
        }
        OnRequestAction::Reject { reason } => {
            info!("[wasm {route_prefix}] on_request -> REJECT ({reason}); GB-4 cfg=v{cfg_version}");
            RequestOutcome::Reject { reason }
        }
    })
}

/// The per-event WASM decision, reduced to "should this event CUT the stream,
/// and why". Returns `Some(reason)` on a cut/reject decision (or a fail-closed
/// module fault, which `on_response_event` already maps to `CutStream`), else
/// `None`. Header mutations on the event path are ignored (a streaming event
/// has no request headers to mutate); only continue-vs-cut matters here.
pub(crate) fn event_cut_reason(
    modules: &BoundModules,
    event: &Event,
    est_output_tokens: u64,
) -> Option<String> {
    let view = EventView {
        event: WireEvent::from(event),
        est_output_tokens,
    };
    match modules.on_response_event(&view) {
        WasmDecision::Continue | WasmDecision::MutateHeaders { .. } => None,
        WasmDecision::CutStream { reason } | WasmDecision::Reject { reason } => Some(reason),
    }
}

/// Run the `on_response_end` WASM hook with the reconciled terminal counts, if
/// any module wants it. Observability, not enforcement (the stream already
/// delivered) — a non-continue decision is logged, a fault fails closed to a
/// logged reject inside `on_response_end`, never a silent continue.
pub(crate) fn run_on_response_end(
    modules: &BoundModules,
    report: &MeterReport,
    route_prefix: &str,
    cfg_version: u64,
) {
    if !modules.wants(Hook::OnResponseEnd) {
        return;
    }
    let end = EndView {
        est_output_tokens: report.estimated_output_tokens,
        authoritative_input_tokens: report.authoritative_input_tokens,
        authoritative_output_tokens: report.authoritative_output_tokens,
    };
    match modules.on_response_end(&end) {
        WasmDecision::Continue => {}
        other => info!("[wasm {route_prefix}] on_response_end -> {other:?} cfg=v{cfg_version}"),
    }
}

/// Apply a WASM `on_request` header transform to the upstream request AFTER
/// the resolved-tag insertion — a module operates on the adjudicated request.
/// Removals first, then sets (a module can replace a header).
pub(crate) fn apply_header_mutations(
    upstream: &mut pingora::http::RequestHeader,
    header_set: &BTreeMap<String, String>,
    header_remove: &[String],
) -> pingora::Result<()> {
    for name in header_remove {
        upstream.remove_header(name.as_str());
    }
    for (name, value) in header_set {
        upstream.insert_header(name.clone(), value.clone())?;
    }
    Ok(())
}

/// Render the GB-4 terminal event for a WASM-initiated mid-stream cut, reusing
/// the exact machinery GB-5 uses (`render_cut_event`). The cap/spend are 0 —
/// a WASM cut is a policy decision, not a token-budget crossing — and the
/// `reason` rides in as the spender id so the operator's template can name it.
pub(crate) fn render_wasm_cut(
    streaming: Option<&StreamingRejection>,
    reason: &str,
    route_prefix: &str,
) -> Bytes {
    let id = CapId::new("wasm-policy", reason);
    crate::proxy_support::render_cut_event(streaming, &id, 0, 0, route_prefix)
}
