//! Drift detection and self-heal — the reconciler (docs/07-control-plane.md,
//! "Drift detection and self-heal", the truth table around line 300).
//!
//! The reconciler is the control plane's loop. Every tick it compares exactly
//! three hashes per connected node, all of which already exist as concepts:
//!
//! - **Desired**: the `render_hash` the control plane computes for THAT node
//!   from the current applied commit — the node's OWN per-node render, i.e. the
//!   GatewaySet-stamped render for a node whose labels match a GatewaySet, else
//!   the fleet-wide render (`Fleet::desired_for`). A node matching no GatewaySet
//!   has the fleet-wide hash as before, so the single-node and no-GatewaySet
//!   cases are unchanged.
//! - **Delivered**: the `render_hash` the control plane last PUSHED to the node
//!   (`Fleet::delivered_hash`).
//! - **Observed**: the `render_hash` the node reports in `Status` — what it is
//!   *actually* running (`NodeState::observed_hash`).
//!
//! Drift is any mismatch among the three, and each mismatch has one convergence
//! action. The classification ([`classify`]) is a **pure function** of the three
//! hashes plus the node's break-glass state, so every row of the truth table is
//! directly testable without a network. The action side ([`Reconciler::tick`])
//! drives the pure verdict: self-heal is a re-push; a node that persistently
//! NACKs desired is surfaced loudly and left visibly divergent, never retried
//! into oblivion; a node under an active break-glass window is TOLERATED and not
//! fought until its TTL lapses (docs/00 break-glass with TTL).
//!
//! The reconciler is not in the request path: it is O(nodes) hash comparisons
//! per tick, no config re-render unless the commit changed, and the data plane
//! serves entirely from its local snapshot whether or not the control plane is
//! up (docs/07).

use std::sync::Arc;
use std::time::Duration;

use log::{error, info, warn};

use crate::server::ControlPlane;
use crate::store::{now_unix, NodeState};

/// After this many consecutive NACKs of desired, a node is declared
/// *persistently divergent* — its local environment genuinely cannot serve the
/// desired render — and the reconciler stops re-pushing and surfaces it for a
/// human (docs/07: "does not hide a NACK by retrying it into oblivion").
pub const PERSISTENT_NACK_THRESHOLD: u32 = 3;

/// The desired/delivered/observed comparison outcome for one node, and the
/// convergence action it implies. Mirrors the docs/07 truth table row-for-row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftCase {
    /// desired = delivered = observed. Converged; no action.
    InSync,
    /// desired ≠ delivered. A new commit has not yet been rolled to this node
    /// (delivery lost, or a fresh render between waves). Action: re-push desired.
    DeliveryStale,
    /// desired = delivered but observed ≠ desired. The node drifted — it
    /// restarted on a stale local file, was break-glassed, or was tampered
    /// with. Action: re-push desired; the node swaps back.
    NodeDrifted,
    /// The node is under an active break-glass window: its drift is TOLERATED
    /// for the bounded window and NOT fought (docs/00). Carries the expiry.
    BreakGlassTolerated { until: u64 },
    /// The node has NACKed desired past the persistence threshold: deliberately
    /// divergent, surfaced loudly, left for a human. No further re-push.
    PersistentlyDivergent { nacks: u32 },
    /// The node has not reported an observed hash yet (no Status heartbeat since
    /// connect). Not drift — just unknown-so-far. Action: none (wait).
    ObservedUnknown,
}

impl DriftCase {
    /// Whether this case's convergence action is a re-push of desired.
    pub fn heals_by_repush(&self) -> bool {
        matches!(self, DriftCase::DeliveryStale | DriftCase::NodeDrifted)
    }
}

