//! GB-5 budget-share ALLOCATION on the control plane (docs/01 Q4; docs/02
//! "GB-5 at fleet scale — budget shares"; docs/04 Phase 3).
//!
//! The data plane owns the local counters and the enforcement decision
//! ([`gateway_core::budget`]); this module owns the fleet-wide half: it ingests
//! each node's observed-spend telemetry (the `UsageReport` up the existing
//! FleetService stream), tracks the fleet-wide spend per capped value, and
//! REBALANCES each node's share of the cap continuously so a hot node gets a
//! bigger slice. It also answers the synchronous ~90% escalation check with a
//! fresh share grant, and raises GB-6 alerts fleet-wide from the SAME
//! enforcement telemetry (soft 80%, hard cap) so a fleet-wide crossing is never
//! missed even if no single node hit it alone.
//!
//! Runtime state stays in-memory for this milestone (Postgres-backed durable
//! counters are deferred and never truth; docs/07 / docs/03 limitation 3). The
//! caps themselves come from the rendered config (Git truth), not from the
//! telemetry — telemetry only drives HOW the fixed cap is sliced.

use std::collections::BTreeMap;
use std::sync::Mutex;

use gateway_core::budget::{Alert, AlertKind, CapId, SOFT_ALERT_FRACTION};

/// The starting/floor share handed to a node with no observed spend yet, as a
/// fraction of the cap divided across the nodes. A brand-new or cold node must
/// be able to spend SOMETHING before its first usage report, or it would stall
/// at the cap boundary; this is that floor.
pub const COLD_NODE_FLOOR_FRACTION: f64 = 0.10;

/// The label the control plane stamps on alerts it raises itself (fleet-wide),
/// distinct from a node id.
pub const FLEET_NODE: &str = "<control-plane>";

/// One capped value's fleet-wide state: the cap in tokens and each node's
/// last-reported cumulative spend against it. The share allocation is a pure
/// function of these two, recomputed on every rebalance — desired-state math,
/// not stored truth.
#[derive(Debug, Clone, Default)]
struct CapState {
    /// The fleet-wide cap in tokens (from the rendered config). `0` → uncapped
    /// (tracked for telemetry, never denies, never alerts).
    cap: u64,
    /// node_id → cumulative tokens that node has spent against this value.
    per_node_spend: BTreeMap<String, u64>,
    /// Whether the soft/hard GB-6 alerts have fired for this value fleet-wide
    /// (fire once per window; a rebalance below threshold does not re-arm here
    /// — a new cap/window does).
    soft_fired: bool,
    hard_fired: bool,
}

impl CapState {
    fn total_spend(&self) -> u64 {
        self.per_node_spend.values().copied().sum()
    }
}

/// The control plane's fleet-wide budget ledger. Thread-safe (the gRPC session
/// tasks report into it concurrently); a `Mutex<BTreeMap>` is plenty at M1
/// scale, and the interface is what a Postgres-backed store would implement
/// later.
#[derive(Default)]
pub struct FleetBudgets {
    caps: Mutex<BTreeMap<CapId, CapState>>,
}

/// One node's allocated share of one capped value, the unit the control plane
/// grants back down the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareAllocation {
    pub id: CapId,
    pub cap: u64,
    pub share: u64,
}

impl FleetBudgets {
    pub fn new() -> FleetBudgets {
        FleetBudgets::default()
    }

    /// Learn a capped value's cap from the config-derived figure a node reports
    /// (nodes carry the composed cap from their rendered snapshot). Idempotent;
    /// the largest reported cap wins if they ever disagree (defensive — every
    /// node renders the same cap from the same commit).
    pub fn observe_cap(&self, id: &CapId, cap: u64) {
        let mut caps = self.caps.lock().expect("budget lock");
        let entry = caps.entry(id.clone()).or_default();
        entry.cap = entry.cap.max(cap);
    }

    /// Ingest one node's observed cumulative spend for a capped value (from a
    /// `UsageReport`). Replaces the node's figure (cumulative, not a delta) and
    /// returns any GB-6 alerts the fleet-wide crossing newly triggers — raised
    /// from the enforcement telemetry, at the point the fleet learns of the
    /// crossing, so it is never reconstructed from logs later.
    pub fn report_spend(&self, node_id: &str, id: &CapId, cap: u64, spent: u64) -> Vec<Alert> {
        let mut caps = self.caps.lock().expect("budget lock");
        let entry = caps.entry(id.clone()).or_default();
        entry.cap = entry.cap.max(cap);
        entry
            .per_node_spend
            .insert(node_id.to_string(), spent);
        Self::fleet_alerts(id, entry)
    }

