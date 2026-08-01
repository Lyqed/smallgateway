//! GB-5 spend caps as budget shares, and the GB-6 alert primitives that fire
//! from the enforcement layer (docs/01 Q4; docs/02 "GB-5 at fleet scale —
//! budget shares"; docs/04 Phase 3).
//!
//! ## The trade-off, chosen (docs/01 Q4)
//!
//! A spend limit per attribution value enforced across N data planes is the
//! project's hard distributed-systems problem. A **central counter** adds a
//! hop and a SPOF on every request; **pure-local buckets** overspend
//! unboundedly. We choose **budget shares**: the control plane allocates each
//! data plane a SHARE of the cap from observed spend telemetry and rebalances
//! continuously, a data plane spends freely against its local share, and it
//! escalates to a SYNCHRONOUS check with the control plane only above
//! ~90% local-share consumption. The common path has no per-request hop and no
//! SPOF; only the near-limit path coordinates.
//!
//! ## Bounded overspend (the whole point)
//!
//! When a node cannot reach the control plane (partition) it must fail to a
//! DOCUMENTED bounded-overspend policy rather than blocking all traffic or
//! overspending unboundedly. The policy here: **a node spends up to its
//! currently-held share and then stops** (fail closed on the cap). The bound is
//! therefore *the sum of the unconsumed shares still held across the fleet at
//! the moment of the partition* — never more than the cap plus the last
//! allocated slack, and MEASURABLE: [`LocalBudget`] reports its own overspend
//! against its share as a number (see the crate tests and the partition demo).
//!
//! ## Cap unit (docs/01 Q3)
//!
//! Caps are counted in **tokens**. The live estimate (`chars/4`, the
//! [`crate::metering::Meter`]) meters a stream incrementally for mid-stream
//! enforcement; the provider's terminal usage frame is the authoritative count
//! that the running total reconciles against at stream end. A cap tightened
//! mid-stream does NOT retroactively apply — the stream is metered by the
//! version it bound (docs/03 limitation 2). All of that reuses the existing
//! Meter; this module only owns the counters and the decision.

use std::collections::BTreeMap;

/// The consumption threshold at which a data plane stops spending freely
/// against its local share and escalates to a synchronous control-plane check
/// (docs/01 Q4: "escalate to synchronous checks only above ~90% consumption").
/// A fraction of the share, not the cap: the share is the node's local budget.
pub const ESCALATION_FRACTION: f64 = 0.90;

/// One capped spender: an attribution key and the resolved value that names it.
/// `team=ml-research` is one `CapId`; the cap is enforced per distinct value of
/// the key, fleet-wide (docs/02: "a spend limit per attribution value").
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CapId {
    pub key: String,
    pub value: String,
}

impl CapId {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> CapId {
        CapId {
            key: key.into(),
            value: value.into(),
        }
    }
}

impl std::fmt::Display for CapId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}={}", self.key, self.value)
    }
}

/// A composed spend cap for one attribution key: a fleet default applied to
/// every value of the key, plus Git-reviewed per-value overrides. Composition
/// down the scoped chain (fleet → project → route → app) is a lower-scope
/// override of the default and of any per-value entry, exactly like the pins
/// (docs/02: "a per-app TPM override a route-level values file").
///
/// A cap of `None` (no default, no override for a value) means UNCAPPED — the
/// key simply is not spend-limited. `Some(0)` is a hard stop (spend nothing),
/// distinct from uncapped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyCap {
    /// The default cap in tokens for every value of this key. `None` → the
    /// key is uncapped unless a per-value override sets a cap.
    pub default: Option<u64>,
    /// Per-value cap overrides in tokens. A value present here uses this
    /// number instead of `default`; `None` inside the map is an explicit
    /// "this value is uncapped" override of a capped default.
    pub overrides: BTreeMap<String, Option<u64>>,
}

impl KeyCap {
    /// The cap in tokens for one resolved value, applying the per-value
    /// override when present, else the default. `None` → uncapped.
    pub fn cap_for(&self, value: &str) -> Option<u64> {
        match self.overrides.get(value) {
            Some(override_cap) => *override_cap,
            None => self.default,
        }
    }

