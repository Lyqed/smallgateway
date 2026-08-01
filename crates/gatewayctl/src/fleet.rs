//! The fleet's applied state + the all-or-nothing wave rollout (docs/07,
//! "Partial application: all-or-nothing waves, chosen").
//!
//! This module owns the desired render (the current applied repo render), the
//! per-node monotonic version counters, the per-wave committed state, and the
//! adjudication of ONE wave's collected results. The ordered MULTI-wave walk
//! that sequences waves grouped by failure domain lives in [`crate::rollout`];
//! the wave PLAN (selectors, node-to-wave assignment) lives in [`crate::waves`].
//! A single wave is the degenerate one-wave case of that plan and still runs the
//! identical policy here:
//!
//! - Push the new render to every node in the wave.
//! - Wait for every node to `Ack` the exact `render_hash`, within a timeout.
//! - If all ack: advance the fleet's committed version.
//! - If ANY node `Nack`s (or times out): **halt**. The fleet's committed
//!   version does NOT advance, and the divergence is logged loudly and left
//!   surfaced — never silent (docs/07: "on any Nack in the wave, log the
//!   divergence loudly and do not advance the fleet's committed version").
//!
//! Across a multi-wave rollout, [`Fleet::set_wave_commit`] / [`Fleet::wave_commits`]
//! record which commit each wave is on, and [`Fleet::node_pending_in_unapplied_wave`]
//! tells the reconciler a node is legitimately on its prior commit because its
//! (later) wave has not been reached — pending, not drifted.
//!
//! Per-node version: docs/07 says a node's version is monotonic per node,
//! assigned at delivery. The counter here hands each node the next integer each
//! time it is pushed a distinct render, so a reconnecting node's versions never
//! regress.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::gatewayset::GatewaySets;
use crate::render::{render_resolved_for_node, Rendered};
use crate::source::ResolvedRepo;
use crate::waves::WavePlan;

/// The outcome of one wave rollout, for logging and the demo/tests.
#[derive(Debug, PartialEq, Eq)]
pub enum WaveOutcome {
    /// Every node in the wave acked the pushed render_hash. The fleet advanced.
    Committed {
        render_hash: String,
        node_count: usize,
    },
    /// At least one node nacked or went silent. The fleet did NOT advance; the
    /// named divergences are carried for the loud log line.
    Halted {
        render_hash: String,
        divergences: Vec<Divergence>,
    },
    /// There were no connected nodes to push to (the render is applied as the
    /// desired state but no wave ran).
    NoNodes { render_hash: String },
}

/// One node's failure within a wave — the "named divergence" docs/07 requires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence {
    pub node_id: String,
    pub kind: DivergenceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DivergenceKind {
    /// The node rejected the render; it keeps serving its prior version.
    Nacked { version: u64, reason: String },
    /// The node never answered within the wave timeout — unknown, treated as
    /// unreachable (docs/07: "unknown halts").
    Silent { version: u64 },
    /// The node acked, but echoed a render_hash different from what we pushed —
    /// acked the wrong bytes (a bug or tampering).
    WrongHash {
        version: u64,
        expected: String,
        got: String,
    },
}

/// The desired-state + rollout owner. Holds the current applied render and the
/// per-node version counters and committed fleet version. This is desired state
/// derived from Git (the repo render), NOT runtime observed state — it is
/// legitimately in-memory here because it is recomputed from the repo on every
/// apply, never the source of truth.
pub struct Fleet {
    inner: Mutex<FleetInner>,
    /// Count of waves currently in flight (`roll_out`/`roll_out_raw` bodies).
    /// The reconciler reads this to avoid fighting a legitimately mid-rollout
    /// node: while a wave is pushing a new desired render, a node that has not
    /// yet been rolled forward is *mid-rollout*, not drifted, and must be left
    /// to the wave (docs/07: "a node in a not-yet-applied wave's desired hash is
    /// its prior commit's hash"; "the reconciler does not fight a legitimately
    /// mid-rollout node"). A counter (not a bool) so overlapping waves nest
    /// correctly and the guard only clears when the LAST one finishes.
    waves_in_flight: AtomicUsize,
}

/// RAII guard that marks a wave in flight for its lifetime. Constructed at the
/// top of a wave body; dropping it (on return OR panic) decrements the counter,
/// so the mid-rollout window can never leak if a wave errors out.
pub struct WaveGuard<'a> {
    counter: &'a AtomicUsize,
}

