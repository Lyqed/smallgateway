//! The observed-telemetry sink the canary analysis reads (docs/07 anti-goal:
//! "A dedicated metrics/analysis service for canaries" is OUT — analysis reads
//! the gateway's OWN telemetry, so this is a thread-safe in-memory tally the
//! stream handler folds signals into, NOT a metrics service).
//!
//! Every signal here already flows up the existing `FleetService` stream; this
//! module only aggregates it per node so `canary::analyze` can compare a wave
//! against a baseline. There is no new collection path and no new dependency:
//! - **requests / errors / latency**: carried on the periodic `Status`
//!   heartbeat. The `Status.health` field is a free-form string (the wire is
//!   frozen — docs say the fields are stable), so the node encodes its observed
//!   window tallies into it as `ok req=<n> err=<n> p99=<ms>` (or `degraded ...`);
//!   [`parse_health`] reads them back. A node that only ever says "ok" simply
//!   contributes no request/latency samples — the analysis then leans on spend.
//! - **nacks**: a NACK is a first-class error signal already recorded; the
//!   stream handler counts it as an error against the node's window.
//! - **spend**: NOT stored here — it lives in the budget ledger
//!   ([`crate::budget::FleetBudgets`]) already, and the analysis assembler reads
//!   it from there. This sink owns only the infra signals.
//!
//! Runtime state is in-memory (Postgres deferred, never truth) — wipe it and the
//! next heartbeat window rebuilds it, exactly like the rest of the runtime store.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// One node's accumulated observed telemetry for the CURRENT analysis window.
/// Reset (per node) when a new window opens for a wave under analysis.
#[derive(Debug, Clone, Default)]
pub struct NodeWindow {
    pub requests: u64,
    pub errors: u64,
    /// Observed per-request latency samples (ms) reported this window. Bounded by
    /// [`MAX_LATENCY_SAMPLES`] so a chatty node cannot grow this without bound.
    pub latencies_ms: Vec<f64>,
}

/// A cap on retained latency samples per node per window — a canary window is
/// short and p99 over a few hundred samples is plenty; this keeps the in-memory
/// tally bounded regardless of heartbeat volume.
pub const MAX_LATENCY_SAMPLES: usize = 512;

/// The parsed contents of a node's free-form `Status.health` string. `healthy`
/// is the coarse ok/degraded flag; the optional tallies are the window metrics
/// the node encodes for canary analysis.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedHealth {
    pub healthy: bool,
    pub requests: Option<u64>,
    pub errors: Option<u64>,
    pub p99_ms: Option<f64>,
}

/// Parse a node's `Status.health` string. The format is a leading `ok` or
/// `degraded[: reason]` token, optionally followed by whitespace-separated
/// `key=value` tallies: `req=<n> err=<n> p99=<ms>`. Unknown keys are ignored,
/// so the format can grow without breaking older parsers. A bare `"ok"` (what a
/// node that does not emit metrics sends) parses to healthy with no tallies.
pub fn parse_health(health: &str) -> ParsedHealth {
    let trimmed = health.trim();
    let healthy = !trimmed.starts_with("degraded");
    let mut requests = None;
    let mut errors = None;
    let mut p99_ms = None;
    for tok in trimmed.split_whitespace() {
        let Some((k, v)) = tok.split_once('=') else {
            continue;
        };
        match k {
            "req" | "requests" => requests = v.parse().ok(),
            "err" | "errors" => errors = v.parse().ok(),
            "p99" | "p99_ms" => p99_ms = v.trim_end_matches("ms").parse().ok(),
            _ => {}
        }
    }
    ParsedHealth {
        healthy,
        requests,
        errors,
        p99_ms,
    }
}

/// The fleet-wide observed-telemetry sink: per-node windows the stream handler
/// folds signals into, read by the canary analysis assembler. `Mutex<BTreeMap>`
/// is plenty at this scale; the interface is what a durable store would
/// implement later.
#[derive(Default)]
pub struct FleetTelemetry {
    per_node: Mutex<BTreeMap<String, NodeWindow>>,
}

impl FleetTelemetry {
    pub fn new() -> FleetTelemetry {
        FleetTelemetry::default()
    }

    /// Fold one node's `Status.health` heartbeat into its window: parse the
    /// tallies and record request/error counts and the p99 sample. A degraded
    /// health string with no explicit error tally still counts as one error
    /// against at least one request, so a node that only reports "degraded"
    /// (no numbers) still moves the error rate.
    pub fn record_health(&self, node_id: &str, health: &str) {
        let parsed = parse_health(health);
        let mut nodes = self.lock();
        let w = nodes.entry(node_id.to_string()).or_default();
        match (parsed.requests, parsed.errors) {
            (Some(req), Some(err)) => {
                w.requests = w.requests.saturating_add(req);
                w.errors = w.errors.saturating_add(err);
            }
            (Some(req), None) => {
                w.requests = w.requests.saturating_add(req);
                if !parsed.healthy {
                    w.errors = w.errors.saturating_add(1);
                }
            }
            (None, _) if !parsed.healthy => {
                // No numeric tallies, just "degraded": count one error/one req so
                // the signal is not silently dropped.
                w.requests = w.requests.saturating_add(1);
                w.errors = w.errors.saturating_add(1);
            }
            _ => {}
        }
        if let Some(p99) = parsed.p99_ms {
            if w.latencies_ms.len() < MAX_LATENCY_SAMPLES {
                w.latencies_ms.push(p99);
            }
        }
    }