    /// Fold a lower scope's KeyCap over this one: the lower scope's `default`
    /// wins when it sets one, and its per-value entries override this scope's.
    /// This is the cap analog of the scoped-chain map merge (lower scope wins).
    pub fn compose_child(&self, child: &KeyCap) -> KeyCap {
        let mut overrides = self.overrides.clone();
        for (value, cap) in &child.overrides {
            overrides.insert(value.clone(), *cap);
        }
        KeyCap {
            default: child.default.or(self.default),
            overrides,
        }
    }
}

/// The verdict for one prospective spend against a local share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Spend freely: below the escalation threshold of the local share.
    Allow,
    /// At or above ~90% of the local share: keep serving THIS spend, but the
    /// data plane should escalate to a synchronous control-plane check to
    /// (re)confirm or grow its share before the next spend crosses the cap.
    Escalate,
    /// The cap for this value is reached: the spend must be refused with the
    /// GB-4 template (request start) or the stream cut (mid-stream). Carries
    /// the cap so the operator template can render it.
    Deny { cap: u64 },
}

impl Verdict {
    pub fn is_deny(&self) -> bool {
        matches!(self, Verdict::Deny { .. })
    }
}

/// One node's local budget for one [`CapId`]: the share the control plane
/// allocated it, and how much it has spent against that share. The counter is
/// in-memory (Postgres-backed durable counters are deferred; docs/03
/// limitation 3), and it is the ONLY per-request state on the common path —
/// no hop, no lock across the fleet.
///
/// `share` is the node's slice of the fleet-wide `cap`. `cap` is carried so a
/// [`Verdict::Deny`] can name the fleet cap in the operator template and so the
/// node never spends past the cap even if its share was over-allocated.
#[derive(Debug, Clone)]
pub struct LocalBudget {
    id: CapId,
    /// The fleet-wide cap in tokens (the number the operator wrote), or `None`
    /// for an uncapped value (every spend `Allow`s).
    cap: Option<u64>,
    /// The share of `cap` this node currently holds, in tokens. Re-set by the
    /// control plane's rebalance; a hot node gets a bigger slice.
    share: u64,
    /// Tokens spent against this share so far (this node, this budget).
    spent: u64,
}

impl LocalBudget {
    /// A node budget holding `share` tokens of a `cap`-token fleet cap. An
    /// uncapped budget (`cap: None`) never denies.
    pub fn new(id: CapId, cap: Option<u64>, share: u64) -> LocalBudget {
        LocalBudget {
            id,
            cap,
            share,
            spent: 0,
        }
    }

    pub fn id(&self) -> &CapId {
        &self.id
    }

    pub fn cap(&self) -> Option<u64> {
        self.cap
    }

    pub fn share(&self) -> u64 {
        self.share
    }

    pub fn spent(&self) -> u64 {
        self.spent
    }

    /// Tokens still available against the local share (0 once the share is
    /// consumed). Uncapped budgets report `u64::MAX`.
    pub fn remaining(&self) -> u64 {
        if self.cap.is_none() {
            return u64::MAX;
        }
        self.share.saturating_sub(self.spent)
    }

    /// The node's OVERSPEND against its share: how many tokens it has spent
    /// beyond the share it was allocated. Zero on the common path; positive
    /// only if a mid-stream cut let a running stream cross the share before it
    /// could be stopped (bounded by one in-flight stream's remaining tokens).
    /// This is the number the partition demo/test reports.
    pub fn overspend(&self) -> u64 {
        if self.cap.is_none() {
            return 0; // uncapped: no share to overspend
        }
        self.spent.saturating_sub(self.share)
    }

    /// Grow or shrink this node's share to `share` tokens (a control-plane
    /// rebalance). Spend is preserved — the node keeps its running total, only
    /// its ceiling moves.
    pub fn set_share(&mut self, share: u64) {
        self.share = share;
    }

    /// The fraction of the local share consumed, in [0, ∞). Above
    /// [`ESCALATION_FRACTION`] the node escalates. `0.0` for an uncapped or
    /// zero-share budget where the fraction is undefined/irrelevant.
    pub fn consumed_fraction(&self) -> f64 {
        if self.cap.is_none() || self.share == 0 {
            return 0.0;
        }
        self.spent as f64 / self.share as f64
    }