impl Drop for WaveGuard<'_> {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

struct FleetInner {
    /// The current applied render (the fleet-wide render). With GatewaySets this
    /// is the base a non-matching node also gets; a node with matching labels
    /// gets a per-node stamped render via [`Fleet::desired_for`].
    applied: Rendered,
    /// The fleet-wide committed version: the highest version every node in the
    /// last successful wave reached. Advances only on a fully-acked wave.
    committed_version: u64,
    /// Per-node monotonic version counter and the last render_hash each node
    /// was pushed. `(next_version, last_pushed_hash)`.
    per_node: BTreeMap<String, NodeVersioning>,
    /// The desired-state derivation inputs. Present when the fleet was built from
    /// a resolved config repo (the serve path); absent in the degenerate
    /// [`Fleet::new`] path used by unit tests, where every node's desired IS the
    /// single applied render. All three are re-derived from the repo on every
    /// apply — desired state, never stored truth (docs/07).
    desired: Option<DesiredInputs>,
    /// Per-wave committed source_commit: which commit each wave is currently on.
    /// A halt freezes the halting wave and all LATER waves on their prior commit
    /// while earlier waves stay advanced, so this is the *named, queryable* mixed
    /// state docs/07 requires ("waves 1 and 2 on abc123, waves 3.. on def456"),
    /// never "some on new some on old, shrug". Keyed by wave name.
    wave_commit: BTreeMap<String, String>,
    /// A small memo of per-node renders keyed by the node's canonical label
    /// string, so a reconcile tick does not re-render for every node every tick
    /// (docs/07: "no config re-render unless the commit changed"). Cleared on
    /// every apply.
    render_cache: BTreeMap<String, Rendered>,
    /// The config-canary policy in force for the applied commit. Analysis OFF by
    /// default; set from `canary.yaml` by the serve path on every apply.
    canary: crate::canary::CanaryPolicy,
}

/// The inputs desired-state is derived from: the resolved config repo, the
/// GatewaySets stamped per node, and the ordered wave plan. Held so a node's
/// desired render can be computed from its labels on demand.
struct DesiredInputs {
    resolved: ResolvedRepo,
    gatewaysets: GatewaySets,
    plan: WavePlan,
}

/// The config-canary policy in force for the CURRENT applied commit (parsed from
/// `canary.yaml`). Held alongside the desired inputs but on its own field so the
/// Phase-2 `from_source` / `set_applied_from_source` signatures stay unchanged;
/// the serve path calls [`Fleet::set_canary_policy`] after re-resolving. Default
/// is analysis OFF (the plain multi-wave walk), so a fleet built without a canary
/// config behaves exactly as it did in Phase 2.

#[derive(Clone)]
struct NodeVersioning {
    last_version: u64,
    last_pushed_hash: Option<String>,
}

impl Fleet {
    /// The degenerate constructor: a fleet with ONE implicit wave over all nodes,
    /// no GatewaySets, and every node's desired equal to the single applied
    /// render. This is the milestone-1/2 behavior — the single-wave case is the
    /// degenerate one-wave case, and its tests keep passing unchanged.
    pub fn new(applied: Rendered) -> Fleet {
        Fleet {
            inner: Mutex::new(FleetInner {
                applied,
                committed_version: 0,
                per_node: BTreeMap::new(),
                desired: None,
                wave_commit: BTreeMap::new(),
                render_cache: BTreeMap::new(),
                canary: crate::canary::CanaryPolicy::default(),
            }),
            waves_in_flight: AtomicUsize::new(0),
        }
    }

    /// The full constructor (the serve path): a fleet whose desired state is
    /// derived from a resolved config repo, its GatewaySets, and its wave plan.
    /// Per-node renders (GatewaySet stamping) and multi-wave rollout are enabled.
    pub fn from_source(
        applied: Rendered,
        resolved: ResolvedRepo,
        gatewaysets: GatewaySets,
        plan: WavePlan,
    ) -> Fleet {
        Fleet {
            inner: Mutex::new(FleetInner {
                applied,
                committed_version: 0,
                per_node: BTreeMap::new(),
                desired: Some(DesiredInputs {
                    resolved,
                    gatewaysets,
                    plan,
                }),
                wave_commit: BTreeMap::new(),
                render_cache: BTreeMap::new(),
                canary: crate::canary::CanaryPolicy::default(),
            }),
            waves_in_flight: AtomicUsize::new(0),
        }
    }