    /// Count one NACK as an error signal against a node's window (a NACK is a
    /// node rejecting its desired config — a first-class error the analysis must
    /// see even if the node's health string stayed "ok").
    pub fn record_nack(&self, node_id: &str) {
        let mut nodes = self.lock();
        let w = nodes.entry(node_id.to_string()).or_default();
        w.requests = w.requests.saturating_add(1);
        w.errors = w.errors.saturating_add(1);
    }

    /// TEST/DEMO affordance: inject a raw observed window for a node directly,
    /// standing in for a run of heartbeats. Lets the demo and tests drive the
    /// analysis deterministically without a live traffic generator.
    pub fn set_window(&self, node_id: &str, window: NodeWindow) {
        self.lock().insert(node_id.to_string(), window);
    }

    /// Snapshot one node's current window (cloned), or an empty window.
    pub fn window_for(&self, node_id: &str) -> NodeWindow {
        self.lock().get(node_id).cloned().unwrap_or_default()
    }

    /// Reset a node's window (a new analysis window opens for its wave). Absent
    /// node is a no-op.
    pub fn reset(&self, node_id: &str) {
        self.lock().remove(node_id);
    }

    /// Reset the windows for a set of nodes (open a fresh analysis window for a
    /// whole wave and its baseline before collection begins).
    pub fn reset_many(&self, node_ids: &[String]) {
        let mut nodes = self.lock();
        for id in node_ids {
            nodes.remove(id);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, NodeWindow>> {
        self.per_node.lock().expect("telemetry lock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_ok_health_parses_healthy_with_no_tallies() {
        let p = parse_health("ok");
        assert!(p.healthy);
        assert_eq!(p.requests, None);
        assert_eq!(p.errors, None);
        assert_eq!(p.p99_ms, None);
    }

    #[test]
    fn a_health_string_with_tallies_parses_every_field() {
        let p = parse_health("ok req=1000 err=12 p99=145ms");
        assert!(p.healthy);
        assert_eq!(p.requests, Some(1000));
        assert_eq!(p.errors, Some(12));
        assert_eq!(p.p99_ms, Some(145.0));
    }

    #[test]
    fn a_degraded_health_string_parses_unhealthy() {
        let p = parse_health("degraded: upstream 503s req=500 err=250 p99=800ms");
        assert!(!p.healthy);
        assert_eq!(p.requests, Some(500));
        assert_eq!(p.errors, Some(250));
    }

    #[test]
    fn recording_health_accumulates_requests_and_errors() {
        let t = FleetTelemetry::new();
        t.record_health("n1", "ok req=100 err=2 p99=90ms");
        t.record_health("n1", "ok req=100 err=3 p99=110ms");
        let w = t.window_for("n1");
        assert_eq!(w.requests, 200);
        assert_eq!(w.errors, 5);
        assert_eq!(w.latencies_ms, vec![90.0, 110.0]);
    }

    #[test]
    fn a_bare_degraded_counts_one_error_so_the_signal_is_not_lost() {
        let t = FleetTelemetry::new();
        t.record_health("n1", "degraded: boom");
        let w = t.window_for("n1");
        assert_eq!(w.requests, 1);
        assert_eq!(w.errors, 1);
    }

    #[test]
    fn a_nack_counts_as_an_error_even_when_health_stayed_ok() {
        let t = FleetTelemetry::new();
        t.record_health("n1", "ok req=100 err=0 p99=90ms");
        t.record_nack("n1");
        let w = t.window_for("n1");
        assert_eq!(w.requests, 101);
        assert_eq!(w.errors, 1);
    }

    #[test]
    fn latency_samples_are_bounded() {
        let t = FleetTelemetry::new();
        for _ in 0..(MAX_LATENCY_SAMPLES + 50) {
            t.record_health("n1", "ok req=1 err=0 p99=100ms");
        }
        assert_eq!(t.window_for("n1").latencies_ms.len(), MAX_LATENCY_SAMPLES);
    }

    #[test]
    fn reset_clears_a_nodes_window() {
        let t = FleetTelemetry::new();
        t.record_health("n1", "ok req=10 err=1 p99=90ms");
        t.reset("n1");
        assert_eq!(t.window_for("n1").requests, 0);
    }
}