    /// Decide a prospective spend of `tokens` WITHOUT recording it: what a
    /// would-be request/stream-continuation should do. Pure — [`commit`] does
    /// the recording. `Deny` when the spend would take the running total past
    /// the fleet cap; `Escalate` at/above 90% of the share; else `Allow`.
    ///
    /// [`commit`]: LocalBudget::commit
    pub fn check(&self, tokens: u64) -> Verdict {
        let Some(cap) = self.cap else {
            return Verdict::Allow; // uncapped: never denies, never escalates
        };
        // Never let a node's spend cross the fleet cap, even if its share was
        // over-allocated: the cap is the hard fleet-wide limit.
        if self.spent.saturating_add(tokens) > cap {
            return Verdict::Deny { cap };
        }
        // The share is the node's local budget; crossing it is not itself a
        // hard deny (the fleet cap above is), but it means "spend the rest only
        // after re-confirming with the control plane" — the escalation path.
        let prospective = self.spent.saturating_add(tokens);
        if self.share > 0 && prospective as f64 >= ESCALATION_FRACTION * self.share as f64 {
            // Still within the fleet cap (checked above) but into the
            // near-limit band of the local share: keep serving, but escalate.
            if prospective > self.share {
                // Past the local share entirely: only the control plane can
                // authorize more (a bigger share). Under partition this
                // becomes the bounded-overspend deny — see `check_partitioned`.
                return Verdict::Escalate;
            }
            return Verdict::Escalate;
        }
        Verdict::Allow
    }

    /// The partition variant of [`check`]: the node CANNOT reach the control
    /// plane, so escalation is impossible. It therefore spends up to its
    /// currently-held share and then hard-denies — the documented
    /// bounded-overspend policy. A spend that fits within the share is allowed;
    /// one that would cross the share (or the cap) is denied. This is what makes
    /// the overspend bounded and measurable: without a reachable control plane a
    /// node can never spend past the share it already holds.
    ///
    /// [`check`]: LocalBudget::check
    pub fn check_partitioned(&self, tokens: u64) -> Verdict {
        let Some(cap) = self.cap else {
            return Verdict::Allow;
        };
        // Bounded by BOTH the share (no control plane to grow it) and the cap.
        let ceiling = self.share.min(cap);
        if self.spent.saturating_add(tokens) > ceiling {
            return Verdict::Deny { cap };
        }
        Verdict::Allow
    }

    /// Record `tokens` as spent. Returns the new running total. Called after a
    /// stream's tokens are metered (incrementally mid-stream, and reconciled at
    /// end against the terminal usage frame).
    pub fn commit(&mut self, tokens: u64) -> u64 {
        self.spent = self.spent.saturating_add(tokens);
        self.spent
    }

    /// Reconcile the running estimate to the authoritative terminal count for a
    /// stream: replace `estimated` tokens already committed for this stream with
    /// the `authoritative` figure (docs/01 Q3 — the provider's usage frame is
    /// the billing number). A no-op when they agree.
    pub fn reconcile(&mut self, estimated: u64, authoritative: u64) {
        if authoritative >= estimated {
            self.spent = self.spent.saturating_add(authoritative - estimated);
        } else {
            self.spent = self.spent.saturating_sub(estimated - authoritative);
        }
    }
}

/// A GB-6 alert raised AT the point of enforcement (docs/04 Phase 3: "alert
/// rules firing from the enforcement layer itself — not reconstructed later
/// from logs"). It carries the attribution value, the cap, the current spend,
/// and the node/fleet context, so a sink can route it without re-deriving
/// anything.
#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub kind: AlertKind,
    pub id: CapId,
    /// The fleet-wide cap in tokens.
    pub cap: u64,
    /// The spend that crossed the threshold, in tokens (fleet-wide when the
    /// control plane raises it; node-local when a data plane raises it).
    pub spend: u64,
    /// The node the enforcement happened on (`<control-plane>` when the control
    /// plane's allocator raises it fleet-wide).
    pub node: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AlertKind {
    /// A spender crossed the soft threshold (default 80% of the cap).
    SoftThreshold {
        /// The threshold fraction that was crossed, e.g. 0.8.
        fraction: f64,
    },
    /// A spender hit the hard cap: further spend is refused with GB-4.
    HardCap,
}