/// Classify one node against the desired render hash — the pure truth-table
/// evaluation. `now` is the clock reading for the break-glass check (injected so
/// tests are deterministic).
///
/// Break-glass is checked FIRST: an operator override during its window
/// suppresses the heal entirely, even if the node is drifted, tampered, or
/// NACKing — that is the whole point of break-glass (docs/00). A
/// persistently-NACKing node is checked next (it must not be re-pushed even
/// though its delivered=desired), then the ordinary desired/delivered/observed
/// rows.
pub fn classify(node: &NodeState, desired: &str, now: u64) -> DriftCase {
    // Break-glass tolerates ALL drift for the window's duration.
    if node.break_glass_active(now) {
        return DriftCase::BreakGlassTolerated {
            until: node.break_glass_until.unwrap_or(now),
        };
    }

    // "Delivered" for the pure classification is the last hash the node acked;
    // `Reconciler::tick` folds in the authoritative Fleet push record before
    // calling here, so a lost-but-never-acked push is still seen as stale.
    let delivered = node.last_acked_hash.as_deref();
    let observed = node.observed_hash.as_deref();

    // A node that keeps NACKing desired is deliberately divergent — surfaced,
    // not re-pushed. We can only know it NACKed *desired* if its last NACK was
    // against the current desired render; the consecutive counter plus a
    // delivered-equals-desired condition captures "we keep pushing desired and
    // it keeps refusing".
    if node.consecutive_nacks >= PERSISTENT_NACK_THRESHOLD {
        return DriftCase::PersistentlyDivergent {
            nacks: node.consecutive_nacks,
        };
    }

    // desired ≠ delivered: the node has not been delivered the current desired
    // render (a lost push, or a render produced between waves). Re-push.
    match delivered {
        Some(d) if d != desired => return DriftCase::DeliveryStale,
        None => return DriftCase::DeliveryStale, // never delivered desired
        _ => {}
    }

    // delivered = desired. Now consult observed.
    match observed {
        None => DriftCase::ObservedUnknown,
        Some(o) if o == desired => DriftCase::InSync,
        Some(_) => DriftCase::NodeDrifted, // running something other than desired
    }
}

/// The outcome of one reconcile tick over the whole connected fleet, for logs
/// and tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TickReport {
    pub in_sync: usize,
    pub healed: usize,
    pub break_glass_tolerated: usize,
    pub persistently_divergent: usize,
    pub observed_unknown: usize,
    pub delivery_stale: usize,
    /// Nodes that WOULD heal-by-repush but were left to an in-flight wave: they
    /// are mid-rollout, not drifted, so the reconciler did not fight them
    /// (docs/07). Counted, never healed, this tick.
    pub mid_rollout_deferred: usize,
    /// Nodes PENDING in a not-yet-applied or halt-frozen LATER wave: no wave is
    /// in flight, but a partial application exists and this node's wave has not
    /// reached the target commit. On its prior version legitimately (pending,
    /// not drifted); left to the wave rollout, never healed forward (docs/07).
    pub pending_later_wave: usize,
}

/// The reconciler: owns the tick interval and drives [`classify`] + the
/// convergence action across the connected fleet.
pub struct Reconciler {
    cp: Arc<ControlPlane>,
    interval: Duration,
}

impl Reconciler {
    pub fn new(cp: Arc<ControlPlane>, interval: Duration) -> Reconciler {
        Reconciler { cp, interval }
    }

    /// Run one reconcile tick: classify every connected node and take its
    /// convergence action. Returns a report of what happened. This is the loop
    /// body — [`Reconciler::run`] just calls it on a timer.
    pub async fn tick(&self) -> TickReport {
        let now = now_unix();
        let mut report = TickReport::default();

        for node_id in self.cp.store.connected_ids() {
            let Some(mut node) = self.cp.store.get(&node_id) else {
                continue;
            };
            // Desired is the NODE'S OWN per-node render (GatewaySet-stamped when
            // its labels match one, else the fleet-wide render). Classifying
            // against the fleet-wide hash would see a stamped node as forever
            // drifted and heal it toward the wrong (unstamped) bytes every tick;
            // its own desired hash is the one it will actually ack.
            let desired = self.cp.fleet.desired_for(&node.labels).render_hash;

            // Fold the AUTHORITATIVE delivered hash (from the Fleet's per-node
            // push record) into the node view before classifying, so a lost push
            // that was never acked is still seen as delivery-stale.
            if let Some(delivered) = self.cp.fleet.delivered_hash(&node_id) {
                if node.last_acked_hash.is_none() {
                    node.last_acked_hash = Some(delivered);
                }
            }
            let case = classify(&node, &desired, now);

            // Do not fight a legitimately mid-rollout node (docs/07). While a
            // wave is in flight, a node whose only "drift" is that it has not
            // yet been rolled forward is mid-rollout, not drifted: the wave owns
            // its convergence. Healing it here would double-push the same render
            // and collide with the wave's own ack correlation. Break-glass,
            // persistent-divergence, in-sync, and unknown are NOT heal-by-repush
            // and are handled normally even during a wave.
            if case.heals_by_repush() && self.cp.fleet.wave_in_flight() {
                report.mid_rollout_deferred += 1;
                info!(
                    "[reconcile] node {node_id:?} is mid-rollout (a wave is in flight); \
                     deferring to the wave, NOT healing (docs/07: do not fight a \
                     legitimately mid-rollout node)"
                );
                continue;
            }

            // Do not heal a node PENDING in a not-yet-applied or halt-frozen
            // LATER wave (docs/07). No wave is in flight now, but a partial
            // application exists: some wave reached the target commit while this
            // node's (later) wave has not. Such a node is on its prior version
            // ON PURPOSE — pending, not drifted — and its convergence belongs to
            // the wave rollout (or a resumed rollout after the halt is fixed),
            // never to a heal that would drag it forward past its wave's turn.
            if case.heals_by_repush()
                && self.cp.fleet.node_pending_in_unapplied_wave(&node.labels)
            {
                report.pending_later_wave += 1;
                info!(
                    "[reconcile] node {node_id:?} is PENDING in a not-yet-applied/frozen later \
                     wave; it is on its prior version legitimately, NOT drifted — leaving it to \
                     the wave rollout (docs/07)"
                );
                continue;
            }

            self.act(&node_id, &desired, &case, &mut report).await;
        }
        report
    }

