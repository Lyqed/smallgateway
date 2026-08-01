//! The fleet's applied state + the all-or-nothing wave rollout (docs/07,
//! "Partial application: all-or-nothing waves, chosen").
//!
//! This module owns the desired render (the current applied repo render) and
//! the per-node monotonic version counters, and it drives one rollout wave. M1
//! implements a SINGLE wave over all connected nodes; the multi-wave, grouped-
//! by-failure-domain sequencing is deferred (noted in the README). The wave
//! policy that IS implemented is the load-bearing half:
//!
//! - Push the new render to every node in the wave.
//! - Wait for every node to `Ack` the exact `render_hash`, within a timeout.
//! - If all ack: advance the fleet's committed version.
//! - If ANY node `Nack`s (or times out): **halt**. The fleet's committed
//!   version does NOT advance, and the divergence is logged loudly and left
//!   surfaced — never silent (docs/07: "on any Nack in the wave, log the
//!   divergence loudly and do not advance the fleet's committed version").
//!
//! Per-node version: docs/07 says a node's version is monotonic per node,
//! assigned at delivery. The counter here hands each node the next integer each
//! time it is pushed a distinct render, so a reconnecting node's versions never
//! regress.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::render::Rendered;

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
    /// The current applied render (the desired config for the whole fleet in
    /// M1, since selectors are a Phase 5 stub).
    applied: Rendered,
    /// The fleet-wide committed version: the highest version every node in the
    /// last successful wave reached. Advances only on a fully-acked wave.
    committed_version: u64,
    /// Per-node monotonic version counter and the last render_hash each node
    /// was pushed. `(next_version, last_pushed_hash)`.
    per_node: BTreeMap<String, NodeVersioning>,
}

#[derive(Clone)]
struct NodeVersioning {
    last_version: u64,
    last_pushed_hash: Option<String>,
}

impl Fleet {
    pub fn new(applied: Rendered) -> Fleet {
        Fleet {
            inner: Mutex::new(FleetInner {
                applied,
                committed_version: 0,
                per_node: BTreeMap::new(),
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
    /// re-render is a no-op, mirroring the node's own hash short-circuit.
    pub fn set_applied(&self, next: Rendered) -> bool {
        let mut inner = self.lock();
        if inner.applied.render_hash == next.render_hash {
            return false;
        }
        inner.applied = next;
        true
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

    /// Adjudicate one wave's collected results into a `WaveOutcome` and, on a
    /// clean sweep, advance the committed version to `version`.
    ///
    /// `results` is `(node_id, AckResult)` for every node the wave pushed to.
    pub fn conclude_wave(
        &self,
        render_hash: &str,
        version: u64,
        results: &[(String, AckResult)],
    ) -> WaveOutcome {
        if results.is_empty() {
            return WaveOutcome::NoNodes {
                render_hash: render_hash.to_string(),
            };
        }

        let mut divergences = Vec::new();
        for (node_id, result) in results {
            match result {
                AckResult::Acked { hash } if hash == render_hash => {}
                AckResult::Acked { hash } => divergences.push(Divergence {
                    node_id: node_id.clone(),
                    kind: DivergenceKind::WrongHash {
                        version,
                        expected: render_hash.to_string(),
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
                render_hash: render_hash.to_string(),
                node_count: results.len(),
            }
        } else {
            WaveOutcome::Halted {
                render_hash: render_hash.to_string(),
                divergences,
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FleetInner> {
        self.inner.lock().expect("fleet lock")
    }
}

/// One node's response to a wave push, as collected by the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AckResult {
    Acked { hash: String },
    Nacked { reason: String },
    /// No answer within the wave timeout.
    Silent,
}

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
}
