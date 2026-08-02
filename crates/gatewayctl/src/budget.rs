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
    /// Per-node cumulative-spend SNAPSHOTS taken when a canary analysis window
    /// opens, so the config-canary spend signal compares a per-WINDOW delta
    /// rather than lifetime cumulative spend. Keyed by node id → the node's
    /// total spend at window open. Absent node → the window started at 0 for it.
    /// This is the spend analogue of the telemetry sink's `reset_many`: it does
    /// not mutate the ledger (cumulative truth is preserved for share
    /// allocation and GB-6 alerts), it only marks the window's zero point.
    spend_window_open: Mutex<BTreeMap<String, u64>>,
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
        let before = entry.total_spend();
        entry
            .per_node_spend
            .insert(node_id.to_string(), spent);
        // Windowed caps: node spend is monotone WITHIN a billing window, so a
        // total that DROPS means a node rolled into a new window (nodes reset
        // lazily and report the new, smaller figure). Re-arm the fleet GB-6
        // latches so the new window alerts again. At a boundary, interleaved
        // reports (one node rolled, another not yet) can re-fire once early —
        // bounded by report cadence plus clock skew, the same honesty class
        // as the partition overspend bound.
        if entry.total_spend() < before {
            entry.soft_fired = false;
            entry.hard_fired = false;
        }
        Self::fleet_alerts(id, entry)
    }

    /// Reclaim a departed node's spend from the fleet ledger (called on
    /// disconnect). Without this, a dead node's consumed tokens keep counting
    /// toward `total_spend` forever — starving the surviving hot nodes of
    /// grantable headroom — and its lingering entry inflates `node_count`,
    /// shrinking every cold-node floor slice. Removing the entry across every
    /// capped value restores the survivors' headroom on the next rebalance.
    ///
    /// This does NOT re-arm the GB-6 latches: an alert that already fired for a
    /// crossing stays fired for the window (the crossing genuinely happened).
    pub fn forget_node(&self, node_id: &str) {
        let mut caps = self.caps.lock().expect("budget lock");
        for entry in caps.values_mut() {
            entry.per_node_spend.remove(node_id);
        }
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
    /// is its weighted slice of the cap: cold nodes (zero observed spend) reserve
    /// a floor slice carved out of the headroom FIRST, then the hot nodes divide
    /// the pool that remains of the cap by their share of observed spend — the
    /// "hot node gets a bigger slice" rule. The sum of shares over all nodes never
    /// exceeds the cap for ANY fleet size or hot/cold mix — including a fleet whose
    /// cumulative reported spend already exceeds the cap — because the cap is
    /// partitioned, not invented (see [`FleetBudgets::share_for_node`] for the
    /// proof of the conservation invariant).
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
    ///
    /// CONSERVATION INVARIANT (proven): the sum of `share_for_node(n)` over every
    /// node in `entry` is `<= cap`, for ANY node count and ANY hot/cold mix —
    /// including a fleet whose reported cumulative spend already exceeds the cap.
    /// The cap is PARTITIONED, never invented. The partition is:
    ///
    /// 1. **Cold fleet** (`total == 0`): every node gets an even floor slice
    ///    `floor(cap * FLOOR / node_count)`. The `node_count` divisor is what
    ///    keeps K nodes from each claiming `FLOOR` of the cap and inflating the
    ///    real limit to `K * FLOOR`. Sum `<= cap * FLOOR <= cap`.
    /// 2. **Mixed / hot fleet** (`total > 0`): the cold nodes (zero observed
    ///    spend) each reserve a floor slice carved out of the *headroom* and
    ///    split by `node_count`, so all cold floors together take at most
    ///    `FLOOR` of the headroom. What is left of the CAP after that cold
    ///    reservation — the `hot_pool` — is divided among the hot nodes strictly
    ///    by spend weight: `floor(hot_pool * node_spent / total)`.
    ///
    /// Why this conserves even when `total > cap` (the case the previous version
    /// got wrong): a hot node is granted its *weighted slice of the pool*, it
    /// does NOT keep its raw `node_spent`. The old formula returned
    /// `node_spent + slice`, and since `sum(node_spent) == total`, once the fleet
    /// had collectively reported more than the cap the grants summed to `total`,
    /// silently raising the real fleet limit to `total`. Dividing a fixed
    /// `hot_pool` by weight bounds the hot grants to `hot_pool` exactly:
    /// `sum(floor(hot_pool * s_i / total)) <= hot_pool * (sum s_i)/total ==
    /// hot_pool`, and `hot_pool + cold_reservation == cap` by construction, so
    /// the fleet total can never exceed the cap while connected. A node that has
    /// already spent past its recomputed share is handled by the data plane's own
    /// per-node deny against the cap ([`gateway_core::budget::LocalBudget`]) — it
    /// simply stops, it does not get retroactively granted headroom that does not
    /// exist.
    fn share_for_node(node_id: &str, entry: &CapState) -> u64 {
        let cap = entry.cap;
        let total = entry.total_spend();
        let node_count = entry.per_node_spend.len().max(1) as u64;
        let node_spent = entry.per_node_spend.get(node_id).copied().unwrap_or(0);

        if cap == 0 {
            return 0; // uncapped: no share to grant
        }

        // A cold fleet: no node has spent anything yet. Divide a floor slice of
        // the whole cap EVENLY across the nodes. Sum across nodes is
        // <= cap * FLOOR <= cap.
        if total == 0 {
            return ((cap as f64) * COLD_NODE_FLOOR_FRACTION / node_count as f64).floor() as u64;
        }

        // Fleet headroom left before the cap (0 once the fleet is at/over cap).
        let headroom = cap.saturating_sub(total);

        // The per-cold-node floor slice, carved out of the headroom and split so
        // the cold nodes collectively take at most COLD_NODE_FLOOR_FRACTION of
        // the headroom, leaving the rest of the CAP to the hot nodes. When the
        // fleet is already at/over cap, headroom is 0 and cold nodes get nothing.
        let cold_count = entry
            .per_node_spend
            .values()
            .filter(|&&s| s == 0)
            .count() as u64;
        let cold_floor_each = if cold_count == 0 {
            0
        } else {
            ((headroom as f64) * COLD_NODE_FLOOR_FRACTION / node_count as f64).floor() as u64
        };

        if node_spent == 0 {
            // A cold node in an otherwise-hot fleet: just its reserved floor.
            return cold_floor_each.min(cap);
        }

        // A hot node divides the pool that REMAINS of the cap after the cold
        // reservation, by its spend weight. It is granted its weighted slice of
        // that pool — NOT `node_spent + slice`, which would let the grants sum to
        // `total` and blow past the cap once the fleet is collectively over it.
        // Because the hot slices sum to at most `hot_pool` and
        // `hot_pool + cold_reservation == cap`, the fleet total is conserved.
        let cold_reservation = cold_floor_each.saturating_mul(cold_count);
        let hot_pool = cap.saturating_sub(cold_reservation);
        let hot_slice = ((hot_pool as f64) * (node_spent as f64 / total as f64)).floor() as u64;
        hot_slice.min(cap)
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

    /// One node's total observed spend across EVERY capped value (the sum of its
    /// per-cap figures). This is the CUMULATIVE lifetime figure used by the share
    /// allocator and GB-6 alerts; the config-canary analysis reads the per-WINDOW
    /// delta ([`FleetBudgets::node_windowed_spend`]) instead, so a fresh canary
    /// node is not judged against a long-running baseline's lifetime total.
    pub fn node_total_spend(&self, node_id: &str) -> u64 {
        self.caps
            .lock()
            .expect("budget lock")
            .values()
            .map(|e| e.per_node_spend.get(node_id).copied().unwrap_or(0))
            .sum()
    }

    /// Open a canary analysis spend WINDOW for a set of nodes: snapshot each
    /// node's current cumulative total as the window's zero point. Mirrors the
    /// telemetry sink's `reset_many` (which zeroes the infra window) WITHOUT
    /// destroying the cumulative ledger — the spend delta since this snapshot is
    /// the per-window spend rate the analysis compares. Snapshotting BOTH the
    /// canary and the baseline nodes makes both a per-window figure, so the
    /// comparison is apples-to-apples regardless of node uptime: a freshly-added
    /// canary vs a long-running baseline no longer masks (or fabricates) a spend
    /// anomaly by comparing lifetime totals.
    pub fn open_spend_window(&self, node_ids: &[String]) {
        let totals: BTreeMap<String, u64> = node_ids
            .iter()
            .map(|id| (id.clone(), self.node_total_spend(id)))
            .collect();
        let mut open = self.spend_window_open.lock().expect("spend window lock");
        for (id, total) in totals {
            open.insert(id, total);
        }
    }

    /// One node's spend OVER the current analysis window: its cumulative total
    /// now, minus the snapshot taken at [`FleetBudgets::open_spend_window`]. When
    /// no window was opened for the node (no snapshot), this is the cumulative
    /// total — the zero-window test/demo path where every node is seeded fresh in
    /// the same window, so cumulative == windowed there. `saturating_sub` guards
    /// the (impossible-in-practice) case of a cumulative figure moving backward.
    pub fn node_windowed_spend(&self, node_id: &str) -> u64 {
        let total = self.node_total_spend(node_id);
        let base = self
            .spend_window_open
            .lock()
            .expect("spend window lock")
            .get(node_id)
            .copied()
            .unwrap_or(0);
        total.saturating_sub(base)
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
        // headroom 40k; cold floor = 10% of 40k / 2 nodes = 2k, reserved OUT of
        // the headroom before the hot node divides the remainder.
        let cold = &fb.shares_for("cold")[0];
        assert_eq!(cold.share, 2_000);
        // The hot node keeps its 60k spend + the remaining headroom (40k - 2k
        // reserved for the cold node) = 60k + 38k = 98k. NOT 100k: the cold
        // floor is carved out, so the two shares sum to exactly the cap.
        let hot = &fb.shares_for("hot")[0];
        assert_eq!(hot.share, 98_000);
        assert_eq!(cold.share + hot.share, 100_000, "conserved: sum == cap");
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

    /// The core conservation invariant: across MANY nodes, mixed hot and cold,
    /// the sum of shares must never exceed the cap. This is what an over-
    /// allocated fleet violates — silently raising the real fleet limit.
    fn sum_of_shares(fb: &FleetBudgets, nodes: &[&str], v: &str) -> u64 {
        nodes
            .iter()
            .map(|n| fb.shares_for(n).iter().find(|a| a.id == id(v)).map_or(0, |a| a.share))
            .sum()
    }

    #[test]
    fn a_cold_fleet_conserves_the_cap_across_many_nodes() {
        let fb = FleetBudgets::new();
        fb.observe_cap(&id("ml"), 100_000);
        // 20 cold nodes must NOT each claim 10% of the cap (which would sum to
        // 2x the cap). The node_count divisor keeps the sum <= cap.
        let nodes: Vec<String> = (0..20).map(|i| format!("n{i}")).collect();
        for n in &nodes {
            fb.report_spend(n, &id("ml"), 100_000, 0);
        }
        let refs: Vec<&str> = nodes.iter().map(String::as_str).collect();
        let sum = sum_of_shares(&fb, &refs, "ml");
        assert!(sum <= 100_000, "20 cold nodes must not inflate the cap: {sum}");
        // Each node gets an even floor slice: 10% of 100k / 20 = 500.
        assert_eq!(fb.shares_for("n0")[0].share, 500);
    }

    #[test]
    fn a_mixed_hot_and_cold_fleet_conserves_the_cap() {
        let fb = FleetBudgets::new();
        // 3 hot nodes and 5 cold nodes; the sum of every node's share must not
        // exceed the cap even though each is granted independently.
        fb.report_spend("h1", &id("ml"), 100_000, 30_000);
        fb.report_spend("h2", &id("ml"), 100_000, 15_000);
        fb.report_spend("h3", &id("ml"), 100_000, 5_000);
        for c in ["c1", "c2", "c3", "c4", "c5"] {
            fb.report_spend(c, &id("ml"), 100_000, 0);
        }
        let refs = ["h1", "h2", "h3", "c1", "c2", "c3", "c4", "c5"];
        let sum = sum_of_shares(&fb, &refs, "ml");
        assert!(sum <= 100_000, "mixed fleet must conserve the cap: {sum}");
    }

    /// The case the previous allocator got wrong (the HIGH): a fleet whose
    /// CUMULATIVE reported spend already EXCEEDS the cap. Every node reports a
    /// cumulative figure; nothing stops their sum from crossing the fleet cap.
    /// The old math granted each hot node `node_spent + slice`, and since the
    /// per-node spends sum to the (over-cap) total, the grants summed to `total`
    /// — silently raising the real fleet limit to whatever the fleet had already
    /// spent. This asserts the grants are re-partitioned back down to the cap.
    #[test]
    fn a_fleet_already_over_the_cap_is_repartitioned_back_to_the_cap() {
        let fb = FleetBudgets::new();
        // Five nodes each reporting spend near the cap: total 375k, 3.75x the
        // 100k cap. Fully connected, no partition.
        for (n, spent) in [
            ("n1", 90_000),
            ("n2", 80_000),
            ("n3", 70_000),
            ("n4", 75_000),
            ("n5", 60_000),
        ] {
            fb.report_spend(n, &id("ml"), 100_000, spent);
        }
        let refs = ["n1", "n2", "n3", "n4", "n5"];
        let sum = sum_of_shares(&fb, &refs, "ml");
        // Old code: sum == 375_000 (== total), 3.75x the cap. New: <= cap.
        assert!(
            sum <= 100_000,
            "an over-cap fleet must be repartitioned to the cap, not granted its \
             full over-cap spend: sum={sum}"
        );
        // The hottest node still gets the biggest slice (weight preserved).
        let n1 = fb.shares_for("n1")[0].share;
        let n5 = fb.shares_for("n5")[0].share;
        assert!(n1 > n5, "hottest node keeps the biggest slice: {n1} vs {n5}");
    }

    /// The pathological version of the same defect at large fleet size: 30 hot
    /// nodes each reporting spend at the cap. The old allocator would grant each
    /// its clamped `cap`, summing to 30x the cap.
    #[test]
    fn many_nodes_each_at_the_cap_do_not_multiply_the_cap() {
        let fb = FleetBudgets::new();
        let nodes: Vec<String> = (0..30).map(|i| format!("n{i}")).collect();
        for n in &nodes {
            // Each node reports it has spent the whole cap.
            fb.report_spend(n, &id("ml"), 100_000, 100_000);
        }
        let refs: Vec<&str> = nodes.iter().map(String::as_str).collect();
        let sum = sum_of_shares(&fb, &refs, "ml");
        assert!(
            sum <= 100_000,
            "30 nodes each at the cap must not sum to 30x the cap: {sum}"
        );
    }

    #[test]
    fn forget_node_reclaims_a_departed_nodes_spend() {
        let fb = FleetBudgets::new();
        fb.report_spend("survivor", &id("ml"), 100_000, 20_000);
        fb.report_spend("gone", &id("ml"), 100_000, 50_000);
        // Before: total 70k, survivor's slice of the 30k headroom is small.
        assert_eq!(fb.total_spend(&id("ml")), 70_000);
        fb.forget_node("gone");
        // After: the departed node's 50k is reclaimed; the survivor now divides
        // the full headroom again.
        assert_eq!(fb.total_spend(&id("ml")), 20_000);
        let s = &fb.shares_for("survivor")[0];
        // 20k spent + 100% of the restored 80k headroom = 100k.
        assert_eq!(s.share, 100_000);
    }

    #[test]
    fn windowed_spend_is_the_delta_since_the_window_opened() {
        let fb = FleetBudgets::new();
        // A long-running node that has already spent a lot before the window.
        fb.report_spend("canary", &id("ml"), 10_000_000, 500_000);
        // Open the canary analysis window: this node's 500k is now the zero point.
        fb.open_spend_window(&["canary".to_string()]);
        // Without the delta, `node_total_spend` still reports the lifetime 500k.
        assert_eq!(fb.node_total_spend("canary"), 500_000);
        // The windowed figure starts at 0 — nothing spent SINCE the window opened.
        assert_eq!(fb.node_windowed_spend("canary"), 0);
        // It spends 3000 more this window.
        fb.report_spend("canary", &id("ml"), 10_000_000, 503_000);
        assert_eq!(fb.node_windowed_spend("canary"), 3_000);
        assert_eq!(fb.node_total_spend("canary"), 503_000, "cumulative untouched");
    }

    #[test]
    fn windowed_spend_without_a_snapshot_is_the_cumulative_total() {
        // The zero-window test/demo path: no `open_spend_window` call, so the
        // windowed figure equals the cumulative total (every node seeded fresh in
        // the same window there, so cumulative == windowed).
        let fb = FleetBudgets::new();
        fb.report_spend("n1", &id("ml"), 100_000, 4_200);
        assert_eq!(fb.node_windowed_spend("n1"), 4_200);
    }

    #[test]
    fn a_fresh_canary_vs_a_long_running_baseline_compares_per_window_spend() {
        // The exact false-negative the cumulative comparison masked: a freshly-
        // added canary against a long-running baseline. With cumulative totals the
        // baseline's lifetime spend dwarfs the canary's, hiding a per-window spike.
        // With the delta, both are measured over the SAME window.
        let fb = FleetBudgets::new();
        // Baseline has been running a long time: huge cumulative spend.
        fb.report_spend("baseline", &id("ml"), 10_000_000, 900_000);
        // Canary just joined: small cumulative spend.
        fb.report_spend("canary", &id("ml"), 10_000_000, 1_000);
        // Open the window over BOTH — their lifetime totals are the zero points.
        fb.open_spend_window(&["baseline".to_string(), "canary".to_string()]);
        // Over the window the canary spends 5000 (a spike) while the baseline
        // spends its usual 500.
        fb.report_spend("baseline", &id("ml"), 10_000_000, 900_500);
        fb.report_spend("canary", &id("ml"), 10_000_000, 6_000);
        // Cumulative would say baseline(900500) >> canary(6000): NO anomaly.
        assert!(fb.node_total_spend("baseline") > fb.node_total_spend("canary"));
        // Windowed correctly says the canary spent 10x the baseline THIS window.
        assert_eq!(fb.node_windowed_spend("baseline"), 500);
        assert_eq!(fb.node_windowed_spend("canary"), 5_000);
        assert!(
            fb.node_windowed_spend("canary") > fb.node_windowed_spend("baseline") * 2,
            "the per-window spike is visible where the cumulative comparison hid it"
        );
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

    #[test]
    fn a_fleet_total_drop_re_arms_the_gb6_latches() {
        // Node spend is monotone WITHIN a window; a dropping total means a
        // node rolled into a new billing window. The fleet latches re-arm so
        // the new window alerts again.
        let fb = FleetBudgets::new();
        let alerts = fb.report_spend("n1", &id("ml"), 100_000, 85_000);
        assert_eq!(alerts.len(), 1, "soft fires in window 1");
        // Same window, higher spend: latched.
        assert!(fb.report_spend("n1", &id("ml"), 100_000, 90_000).is_empty());
        // The node rolled: it reports the new window's small figure.
        assert!(fb.report_spend("n1", &id("ml"), 100_000, 1_000).is_empty());
        // Crossing again in the new window fires again.
        let again = fb.report_spend("n1", &id("ml"), 100_000, 86_000);
        assert_eq!(again.len(), 1, "soft re-fires after the window rolled");
    }
}