    /// Mark a wave as in flight for the returned guard's lifetime. The server's
    /// wave body holds this guard while it pushes and awaits acks; the
    /// reconciler consults [`Fleet::wave_in_flight`] to skip healing nodes that
    /// are legitimately mid-rollout (docs/07).
    pub fn begin_wave(&self) -> WaveGuard<'_> {
        self.waves_in_flight.fetch_add(1, Ordering::SeqCst);
        WaveGuard {
            counter: &self.waves_in_flight,
        }
    }

    /// Whether at least one wave is currently rolling out. While true, the
    /// reconciler must not fight a node that has not yet been rolled forward —
    /// it is mid-rollout, not drifted.
    pub fn wave_in_flight(&self) -> bool {
        self.waves_in_flight.load(Ordering::SeqCst) > 0
    }

    /// The currently-applied render (cloned for use off-lock).
    pub fn applied(&self) -> Rendered {
        self.lock().applied.clone()
    }

    /// The fleet-wide committed version.
    pub fn committed_version(&self) -> u64 {
        self.lock().committed_version
    }

    /// Replace the applied render (a local reload changed the repo). Returns
    /// true if the render actually changed (different hash) — an identical
    /// re-render is a no-op, mirroring the node's own hash short-circuit. Clears
    /// the per-node render cache so per-node desired is recomputed against the
    /// new commit.
    pub fn set_applied(&self, next: Rendered) -> bool {
        let mut inner = self.lock();
        if inner.applied.render_hash == next.render_hash {
            return false;
        }
        inner.applied = next;
        inner.render_cache.clear();
        true
    }

    /// Replace the applied render AND its desired-derivation inputs (a reload
    /// re-resolved the repo). Used by the serve path so a config change also
    /// refreshes the GatewaySets and wave plan. Returns true if the RESOLVED
    /// config changed, so the caller runs the ordered multi-wave rollout.
    ///
    /// The change signal is `source_commit` (a content id over ALL repo files),
    /// NOT the fleet-wide `render_hash`. The fleet-wide render_hash is the hash of
    /// the UNSTAMPED base render, so a change confined to a GatewaySet overlay (or
    /// to `waves.yaml`) perturbs some node's per-node stamped render — and the
    /// wave plan — without moving the base hash. Gating on render_hash alone would
    /// silently skip the ordered rollout for a GatewaySet-only edit and leave
    /// propagation to the unordered per-node reconciler, bypassing the ordered-
    /// wave substrate. Gating on `source_commit` rolls the wave plan for ANY
    /// resolved-config change (base fragment, overlay, or wave plan) while still
    /// treating a byte-identical re-resolve (same content id) as a no-op, mirroring
    /// the node's own hash short-circuit.
    pub fn set_applied_from_source(
        &self,
        next: Rendered,
        resolved: ResolvedRepo,
        gatewaysets: GatewaySets,
        plan: WavePlan,
    ) -> bool {
        let mut inner = self.lock();
        let changed = inner.applied.source_commit != next.source_commit;
        inner.applied = next;
        inner.desired = Some(DesiredInputs {
            resolved,
            gatewaysets,
            plan,
        });
        inner.render_cache.clear();
        changed
    }

    /// The wave plan currently in force (cloned). The degenerate single plan
    /// when the fleet was built without a source.
    pub fn wave_plan(&self) -> WavePlan {
        self.lock()
            .desired
            .as_ref()
            .map(|d| d.plan.clone())
            .unwrap_or_else(WavePlan::single)
    }

    /// Set the config-canary policy in force (parsed from `canary.yaml` on the
    /// current apply). The serve path calls this after every re-resolve so the
    /// rollout sees the reviewed policy; the default (analysis OFF) holds until
    /// then, preserving the Phase-2 plain multi-wave walk.
    pub fn set_canary_policy(&self, policy: crate::canary::CanaryPolicy) {
        self.lock().canary = policy;
    }

    /// The config-canary policy currently in force (cloned). Default (analysis
    /// OFF) when none was set.
    pub fn canary_policy(&self) -> crate::canary::CanaryPolicy {
        self.lock().canary.clone()
    }

    /// The desired render FOR ONE NODE, given its labels: the per-node GatewaySet-
    /// stamped render when the fleet has a source, else the single applied render.
    /// Memoized by the node's canonical label string so a reconcile tick does not
    /// re-render per node every tick. This is desired state derived from the
    /// repo, never stored truth (docs/07). Falls back to the applied render if a
    /// per-node render fails (it validated fleet-wide at apply, so this is
    /// defensive; the fleet-wide render is a safe floor).
    pub fn desired_for(&self, labels: &BTreeMap<String, String>) -> Rendered {
        let key = canonical_labels(labels);
        let mut inner = self.lock();
        if let Some(hit) = inner.render_cache.get(&key) {
            return hit.clone();
        }
        let rendered = match &inner.desired {
            None => inner.applied.clone(),
            Some(d) => render_resolved_for_node(&d.resolved, &d.gatewaysets, labels)
                .unwrap_or_else(|_| inner.applied.clone()),
        };
        inner.render_cache.insert(key, rendered.clone());
        rendered
    }

    /// Record that `wave_name` is now committed on `source_commit` (a fully-acked
    /// wave advanced). Surfaced as the per-wave committed state.
    pub fn set_wave_commit(&self, wave_name: &str, source_commit: &str) {
        self.lock()
            .wave_commit
            .insert(wave_name.to_string(), source_commit.to_string());
    }

    /// Revert `wave_name`'s committed state to `prior_commit` (an auto-rollback
    /// on a failed config-canary). Distinct from [`Fleet::set_wave_commit`] only
    /// in intent — it records the wave BACK on an earlier version — but named so
    /// the rollback path reads clearly. When `prior_commit` is the never-committed
    /// sentinel the entry is removed, so the wave surfaces as having no committed
    /// version rather than pointing at a placeholder.
    pub fn revert_wave_commit(&self, wave_name: &str, prior_commit: &str) {
        let mut inner = self.lock();
        if prior_commit == crate::rollout::NEVER_COMMITTED {
            inner.wave_commit.remove(wave_name);
        } else {
            inner
                .wave_commit
                .insert(wave_name.to_string(), prior_commit.to_string());
        }
    }

    /// The per-wave committed source_commit map, sorted by wave name — the
    /// queryable, alertable mixed-state view (docs/07: "waves 1 and 2 on abc123,
    /// waves 3.. on def456", never "shrug").
    pub fn wave_commits(&self) -> BTreeMap<String, String> {
        self.lock().wave_commit.clone()
    }

    /// Whether a node with `labels` is legitimately PENDING in a wave that has
    /// NOT yet advanced to the current target commit — a not-yet-started later
    /// wave, or a wave frozen behind a halt. Such a node is on its prior version
    /// on purpose (docs/07: "a node in a not-yet-applied wave's desired hash is
    /// its prior commit's hash, not the newest"), so the reconciler must NOT heal
    /// it toward the new render — it is pending, not drifted. The wave rollout
    /// (or a resumed rollout after the halt is fixed) is what advances it.
    ///
    /// Only meaningful when a MULTI-wave plan is in force AND a genuine partial-
    /// application state exists (some wave has reached the target commit while
    /// the node's wave has not). It returns true then — the node is pending in a
    /// not-yet-advanced or halt-frozen later wave, and the reconciler must leave
    /// it. It returns FALSE for:
    ///   - a no-source / degenerate fleet (unit tests): never pending;
    ///   - a single everything-wave plan (the default serve path, no waves.yaml):
    ///     there are no later waves, so the reconciler heals normally, exactly as
    ///     milestone-2 — no behavior change for the 1-wave case;
    ///   - the startup state before any wave has committed (no wave is on target
    ///     yet, so this is NOT a partial application): the reconciler converges
    ///     nodes normally via heal, as before.
    ///
    /// The wave-in-flight guard (checked separately) covers the DURING-rollout
    /// window; this method covers the AFTER-a-halt steady state where later waves
    /// stay legitimately frozen on the prior commit.
    pub fn node_pending_in_unapplied_wave(&self, labels: &BTreeMap<String, String>) -> bool {
        let inner = self.lock();
        let Some(d) = &inner.desired else {
            return false;
        };
        // A single everything-wave has no "later wave" to be pending in.
        if d.plan.waves.len() <= 1 {
            return false;
        }
        let target_commit = &inner.applied.source_commit;
        // A partial application exists only if SOME wave already reached target.
        let any_on_target = inner.wave_commit.values().any(|c| c == target_commit);
        if !any_on_target {
            return false; // no wave is on target yet => not a partial application
        }
        let idx = d.plan.wave_index_for(labels);
        let wave_name = d
            .plan
            .waves
            .get(idx)
            .map(|w| w.name.clone())
            .unwrap_or_else(|| crate::waves::IMPLICIT_FINAL_WAVE.to_string());
        // Pending iff THIS node's wave has not reached target while others have.
        inner.wave_commit.get(&wave_name).map(|c| c.as_str()) != Some(target_commit.as_str())
    }

    /// The `render_hash` the control plane last PUSHED to `node_id` — the
    /// "delivered" hash of the drift truth table (docs/07). `None` if the node
    /// was never pushed. This is a record of what was delivered, not desired
    /// state: desired is always recomputed from the applied render.
    pub fn delivered_hash(&self, node_id: &str) -> Option<String> {
        self.lock()
            .per_node
            .get(node_id)
            .and_then(|v| v.last_pushed_hash.clone())
    }

    /// Assign the next per-node version for a push of `hash` to `node_id`. If
    /// the node was already pushed this exact hash, its version is unchanged
    /// (an idempotent re-push is a no-op number-wise, matching the node's
    /// no-op-is-an-ack semantics). Monotonic per node.
    pub fn next_version_for(&self, node_id: &str, hash: &str) -> u64 {
        let mut inner = self.lock();
        let entry = inner
            .per_node
            .entry(node_id.to_string())
            .or_insert(NodeVersioning {
                last_version: 0,
                last_pushed_hash: None,
            });
        if entry.last_pushed_hash.as_deref() == Some(hash) {
            return entry.last_version;
        }
        entry.last_version += 1;
        entry.last_pushed_hash = Some(hash.to_string());
        entry.last_version
    }

    /// Adjudicate one wave's collected results against a SINGLE expected hash
    /// (every node in the wave was pushed the same render) into a `WaveOutcome`,
    /// advancing the committed version on a clean sweep. The degenerate single-
    /// wave and the raw/tampered paths use this; it delegates to the per-node
    /// adjudicator with the one hash expected of every node.
    ///
    /// `results` is `(node_id, AckResult)` for every node the wave pushed to.
    pub fn conclude_wave(
        &self,
        render_hash: &str,
        version: u64,
        results: &[(String, AckResult)],
    ) -> WaveOutcome {
        let expected: Vec<(String, String)> = results
            .iter()
            .map(|(id, _)| (id.clone(), render_hash.to_string()))
            .collect();
        self.conclude_wave_multi(render_hash, &expected, version, results)
    }

    /// Adjudicate a wave where each node has its OWN expected render hash (the
    /// GatewaySet-per-node case). A node acks correctly when its ack hash equals
    /// ITS expected hash; a wrong hash, NACK, or silence is a divergence and the
    /// wave halts. On a clean sweep the committed version advances to `version`.
    /// `outcome_hash` labels the outcome (a representative hash for the wave —
    /// the wave's fleet-wide render_hash, or the first node's when per-node).
    pub fn conclude_wave_multi(
        &self,
        outcome_hash: &str,
        expected: &[(String, String)],
        version: u64,
        results: &[(String, AckResult)],
    ) -> WaveOutcome {
        if results.is_empty() {
            return WaveOutcome::NoNodes {
                render_hash: outcome_hash.to_string(),
            };
        }
        let expected_of = |node_id: &str| -> &str {
            expected
                .iter()
                .find(|(id, _)| id == node_id)
                .map(|(_, h)| h.as_str())
                .unwrap_or(outcome_hash)
        };

        let mut divergences = Vec::new();
        for (node_id, result) in results {
            let want = expected_of(node_id);
            match result {
                AckResult::Acked { hash } if hash == want => {}
                AckResult::Acked { hash } => divergences.push(Divergence {
                    node_id: node_id.clone(),
                    kind: DivergenceKind::WrongHash {
                        version,
                        expected: want.to_string(),
                        got: hash.clone(),
                    },
                }),
                AckResult::Nacked { reason } => divergences.push(Divergence {
                    node_id: node_id.clone(),
                    kind: DivergenceKind::Nacked {
                        version,
                        reason: reason.clone(),
                    },
                }),
                AckResult::Silent => divergences.push(Divergence {
                    node_id: node_id.clone(),
                    kind: DivergenceKind::Silent { version },
                }),
            }
        }

        if divergences.is_empty() {
            self.lock().committed_version = version;
            WaveOutcome::Committed {
                render_hash: outcome_hash.to_string(),
                node_count: results.len(),
            }
        } else {
            WaveOutcome::Halted {
                render_hash: outcome_hash.to_string(),
                divergences,
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FleetInner> {
        self.inner.lock().expect("fleet lock")
    }
}

/// A canonical string for a node's labels — sorted `k=v;` pairs — used as the
/// per-node render-cache key. `BTreeMap` iteration is already sorted, so this is
/// deterministic and cache-stable for identical label sets.
fn canonical_labels(labels: &BTreeMap<String, String>) -> String {
    let mut s = String::new();
    for (k, v) in labels {
        s.push_str(k);
        s.push('=');
        s.push_str(v);
        s.push(';');
    }
    s
}

/// One node's response to a wave push, as collected by the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AckResult {
    Acked { hash: String },
    Nacked { reason: String },
    /// No answer within the wave timeout.
    Silent,
}

// The per-wave rollout OUTCOME types (`WaveStep`, `WaveStepState`,
// `MultiWaveOutcome`) live in [`crate::rollout`] alongside the walk that
// produces them — they are rollout products, not fleet state. `Fleet` here owns
// the applied render, per-node versions, and the per-wave COMMITTED map that the
// walk records into.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::testrepo;
    use crate::render::render_repo;

    fn rendered(env: &str) -> Rendered {
        render_repo(&testrepo::write(env)).unwrap()
    }

    #[test]
    fn a_fully_acked_wave_commits_and_advances_the_version() {
        let fleet = Fleet::new(rendered("prod"));
        let hash = fleet.applied().render_hash;
        let results = vec![
            ("n1".to_string(), AckResult::Acked { hash: hash.clone() }),
            ("n2".to_string(), AckResult::Acked { hash: hash.clone() }),
        ];
        let outcome = fleet.conclude_wave(&hash, 1, &results);
        assert_eq!(
            outcome,
            WaveOutcome::Committed {
                render_hash: hash.clone(),
                node_count: 2
            }
        );
        assert_eq!(fleet.committed_version(), 1);
    }

    #[test]
    fn a_single_nack_halts_the_wave_and_the_version_does_not_advance() {
        let fleet = Fleet::new(rendered("prod"));
        let hash = fleet.applied().render_hash;
        let results = vec![
            ("n1".to_string(), AckResult::Acked { hash: hash.clone() }),
            (
                "n2".to_string(),
                AckResult::Nacked {
                    reason: "unknown provider foo".to_string(),
                },
            ),
        ];
        let outcome = fleet.conclude_wave(&hash, 5, &results);
        match outcome {
            WaveOutcome::Halted { divergences, .. } => {
                assert_eq!(divergences.len(), 1);
                assert_eq!(divergences[0].node_id, "n2");
                assert!(matches!(
                    divergences[0].kind,
                    DivergenceKind::Nacked { version: 5, .. }
                ));
            }
            other => panic!("expected Halted, got {other:?}"),
        }
        assert_eq!(fleet.committed_version(), 0, "committed version frozen");
    }

    #[test]
    fn a_silent_node_halts_the_wave() {
        let fleet = Fleet::new(rendered("prod"));
        let hash = fleet.applied().render_hash;
        let results = vec![
            ("n1".to_string(), AckResult::Acked { hash: hash.clone() }),
            ("n2".to_string(), AckResult::Silent),
        ];
        let outcome = fleet.conclude_wave(&hash, 2, &results);
        assert!(matches!(outcome, WaveOutcome::Halted { .. }));
        assert_eq!(fleet.committed_version(), 0);
    }

    #[test]
    fn an_ack_of_the_wrong_hash_is_a_divergence() {
        let fleet = Fleet::new(rendered("prod"));
        let hash = fleet.applied().render_hash;
        let results = vec![(
            "n1".to_string(),
            AckResult::Acked {
                hash: "tampered".to_string(),
            },
        )];
        let outcome = fleet.conclude_wave(&hash, 1, &results);
        match outcome {
            WaveOutcome::Halted { divergences, .. } => assert!(matches!(
                divergences[0].kind,
                DivergenceKind::WrongHash { .. }
            )),
            other => panic!("expected Halted, got {other:?}"),
        }
    }

    #[test]
    fn per_node_versions_are_monotonic_and_idempotent_on_same_hash() {
        let fleet = Fleet::new(rendered("prod"));
        let v1 = fleet.next_version_for("n1", "hashA");
        let v1_again = fleet.next_version_for("n1", "hashA");
        let v2 = fleet.next_version_for("n1", "hashB");
        assert_eq!(v1, 1);
        assert_eq!(v1_again, 1, "same hash re-push keeps the version");
        assert_eq!(v2, 2, "a new hash advances");
        // A different node has its own counter.
        assert_eq!(fleet.next_version_for("n2", "hashB"), 1);
    }

    #[test]
    fn set_applied_no_ops_on_identical_hash() {
        let fleet = Fleet::new(rendered("prod"));
        let same = fleet.applied();
        assert!(!fleet.set_applied(same), "identical render is a no-op");
        assert!(fleet.set_applied(rendered("canary")), "a real change applies");
    }

    #[test]
    fn wave_in_flight_guard_nests_and_clears_on_drop() {
        let fleet = Fleet::new(rendered("prod"));
        assert!(!fleet.wave_in_flight(), "no wave initially");
        {
            let _outer = fleet.begin_wave();
            assert!(fleet.wave_in_flight(), "outer wave marks in-flight");
            {
                let _inner = fleet.begin_wave();
                assert!(fleet.wave_in_flight(), "nested wave stays in-flight");
            }
            assert!(
                fleet.wave_in_flight(),
                "inner drop must not clear while outer still holds"
            );
        }
        assert!(!fleet.wave_in_flight(), "last drop clears the guard");
    }

    #[test]
    fn a_wave_with_no_nodes_is_no_nodes_not_a_false_commit() {
        let fleet = Fleet::new(rendered("prod"));
        let hash = fleet.applied().render_hash;
        assert_eq!(
            fleet.conclude_wave(&hash, 1, &[]),
            WaveOutcome::NoNodes { render_hash: hash }
        );
        assert_eq!(fleet.committed_version(), 0);
    }

    // --- The reconciler's pending-later-wave guard ---------------------------

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// A `from_source` fleet with an eu -> us two-wave plan, applied at `env`.
    fn multiwave_fleet(env: &str) -> Fleet {
        use crate::source::{ConfigSource, DirectorySource};
        use crate::waves::{Selector, SelectorTerm, UnmatchedPolicy, Wave, WavePlan};
        let root = testrepo::write(env);
        let resolved = DirectorySource::new(&root).resolve().unwrap();
        let applied = render_repo(&root).unwrap();
        let plan = WavePlan::new(
            vec![
                Wave {
                    name: "eu".to_string(),
                    selector: Selector::of(vec![SelectorTerm::eq("region", "eu")]),
                },
                Wave {
                    name: "us".to_string(),
                    selector: Selector::of(vec![SelectorTerm::eq("region", "us")]),
                },
            ],
            UnmatchedPolicy::ImplicitFinalWave,
        );
        Fleet::from_source(applied, resolved, GatewaySets::default(), plan)
    }

    #[test]
    fn a_node_in_a_frozen_later_wave_is_pending_not_drifted() {
        // Partial application: the eu wave advanced to the applied commit; the us
        // wave has NOT. A us node is then legitimately on its prior version and
        // must read as PENDING (the reconciler leaves it), while an eu node —
        // whose wave already reached target — is NOT pending.
        let fleet = multiwave_fleet("prod");
        let target = fleet.applied().source_commit;
        fleet.set_wave_commit("eu", &target); // eu reached target; us did not.

        assert!(
            fleet.node_pending_in_unapplied_wave(&labels(&[("region", "us")])),
            "the us node's wave has not reached target -> pending"
        );
        assert!(
            !fleet.node_pending_in_unapplied_wave(&labels(&[("region", "eu")])),
            "the eu node's wave reached target -> not pending"
        );
    }

    #[test]
    fn no_node_is_pending_before_any_wave_reaches_target() {
        // Startup / pre-rollout: no wave is on target yet, so this is NOT a
        // partial application and every node converges normally via heal.
        let fleet = multiwave_fleet("prod");
        assert!(!fleet.node_pending_in_unapplied_wave(&labels(&[("region", "us")])));
        assert!(!fleet.node_pending_in_unapplied_wave(&labels(&[("region", "eu")])));
    }

    #[test]
    fn a_degenerate_single_wave_fleet_never_reports_pending() {
        // The no-source / 1-wave case: there are no later waves, so the
        // reconciler heals normally exactly as milestone-2 — no behavior change.
        let fleet = Fleet::new(rendered("prod"));
        assert!(!fleet.node_pending_in_unapplied_wave(&labels(&[("region", "us")])));
    }
}