    /// The GB-6 alerts a fleet-wide total newly crosses (soft 80%, hard cap),
    /// fired once each per window. Mutates the fired-latches on `entry`.
    fn fleet_alerts(id: &CapId, entry: &mut CapState) -> Vec<Alert> {
        let mut out = Vec::new();
        if entry.cap == 0 {
            return out;
        }
        let total = entry.total_spend();
        let frac = total as f64 / entry.cap as f64;
        if !entry.soft_fired && frac >= SOFT_ALERT_FRACTION {
            entry.soft_fired = true;
            out.push(Alert {
                kind: AlertKind::SoftThreshold {
                    fraction: SOFT_ALERT_FRACTION,
                },
                id: id.clone(),
                cap: entry.cap,
                spend: total,
                node: FLEET_NODE.to_string(),
            });
        }
        if !entry.hard_fired && total >= entry.cap {
            entry.hard_fired = true;
            out.push(Alert {
                kind: AlertKind::HardCap,
                id: id.clone(),
                cap: entry.cap,
                spend: total,
                node: FLEET_NODE.to_string(),
            });
        }
        out
    }

    /// Rebalance one node's shares across every capped value it knows about,
    /// returning the allocation to grant it. The share for node `n` on value `v`
    /// is: the tokens `n` already spent on `v` (it must keep its own consumed
    /// portion), PLUS a slice of the remaining fleet headroom weighted by `n`'s
    /// share of observed spend — the "hot node gets a bigger slice" rule — with
    /// a cold-node floor so a node that has spent nothing still gets a starting
    /// slice. The sum of shares never exceeds the cap: headroom is divided, not
    /// invented.
    pub fn shares_for(&self, node_id: &str) -> Vec<ShareAllocation> {
        let caps = self.caps.lock().expect("budget lock");
        let mut out = Vec::new();
        for (id, entry) in caps.iter() {
            if entry.cap == 0 {
                continue; // uncapped: no share to grant
            }
            out.push(ShareAllocation {
                id: id.clone(),
                cap: entry.cap,
                share: Self::share_for_node(node_id, entry),
            });
        }
        out
    }

    /// The pure share-allocation math for one node against one cap state.
    fn share_for_node(node_id: &str, entry: &CapState) -> u64 {
        let cap = entry.cap;
        let total = entry.total_spend();
        let node_count = entry.per_node_spend.len().max(1) as u64;
        let node_spent = entry.per_node_spend.get(node_id).copied().unwrap_or(0);

        // Fleet headroom left to divide.
        let headroom = cap.saturating_sub(total);

        // Weight the headroom by this node's share of observed spend (hot node
        // bigger slice). A cold fleet (no spend anywhere) divides the headroom
        // by the cold-node floor so every node gets a starting slice.
        let weighted_headroom = if total == 0 {
            // No spend anywhere yet: hand each node a floor slice of the cap.
            ((cap as f64) * COLD_NODE_FLOOR_FRACTION).floor() as u64
        } else if node_spent == 0 {
            // A cold node in an otherwise-hot fleet: a floor slice of the
            // headroom, split across the cold nodes so hot nodes keep the bulk.
            let cold_floor = ((headroom as f64) * COLD_NODE_FLOOR_FRACTION / node_count as f64)
                .floor() as u64;
            cold_floor
        } else {
            // Hot node: its slice of the headroom in proportion to its spend.
            ((headroom as f64) * (node_spent as f64 / total as f64)).floor() as u64
        };

        // The node keeps its own consumed portion plus its weighted headroom
        // slice, capped at the fleet cap (never grant past the cap).
        node_spent
            .saturating_add(weighted_headroom)
            .min(cap)
    }

    /// The fleet-wide observed spend for a value (sum across nodes) — the
    /// alertable, queryable number.
    pub fn total_spend(&self, id: &CapId) -> u64 {
        self.caps
            .lock()
            .expect("budget lock")
            .get(id)
            .map(CapState::total_spend)
            .unwrap_or(0)
    }