/// The soft-alert threshold as a fraction of the cap (docs/04 Phase 3: "crosses
/// a soft threshold (e.g. 80%)").
pub const SOFT_ALERT_FRACTION: f64 = 0.80;

impl AlertKind {
    fn label(self) -> String {
        match self {
            AlertKind::SoftThreshold { fraction } => {
                format!("SOFT-{}%", (fraction * 100.0).round() as u64)
            }
            AlertKind::HardCap => "HARD-CAP".to_string(),
        }
    }
}

impl std::fmt::Display for Alert {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pct = if self.cap == 0 {
            0.0
        } else {
            self.spend as f64 / self.cap as f64 * 100.0
        };
        write!(
            f,
            "[gb6 {}] spender={} spend={}/{} tokens ({:.0}%) node={}",
            self.kind.label(),
            self.id,
            self.spend,
            self.cap,
            pct,
            self.node,
        )
    }
}

/// Where an alert is delivered. Pluggable so a deployment routes GB-6 alerts to
/// a pager, a bus, or a metrics side-channel; the milestone ships a
/// log-structured sink and a webhook-shaped emitter, and richer sinks are noted
/// deferred in the crate READMEs.
pub trait AlertSink: Send + Sync {
    fn emit(&self, alert: &Alert);
}

/// The default sink: a structured log line at the point of enforcement, so the
/// alert cannot be missed even with no external system wired.
#[derive(Debug, Default)]
pub struct LogAlertSink;

impl AlertSink for LogAlertSink {
    fn emit(&self, alert: &Alert) {
        // `log` is a dependency of the binaries, not of gateway-core; keep the
        // core sink dependency-free by writing the same structured line to
        // stderr. The binaries wrap this (or replace it) with a `log`-backed
        // sink; the shape is identical so the demo greps one format.
        eprintln!("{alert}");
    }
}

/// A webhook-shaped emitter: it does not perform network I/O (gateway-core has
/// no HTTP client and stays I/O-free), it RENDERS the alert into the exact JSON
/// body a webhook POST would carry and hands it to a caller-supplied delivery
/// closure. A real deployment supplies a closure that POSTs it; the milestone's
/// demo supplies one that appends to a file. This keeps the "originate at the
/// point of enforcement" guarantee while leaving transport to the edge.
pub struct WebhookAlertSink<F: Fn(String) + Send + Sync> {
    deliver: F,
}

impl<F: Fn(String) + Send + Sync> WebhookAlertSink<F> {
    pub fn new(deliver: F) -> WebhookAlertSink<F> {
        WebhookAlertSink { deliver }
    }

    /// The JSON body a webhook POST would carry for this alert.
    pub fn body(alert: &Alert) -> String {
        let kind = match alert.kind {
            AlertKind::SoftThreshold { fraction } => {
                format!("soft_threshold\",\"fraction\":{fraction}")
            }
            AlertKind::HardCap => "hard_cap\",\"fraction\":1.0".to_string(),
        };
        format!(
            "{{\"kind\":\"{kind},\"key\":\"{}\",\"value\":\"{}\",\"cap_tokens\":{},\"spend_tokens\":{},\"node\":\"{}\"}}",
            alert.id.key, alert.id.value, alert.cap, alert.spend, alert.node,
        )
    }
}

impl<F: Fn(String) + Send + Sync> AlertSink for WebhookAlertSink<F> {
    fn emit(&self, alert: &Alert) {
        (self.deliver)(Self::body(alert));
    }
}

/// A per-spender latch so each threshold fires an alert AT MOST ONCE per node
/// per budget (a stream that keeps metering must not re-page every chunk). The
/// enforcement layer calls [`AlertLatch::cross`] with the current spend; it
/// returns the alert to emit the first time each threshold is crossed, then
/// `None` until the counter is reset (a new billing window / a rebalance that
/// grows the share back under threshold).
#[derive(Debug, Default)]
pub struct AlertLatch {
    soft_fired: bool,
    hard_fired: bool,
}