    /// Take the convergence action for one classified node.
    async fn act(&self, node_id: &str, desired: &str, case: &DriftCase, report: &mut TickReport) {
        match case {
            DriftCase::InSync => {
                report.in_sync += 1;
            }
            DriftCase::ObservedUnknown => {
                report.observed_unknown += 1;
            }
            DriftCase::BreakGlassTolerated { until } => {
                report.break_glass_tolerated += 1;
                info!(
                    "[reconcile] node {node_id:?} is BREAK-GLASS (override until unix {until}); \
                     tolerating its drift, NOT healing (docs/00 break-glass with TTL)"
                );
            }
            DriftCase::PersistentlyDivergent { nacks } => {
                report.persistently_divergent += 1;
                // Loud, never silent, never retried into oblivion (docs/07).
                error!(
                    "[reconcile] DIVERGENT node {node_id:?} has NACKed desired \
                     render_hash={} {nacks} times; leaving it visibly divergent for a human \
                     (not re-pushing)",
                    short(desired)
                );
            }
            DriftCase::DeliveryStale | DriftCase::NodeDrifted => {
                let what = if matches!(case, DriftCase::NodeDrifted) {
                    "DRIFTED (observed != desired)"
                } else {
                    "delivery stale (delivered != desired)"
                };
                let delivered = self
                    .cp
                    .fleet
                    .delivered_hash(node_id)
                    .unwrap_or_else(|| "<none>".to_string());
                let observed = self
                    .cp
                    .store
                    .get(node_id)
                    .and_then(|n| n.observed_hash)
                    .unwrap_or_else(|| "<none>".to_string());
                warn!(
                    "[reconcile] node {node_id:?} {what}: desired={} delivered={} observed={}; \
                     self-healing by re-pushing desired",
                    short(desired),
                    short(&delivered),
                    short(&observed),
                );
                let healed = self.cp.heal_node(node_id).await;
                if healed {
                    report.healed += 1;
                    if matches!(case, DriftCase::DeliveryStale) {
                        report.delivery_stale += 1;
                    }
                    info!(
                        "[reconcile] node {node_id:?} healed back to desired render_hash={}",
                        short(desired)
                    );
                } else {
                    warn!(
                        "[reconcile] node {node_id:?} did not ack the heal push this tick; \
                         will retry next tick (still surfaced)"
                    );
                }
            }
        }
    }

    /// Run the reconcile loop forever on the configured interval. Spawned as a
    /// task by the control-plane entrypoint.
    pub async fn run(self) {
        let mut ticker = tokio::time::interval(self.interval);
        // Skip the immediate first tick so the loop settles after startup.
        ticker.tick().await;
        info!(
            "[reconcile] drift reconciler running every {}s",
            self.interval.as_secs()
        );
        loop {
            ticker.tick().await;
            let report = self.tick().await;
            if report.healed > 0
                || report.persistently_divergent > 0
                || report.break_glass_tolerated > 0
                || report.mid_rollout_deferred > 0
                || report.pending_later_wave > 0
            {
                info!(
                    "[reconcile] tick: in_sync={} healed={} break_glass={} divergent={} \
                     unknown={} mid_rollout={} pending_later_wave={}",
                    report.in_sync,
                    report.healed,
                    report.break_glass_tolerated,
                    report.persistently_divergent,
                    report.observed_unknown,
                    report.mid_rollout_deferred,
                    report.pending_later_wave,
                );
            }
        }
    }
}