    /// Every capped value the fleet knows about, with its cap and current total
    /// spend — the surfaced ledger for logging/tests.
    pub fn ledger(&self) -> Vec<(CapId, u64, u64)> {
        self.caps
            .lock()
            .expect("budget lock")
            .iter()
            .filter(|(_, e)| e.cap > 0)
            .map(|(id, e)| (id.clone(), e.cap, e.total_spend()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(v: &str) -> CapId {
        CapId::new("team", v)
    }

    #[test]
    fn a_cold_fleet_hands_each_node_a_floor_slice() {
        let fb = FleetBudgets::new();
        fb.observe_cap(&id("ml"), 100_000);
        // No spend reported yet: each node's share is the cold floor of the cap.
        let shares = fb.shares_for("n1");
        assert_eq!(shares.len(), 1);
        assert_eq!(shares[0].cap, 100_000);
        assert_eq!(shares[0].share, 10_000); // 10% floor of 100k
    }

    #[test]
    fn a_hot_node_gets_a_bigger_slice_of_the_headroom() {
        let fb = FleetBudgets::new();
        // n1 spent 40k, n2 spent 10k; total 50k, headroom 50k.
        fb.report_spend("n1", &id("ml"), 100_000, 40_000);
        fb.report_spend("n2", &id("ml"), 100_000, 10_000);

        let n1 = &fb.shares_for("n1")[0];
        let n2 = &fb.shares_for("n2")[0];
        // n1: 40k spent + 80% of 50k headroom = 40k + 40k = 80k.
        assert_eq!(n1.share, 80_000);
        // n2: 10k spent + 20% of 50k headroom = 10k + 10k = 20k.
        assert_eq!(n2.share, 20_000);
        // The two shares sum to exactly the cap — headroom divided, not invented.
        assert_eq!(n1.share + n2.share, 100_000);
    }

    #[test]
    fn continuous_rebalance_shifts_the_slice_toward_the_hotter_node() {
        let fb = FleetBudgets::new();
        // First round: even spend -> even-ish shares.
        fb.report_spend("n1", &id("ml"), 100_000, 20_000);
        fb.report_spend("n2", &id("ml"), 100_000, 20_000);
        let before = fb.shares_for("n1")[0].share; // 20k + 50% of 60k = 50k

        // n1 heats up: it now dominates observed spend.
        fb.report_spend("n1", &id("ml"), 100_000, 55_000);
        fb.report_spend("n2", &id("ml"), 100_000, 20_000);
        let after = fb.shares_for("n1")[0].share;
        // n1's slice grew with its spend (continuous rebalance).
        assert!(after > before, "hot node's share grew: {before} -> {after}");
        // total 75k, headroom 25k; n1: 55k + 55/75*25k ≈ 55k + 18k = 73k.
        assert_eq!(after, 73_333);
    }

    #[test]
    fn a_cold_node_in_a_hot_fleet_gets_a_floor_of_the_headroom() {
        let fb = FleetBudgets::new();
        fb.report_spend("hot", &id("ml"), 100_000, 60_000);
        fb.report_spend("cold", &id("ml"), 100_000, 0);
        // headroom 40k; cold floor = 10% of 40k / 2 nodes = 2k.
        let cold = &fb.shares_for("cold")[0];
        assert_eq!(cold.share, 2_000);
        // The hot node keeps the bulk: 60k spent + 100% of headroom weight.
        let hot = &fb.shares_for("hot")[0];
        assert_eq!(hot.share, 100_000); // 60k + 40k headroom (all spend is its)
    }

    #[test]
    fn shares_never_exceed_the_cap() {
        let fb = FleetBudgets::new();
        // A node reports spend already AT the cap: its share is clamped to cap.
        fb.report_spend("n1", &id("ml"), 100_000, 100_000);
        let s = &fb.shares_for("n1")[0];
        assert!(s.share <= s.cap);
        assert_eq!(s.share, 100_000);
    }

    #[test]
    fn uncapped_values_get_no_share_grant() {
        let fb = FleetBudgets::new();
        fb.report_spend("n1", &CapId::new("region", "eu"), 0, 5_000);
        assert!(fb.shares_for("n1").is_empty(), "uncapped -> no share");
    }

    // --- GB-6 fleet-wide alerts from the enforcement telemetry ---------------

    #[test]
    fn fleet_wide_spend_crossing_eighty_percent_fires_one_soft_alert() {
        let fb = FleetBudgets::new();
        // Two nodes each below 80% alone, but together cross it.
        let a = fb.report_spend("n1", &id("ml"), 100_000, 45_000);
        assert!(a.is_empty(), "45k alone is below 80%");
        let b = fb.report_spend("n2", &id("ml"), 100_000, 40_000); // total 85k
        assert_eq!(b.len(), 1);
        assert!(matches!(b[0].kind, AlertKind::SoftThreshold { .. }));
        assert_eq!(b[0].spend, 85_000);
        assert_eq!(b[0].node, FLEET_NODE);
        // A further report past 80% does not re-fire the soft alert.
        let c = fb.report_spend("n1", &id("ml"), 100_000, 46_000); // total 86k
        assert!(c.is_empty());
    }

    #[test]
    fn fleet_wide_spend_hitting_the_cap_fires_the_hard_alert() {
        let fb = FleetBudgets::new();
        fb.report_spend("n1", &id("ml"), 100_000, 70_000); // soft fires here
        let hard = fb.report_spend("n2", &id("ml"), 100_000, 35_000); // total 105k
        // 105k crosses the cap -> hard alert (soft already fired).
        assert!(hard.iter().any(|a| a.kind == AlertKind::HardCap));
        let hard_alert = hard.iter().find(|a| a.kind == AlertKind::HardCap).unwrap();
        assert_eq!(hard_alert.cap, 100_000);
        assert_eq!(hard_alert.spend, 105_000);
    }

    #[test]
    fn the_ledger_surfaces_cap_and_total_spend_per_value() {
        let fb = FleetBudgets::new();
        fb.report_spend("n1", &id("ml"), 100_000, 30_000);
        fb.report_spend("n2", &id("ml"), 100_000, 20_000);
        fb.report_spend("n1", &id("infra"), 50_000, 5_000);
        let mut ledger = fb.ledger();
        ledger.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(ledger.len(), 2);
        // ("team=infra", 50k, 5k) sorts before ("team=ml", 100k, 50k).
        assert_eq!(ledger[0].0, id("infra"));
        assert_eq!(ledger[0].2, 5_000);
        assert_eq!(ledger[1].0, id("ml"));
        assert_eq!(ledger[1].2, 50_000);
    }
}