impl AlertLatch {
    pub fn new() -> AlertLatch {
        AlertLatch::default()
    }

    /// Given the current `spend` against `cap` for `id` on `node`, return the
    /// alert(s) newly crossed since the last call. Soft first, then hard; each
    /// fires once. An uncapped budget (`cap` 0 or spend below soft) fires
    /// nothing.
    pub fn cross(&mut self, id: &CapId, cap: u64, spend: u64, node: &str) -> Vec<Alert> {
        let mut out = Vec::new();
        if cap == 0 {
            return out;
        }
        let frac = spend as f64 / cap as f64;
        if !self.soft_fired && frac >= SOFT_ALERT_FRACTION {
            self.soft_fired = true;
            out.push(Alert {
                kind: AlertKind::SoftThreshold {
                    fraction: SOFT_ALERT_FRACTION,
                },
                id: id.clone(),
                cap,
                spend,
                node: node.to_string(),
            });
        }
        if !self.hard_fired && spend >= cap {
            self.hard_fired = true;
            out.push(Alert {
                kind: AlertKind::HardCap,
                id: id.clone(),
                cap,
                spend,
                node: node.to_string(),
            });
        }
        out
    }

    /// Reset the latch (a new billing window, or a rebalance that grew the
    /// share so the spender is back under threshold).
    pub fn reset(&mut self) {
        self.soft_fired = false;
        self.hard_fired = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> CapId {
        CapId::new("team", "ml-research")
    }

    // --- KeyCap composition down the scoped chain ----------------------------

    #[test]
    fn cap_for_uses_override_then_default_then_uncapped() {
        let mut overrides = BTreeMap::new();
        overrides.insert("ml-research".to_string(), Some(200_000));
        overrides.insert("free-tier".to_string(), None); // explicit uncapped
        let cap = KeyCap {
            default: Some(100_000),
            overrides,
        };
        assert_eq!(cap.cap_for("ml-research"), Some(200_000)); // override wins
        assert_eq!(cap.cap_for("anything-else"), Some(100_000)); // default
        assert_eq!(cap.cap_for("free-tier"), None); // explicit uncapped override
    }

    #[test]
    fn no_default_and_no_override_is_uncapped() {
        let cap = KeyCap::default();
        assert_eq!(cap.cap_for("x"), None);
    }

    #[test]
    fn compose_child_lets_the_lower_scope_win() {
        // fleet default 100k; a project lowers the default to 50k and pins a
        // per-value 10k for one team.
        let fleet = KeyCap {
            default: Some(100_000),
            overrides: BTreeMap::new(),
        };
        let mut child_overrides = BTreeMap::new();
        child_overrides.insert("ml-research".to_string(), Some(10_000));
        let project = KeyCap {
            default: Some(50_000),
            overrides: child_overrides,
        };
        let composed = fleet.compose_child(&project);
        assert_eq!(composed.default, Some(50_000)); // lower scope's default wins
        assert_eq!(composed.cap_for("ml-research"), Some(10_000)); // its override
        assert_eq!(composed.cap_for("other"), Some(50_000)); // its default
    }

    #[test]
    fn compose_child_inherits_default_when_child_sets_none() {
        let fleet = KeyCap {
            default: Some(100_000),
            overrides: BTreeMap::new(),
        };
        let child = KeyCap::default(); // sets nothing
        let composed = fleet.compose_child(&child);
        assert_eq!(composed.default, Some(100_000)); // inherited
    }

    // --- LocalBudget: allow / escalate / deny --------------------------------

    #[test]
    fn spend_below_ninety_percent_of_share_allows() {
        // cap 100k, share 100k (single node holds the whole cap), spent 0.
        let b = LocalBudget::new(id(), Some(100_000), 100_000);
        assert_eq!(b.check(1_000), Verdict::Allow); // 1% of share
    }

    #[test]
    fn spend_at_ninety_percent_of_share_escalates() {
        let mut b = LocalBudget::new(id(), Some(100_000), 100_000);
        b.commit(89_000);
        // A 2k spend takes it to 91% of the share -> escalate.
        assert_eq!(b.check(2_000), Verdict::Escalate);
    }

    #[test]
    fn the_escalation_boundary_is_exactly_ninety_percent() {
        let mut b = LocalBudget::new(id(), Some(100_000), 100_000);
        b.commit(80_000);
        // +9999 -> 89_999 (< 90% of 100k) allows; +10_000 -> 90_000 (== 90%)
        // escalates. The boundary is inclusive at 0.90.
        assert_eq!(b.check(9_999), Verdict::Allow);
        assert_eq!(b.check(10_000), Verdict::Escalate);
    }

    #[test]
    fn a_spend_past_the_cap_denies_with_the_cap() {
        let mut b = LocalBudget::new(id(), Some(100_000), 100_000);
        b.commit(99_000);
        assert_eq!(b.check(2_000), Verdict::Deny { cap: 100_000 });
    }

    #[test]
    fn an_uncapped_budget_never_denies_or_escalates() {
        let mut b = LocalBudget::new(id(), None, 0);
        b.commit(1_000_000_000);
        assert_eq!(b.check(1_000_000_000), Verdict::Allow);
        assert_eq!(b.remaining(), u64::MAX);
        assert_eq!(b.overspend(), 0);
    }

    // --- Bounded overspend under partition (the MEASURED number) -------------

    #[test]
    fn under_partition_a_node_spends_only_up_to_its_held_share_then_denies() {
        // A node holds a 40k share of a 100k cap and the control plane is
        // unreachable. It may spend up to 40k and no more — it cannot grow its
        // share without the control plane. The overspend past the share is
        // therefore ZERO for a well-behaved caller.
        let mut b = LocalBudget::new(id(), Some(100_000), 40_000);
        b.commit(38_000);
        assert_eq!(b.check_partitioned(2_000), Verdict::Allow); // fits in 40k
        b.commit(2_000);
        assert_eq!(b.spent(), 40_000);
        // The next spend has no room in the held share -> deny.
        assert_eq!(b.check_partitioned(1), Verdict::Deny { cap: 100_000 });
        assert_eq!(b.overspend(), 0, "spending stopped exactly at the share");
    }

    #[test]
    fn mid_stream_overspend_is_bounded_to_one_streams_tail_and_measurable() {
        // The realistic partition case: a node at 39_500/40_000 admits ONE more
        // stream (it fits the pre-check), the stream runs to 41_200 tokens
        // before the mid-stream cut stops it. The overspend is exactly the tail
        // the running stream produced past the share — bounded by one stream,
        // and reported as a number.
        let mut b = LocalBudget::new(id(), Some(100_000), 40_000);
        b.commit(39_500);
        // The stream is admitted (room for a spend under the share)...
        assert_eq!(b.check_partitioned(100), Verdict::Allow);
        // ...it then meters 1_700 tokens before the cut fires at the share.
        b.commit(1_700);
        assert_eq!(b.spent(), 41_200);
        assert_eq!(b.overspend(), 1_200, "measured overspend past the 40k share");
        // And no NEW stream can start now.
        assert_eq!(b.check_partitioned(1), Verdict::Deny { cap: 100_000 });
    }

    #[test]
    fn set_share_grows_the_ceiling_without_losing_spend() {
        let mut b = LocalBudget::new(id(), Some(100_000), 30_000);
        b.commit(29_000);
        assert_eq!(b.check(2_000), Verdict::Escalate); // near the 30k share
        // The control plane rebalances a bigger slice to this hot node.
        b.set_share(60_000);
        assert_eq!(b.spent(), 29_000, "spend preserved across rebalance");
        assert_eq!(b.check(2_000), Verdict::Allow); // 31k of 60k share -> allow
    }

    // --- Reconciliation to the authoritative usage frame ---------------------

    #[test]
    fn reconcile_adjusts_spend_up_or_down_to_the_authoritative_count() {
        let mut b = LocalBudget::new(id(), Some(100_000), 100_000);
        b.commit(1_000); // live estimate for a stream
        b.reconcile(1_000, 1_200); // provider says it was really 1_200
        assert_eq!(b.spent(), 1_200);
        b.commit(500); // now 1_700
        b.reconcile(500, 400); // an overcount corrected down by 100 -> 1_600
        assert_eq!(b.spent(), 1_600);
    }

    // --- GB-6 alert latch: soft then hard, once each -------------------------

    #[test]
    fn the_latch_fires_soft_at_eighty_percent_then_hard_at_the_cap_once_each() {
        let mut latch = AlertLatch::new();
        let cap = 100_000;
        // Below 80%: nothing.
        assert!(latch.cross(&id(), cap, 50_000, "n1").is_empty());
        // Crossing 80%: one soft alert.
        let soft = latch.cross(&id(), cap, 80_000, "n1");
        assert_eq!(soft.len(), 1);
        assert!(matches!(soft[0].kind, AlertKind::SoftThreshold { .. }));
        // Still above 80% but below cap: no repeat.
        assert!(latch.cross(&id(), cap, 90_000, "n1").is_empty());
        // Hitting the cap: one hard alert.
        let hard = latch.cross(&id(), cap, 100_000, "n1");
        assert_eq!(hard.len(), 1);
        assert_eq!(hard[0].kind, AlertKind::HardCap);
        // Past the cap: no repeat of either.
        assert!(latch.cross(&id(), cap, 120_000, "n1").is_empty());
    }

    #[test]
    fn a_single_cross_can_fire_both_soft_and_hard() {
        // A budget that jumps straight past the cap (a big terminal frame) fires
        // both thresholds in one call, soft first.
        let mut latch = AlertLatch::new();
        let alerts = latch.cross(&id(), 100_000, 100_000, "n1");
        assert_eq!(alerts.len(), 2);
        assert!(matches!(alerts[0].kind, AlertKind::SoftThreshold { .. }));
        assert_eq!(alerts[1].kind, AlertKind::HardCap);
    }

    #[test]
    fn reset_re_arms_the_latch() {
        let mut latch = AlertLatch::new();
        assert_eq!(latch.cross(&id(), 100_000, 100_000, "n1").len(), 2);
        latch.reset();
        // After reset the thresholds can fire again (new window).
        assert_eq!(latch.cross(&id(), 100_000, 80_000, "n1").len(), 1);
    }

    // --- Alert rendering: log line + webhook body ----------------------------

    #[test]
    fn alert_display_carries_value_cap_spend_and_node() {
        let alert = Alert {
            kind: AlertKind::HardCap,
            id: id(),
            cap: 100_000,
            spend: 100_000,
            node: "edge-fra-2".to_string(),
        };
        let line = alert.to_string();
        assert!(line.contains("HARD-CAP"), "{line}");
        assert!(line.contains("team=ml-research"), "{line}");
        assert!(line.contains("100000/100000"), "{line}");
        assert!(line.contains("node=edge-fra-2"), "{line}");
    }

    #[test]
    fn webhook_body_is_json_with_the_alert_fields() {
        let alert = Alert {
            kind: AlertKind::SoftThreshold { fraction: 0.8 },
            id: id(),
            cap: 100_000,
            spend: 80_000,
            node: "n1".to_string(),
        };
        let body = WebhookAlertSink::<fn(String)>::body(&alert);
        assert!(body.contains("\"kind\":\"soft_threshold\""), "{body}");
        assert!(body.contains("\"key\":\"team\""), "{body}");
        assert!(body.contains("\"value\":\"ml-research\""), "{body}");
        assert!(body.contains("\"cap_tokens\":100000"), "{body}");
        assert!(body.contains("\"spend_tokens\":80000"), "{body}");
    }

    #[test]
    fn webhook_sink_hands_the_body_to_the_delivery_closure() {
        use std::sync::{Arc, Mutex};
        let captured = Arc::new(Mutex::new(Vec::new()));
        let c = captured.clone();
        let sink = WebhookAlertSink::new(move |body: String| c.lock().unwrap().push(body));
        sink.emit(&Alert {
            kind: AlertKind::HardCap,
            id: id(),
            cap: 1,
            spend: 1,
            node: "n1".to_string(),
        });
        assert_eq!(captured.lock().unwrap().len(), 1);
        assert!(captured.lock().unwrap()[0].contains("hard_cap"));
    }
}