fn short(hash: &str) -> &str {
    &hash[..12.min(hash.len())]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Build a NodeState with the given delivered(=acked)/observed hashes.
    fn node(acked: Option<&str>, observed: Option<&str>) -> NodeState {
        let mut n = NodeState {
            node_id: "n1".to_string(),
            labels: BTreeMap::new(),
            last_acked_version: acked.map(|_| 1),
            last_acked_hash: acked.map(|s| s.to_string()),
            last_nack: None,
            consecutive_nacks: 0,
            observed_hash: observed.map(|s| s.to_string()),
            health: Some("ok".to_string()),
            last_seen: 0,
            connected: true,
            break_glass_until: None,
        };
        // Keep last_seen fresh enough that no staleness path interferes.
        n.last_seen = now_unix();
        n
    }

    // --- The truth table, row by row (docs/07 line ~300) ---------------------

    #[test]
    fn all_three_equal_is_in_sync_no_op() {
        let n = node(Some("D"), Some("D"));
        assert_eq!(classify(&n, "D", 100), DriftCase::InSync);
    }

    #[test]
    fn desired_differs_from_delivered_is_delivery_stale_repush() {
        // desired=NEW, delivered/observed=OLD: the new commit hasn't reached
        // this node yet -> re-push.
        let n = node(Some("OLD"), Some("OLD"));
        let case = classify(&n, "NEW", 100);
        assert_eq!(case, DriftCase::DeliveryStale);
        assert!(case.heals_by_repush());
    }

    #[test]
    fn delivered_equals_desired_but_observed_differs_is_node_drifted_repush() {
        // The self-heal row: we delivered desired, the node acked it, but it now
        // reports running something else (restart on stale file / break-glass /
        // tamper) -> re-push desired, node swaps back.
        let n = node(Some("D"), Some("STALE"));
        let case = classify(&n, "D", 100);
        assert_eq!(case, DriftCase::NodeDrifted);
        assert!(case.heals_by_repush());
    }

    #[test]
    fn delivered_equals_desired_and_observed_unknown_waits() {
        // Delivered desired, no Status heartbeat yet -> unknown, not drift.
        let n = node(Some("D"), None);
        assert_eq!(classify(&n, "D", 100), DriftCase::ObservedUnknown);
    }

    #[test]
    fn never_delivered_desired_is_delivery_stale() {
        let n = node(None, None);
        assert_eq!(classify(&n, "D", 100), DriftCase::DeliveryStale);
    }

    // --- Break-glass with TTL: tolerate-then-heal ----------------------------

    #[test]
    fn break_glass_tolerates_drift_within_the_window() {
        // The node IS drifted (observed != desired), but break-glass is active:
        // tolerate, do NOT heal.
        let mut n = node(Some("D"), Some("STALE"));
        n.break_glass_until = Some(200); // expires at unix 200
        // now=150 < 200: within the window.
        assert_eq!(
            classify(&n, "D", 150),
            DriftCase::BreakGlassTolerated { until: 200 }
        );
    }

    #[test]
    fn break_glass_lapses_and_the_node_is_healed_again() {
        // Same drifted node; now=250 >= 200: the window has lapsed. The
        // reconciler resumes and the node classifies as drifted (heal).
        let mut n = node(Some("D"), Some("STALE"));
        n.break_glass_until = Some(200);
        assert_eq!(classify(&n, "D", 250), DriftCase::NodeDrifted);
    }

    // --- Persistent NACK is surfaced, not retried ----------------------------

    #[test]
    fn a_persistently_nacking_node_is_surfaced_not_repushed() {
        let mut n = node(Some("D"), Some("D"));
        n.consecutive_nacks = PERSISTENT_NACK_THRESHOLD;
        let case = classify(&n, "D", 100);
        assert_eq!(
            case,
            DriftCase::PersistentlyDivergent {
                nacks: PERSISTENT_NACK_THRESHOLD
            }
        );
        assert!(!case.heals_by_repush(), "a divergent node is not re-pushed");
    }

    #[test]
    fn a_single_nack_is_not_yet_persistent_divergence() {
        // Below the threshold and drifted -> still healed, not surfaced.
        let mut n = node(Some("D"), Some("STALE"));
        n.consecutive_nacks = 1;
        assert_eq!(classify(&n, "D", 100), DriftCase::NodeDrifted);
    }

    #[test]
    fn break_glass_wins_even_over_persistent_nacks() {
        // An operator break-glassing a persistently-NACKing node suppresses even
        // the divergence surfacing for the window's duration.
        let mut n = node(Some("D"), Some("STALE"));
        n.consecutive_nacks = PERSISTENT_NACK_THRESHOLD + 5;
        n.break_glass_until = Some(500);
        assert!(matches!(
            classify(&n, "D", 100),
            DriftCase::BreakGlassTolerated { .. }
        ));
    }
}
