//! Wave rollout orchestration for the control plane (docs/07-control-plane.md,
//! "Partial application: all-or-nothing waves, chosen").
//!
//! This module carries the rollout half of [`ControlPlane`]: the single-wave
//! (degenerate) path, the ordered MULTI-wave walk that groups nodes by failure
//! domain from their labels, per-node self-heal, and the raw/tampered break-glass
//! paths. It is split out of `server.rs` (which keeps the gRPC `FleetService`
//! stream handling, session table, and push/ack correlation) purely to keep each
//! file focused and under the size budget; the methods here are a second
//! `impl ControlPlane` block and share the session/push machinery via the
//! `pub(crate)` helpers on `ControlPlane` and `Sessions`.
//!
//! The wave walk (docs/07): assign every connected node to the FIRST wave whose
//! selector its labels match; wave by wave, push each node its own per-node
//! desired render (GatewaySet-stamped), wait for every node in the wave to ack
//! the exact render_hash within the wave timeout, and only then proceed. On any
//! Nack/timeout the wave HALTS: it and all LATER waves stay on their prior commit
//! (frozen) while earlier acked waves stay advanced. The per-wave committed state
//! is recorded and surfaced, never "some on new, some on old, shrug". One
//! wave-in-flight guard is held for the whole rollout so the reconciler defers to
//! it across every wave.

use log::{error, info};

use gateway_proto::RenderedSnapshot;

use crate::fleet::{AckResult, Divergence, DivergenceKind, WaveOutcome};
use crate::server::{now_unix, short, ControlPlane, PendingHandle, WAVE_ACK_TIMEOUT};

/// Sentinel `commit` for a frozen wave that has never committed a version — a
/// first-ever rollout that halts before this wave was ever attempted. It names
/// the genuine state (no prior committed version) instead of a placeholder that
/// reads like a real commit hash. Replaced by the concrete prior commit on any
/// later rollout, once the wave_commit map is populated.
pub(crate) const NEVER_COMMITTED: &str = "(never committed)";

/// The outcome of one wave WITHIN a multi-wave rollout, for the surfaced,
/// queryable per-wave committed state (docs/07: "waves 1 and 2 on abc123, waves
/// 3.. on def456", never "shrug").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaveStep {
    pub wave_name: String,
    /// The nodes this wave rolled to (empty for a wave no connected node matched).
    pub node_count: usize,
    /// The committed state of this wave AFTER the rollout attempt: the
    /// source_commit it is now on, and whether it advanced, was frozen (a later
    /// wave than the halt), or halted here.
    pub state: WaveStepState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaveStepState {
    /// Every node in the wave acked; the wave advanced to `commit`.
    Advanced { commit: String },
    /// This wave HALTED: a node diverged; the wave stays on its prior `commit`.
    Halted {
        commit: String,
        divergences: Vec<Divergence>,
    },
    /// A LATER wave than the halt: never attempted, frozen on its prior `commit`.
    Frozen { commit: String },
    /// No connected node matched this wave's selector; nothing to roll.
    Empty,
}

/// The whole multi-wave rollout's outcome: the ordered per-wave steps plus the
/// index of the halting wave, if any. Surfaced so an operator reads exactly
/// which waves are on which commit and where the rollout stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiWaveOutcome {
    pub steps: Vec<WaveStep>,
    /// `Some(index)` of the first halted wave; `None` when every wave advanced.
    pub halted_at: Option<usize>,
    /// The commit the rollout was attempting to apply.
    pub target_commit: String,
}

impl MultiWaveOutcome {
    /// Whether the rollout fully applied (no wave halted).
    pub fn is_fully_applied(&self) -> bool {
        self.halted_at.is_none()
    }

    /// A one-line surfaced summary of the mixed per-wave committed state, e.g.
    /// "waves [canary, eu] on abc123, waves [us] on def456 (halted at us)".
    pub fn summary(&self) -> String {
        let mut advanced: Vec<&str> = Vec::new();
        let mut frozen: Vec<&str> = Vec::new();
        let mut halted: Vec<&str> = Vec::new();
        for step in &self.steps {
            match &step.state {
                WaveStepState::Advanced { .. } => advanced.push(&step.wave_name),
                WaveStepState::Frozen { .. } => frozen.push(&step.wave_name),
                WaveStepState::Halted { .. } => halted.push(&step.wave_name),
                WaveStepState::Empty => {}
            }
        }
        if self.halted_at.is_none() {
            format!("all waves {:?} on {}", advanced, short(&self.target_commit))
        } else {
            format!(
                "waves {:?} advanced to {}; halted at {:?}; later waves {:?} frozen on prior commit",
                advanced,
                short(&self.target_commit),
                halted,
                frozen,
            )
        }
    }
}

impl ControlPlane {
    /// Run one all-or-nothing wave for the currently-applied render across every
    /// connected node — the single-wave (degenerate) path. Pushes the fleet-wide
    /// applied render (no per-node stamping) to every connected node at once. The
    /// raw/tampered break-glass paths and the single-plan reload use this; it is
    /// the one-wave case that [`ControlPlane::roll_out_plan`] generalizes.
    pub async fn roll_out(&self, trigger: &str) -> WaveOutcome {
        let rendered = self.fleet.applied();
        self.run_wave(trigger, &rendered.render_hash, &rendered.source_commit, &rendered.config_bytes)
            .await
    }

    /// Walk the fleet's WAVE PLAN in order (docs/07 "Partial application"):
    /// assign every connected node to the first wave whose selector its labels
    /// match; then, wave by wave, push each node its OWN per-node desired render
    /// (GatewaySet-stamped), wait for every node in the wave to ack the exact
    /// per-node render_hash within the wave timeout, and only then proceed to the
    /// next wave. On any Nack/timeout the wave HALTS: it and ALL LATER waves stay
    /// on their prior committed commit (frozen), while earlier waves that already
    /// acked stay advanced. The per-wave committed state is recorded and returned,
    /// surfaced, never "some on new some on old, shrug".
    ///
    /// The whole rollout holds ONE wave-in-flight guard for its entire duration,
    /// so the reconciler defers to it across every wave — a node in a not-yet-
    /// started later wave is legitimately pending, not drifted, and is not healed
    /// toward the new render mid-rollout.
    pub async fn roll_out_plan(&self, trigger: &str) -> MultiWaveOutcome {
        let _wave_guard = self.fleet.begin_wave();
        let plan = self.fleet.wave_plan();
        let target = self.fleet.applied();
        let target_commit = target.source_commit.clone();

        // Assign connected nodes to waves by their stored labels.
        let node_labels = self.connected_node_labels().await;
        let label_refs: Vec<(&str, &std::collections::BTreeMap<String, String>)> = node_labels
            .iter()
            .map(|(id, l)| (id.as_str(), l))
            .collect();
        let assigned = plan.assign(label_refs.iter().copied());

        info!(
            "[rollout] trigger={trigger} multi-wave rollout of commit={} over {} wave(s)",
            short(&target_commit),
            assigned.len(),
        );

        let mut steps: Vec<WaveStep> = Vec::new();
        let mut halted_at: Option<usize> = None;

        for (idx, wave) in assigned.iter().enumerate() {
            // Once any earlier wave halted, every later wave is FROZEN on its
            // prior commit — never attempted (docs/07: "the halting wave and all
            // later waves stay on their prior version").
            if halted_at.is_some() {
                steps.push(WaveStep {
                    wave_name: wave.name.clone(),
                    node_count: wave.node_ids.len(),
                    state: WaveStepState::Frozen {
                        commit: self.frozen_commit(&wave.name),
                    },
                });
                info!(
                    "[rollout]   wave {:?}: FROZEN on prior commit (a earlier wave halted)",
                    wave.name
                );
                continue;
            }

            if wave.node_ids.is_empty() {
                steps.push(WaveStep {
                    wave_name: wave.name.clone(),
                    node_count: 0,
                    state: WaveStepState::Empty,
                });
                continue;
            }

            let outcome = self.run_wave_nodes(trigger, &wave.name, &wave.node_ids).await;
            match outcome {
                WaveOutcome::Committed { node_count, .. } => {
                    self.fleet.set_wave_commit(&wave.name, &target_commit);
                    info!(
                        "[rollout]   wave {:?}: COMMITTED across {node_count} node(s) -> commit {}",
                        wave.name,
                        short(&target_commit),
                    );
                    steps.push(WaveStep {
                        wave_name: wave.name.clone(),
                        node_count,
                        state: WaveStepState::Advanced {
                            commit: target_commit.clone(),
                        },
                    });
                }
                WaveOutcome::Halted { divergences, .. } => {
                    halted_at = Some(idx);
                    let prior = self.frozen_commit(&wave.name);
                    error!(
                        "[rollout]   wave {:?}: HALTED — {} divergence(s); this wave and all \
                         LATER waves stay on their prior commit ({}), earlier waves stay advanced",
                        wave.name,
                        divergences.len(),
                        short(&prior),
                    );
                    for d in &divergences {
                        error!("[rollout]     divergent node {}", describe(d));
                    }
                    steps.push(WaveStep {
                        wave_name: wave.name.clone(),
                        node_count: wave.node_ids.len(),
                        state: WaveStepState::Halted {
                            commit: prior,
                            divergences,
                        },
                    });
                }
                WaveOutcome::NoNodes { .. } => {
                    steps.push(WaveStep {
                        wave_name: wave.name.clone(),
                        node_count: 0,
                        state: WaveStepState::Empty,
                    });
                }
            }
        }

        let result = MultiWaveOutcome {
            steps,
            halted_at,
            target_commit,
        };
        if result.is_fully_applied() {
            info!("[rollout] trigger={trigger} FULLY APPLIED: {}", result.summary());
        } else {
            error!(
                "[rollout] trigger={trigger} PARTIALLY APPLIED (surfaced, never silent): {}",
                result.summary()
            );
        }
        result
    }

    /// The concrete commit a frozen wave is currently on: its last recorded
    /// per-wave commit. When the wave has NO per-wave record yet — a first-ever
    /// rollout that halts before this wave has ever committed — there is no prior
    /// commit to name (the wave's nodes are on no committed version at all), so
    /// this returns the explicit `NEVER_COMMITTED` sentinel rather than a
    /// placeholder that reads like a real commit. The per-wave record is not lost:
    /// the WaveStep still exists, `halted_at` is set, `summary()` names the frozen
    /// wave, and each node's true running version stays queryable via its
    /// delivered/observed hash. On any subsequent rollout the wave_commit map is
    /// populated, so this returns the concrete prior commit (docs/07:
    /// "waves 3.. on def456").
    fn frozen_commit(&self, wave_name: &str) -> String {
        self.fleet
            .wave_commits()
            .get(wave_name)
            .cloned()
            .unwrap_or_else(|| NEVER_COMMITTED.to_string())
    }

    /// Self-heal one drifted node: re-push its OWN per-node desired render to just
    /// that node and await its ack (docs/07: "Self-heal is re-push"). Unlike a
    /// wave, this touches ONE node and never advances the fleet's committed
    /// version — it converges a node that fell behind desired, it does not roll
    /// out a new desired. Returns `true` if the node acked its desired
    /// `render_hash` within the timeout, `false` otherwise (a NACK or silence,
    /// which the reconciler surfaces and retries next tick).
    pub async fn heal_node(&self, node_id: &str) -> bool {
        let rendered = self.desired_for_node(node_id).await;
        let version = self.fleet.next_version_for(node_id, &rendered.render_hash);
        let snapshot = rendered.to_snapshot(node_id, version, now_unix());
        let Some((push_id, waiter)) = self.push_and_await(node_id, snapshot).await else {
            return false; // stream gone
        };
        match tokio::time::timeout(WAVE_ACK_TIMEOUT, waiter).await {
            Ok(Ok(AckResult::Acked { hash })) => hash == rendered.render_hash,
            _ => {
                self.clear_pending(node_id, push_id).await;
                false
            }
        }
    }

    /// The per-node desired render for `node_id`, computed from its stored labels
    /// (GatewaySet-stamped when applicable). Falls back to the fleet-wide applied
    /// render if the node's labels are unknown.
    async fn desired_for_node(&self, node_id: &str) -> crate::render::Rendered {
        match self.store.get(node_id) {
            Some(n) => self.fleet.desired_for(&n.labels),
            None => self.fleet.applied(),
        }
    }

    /// `(node_id, labels)` for every connected node, read from the runtime store.
    async fn connected_node_labels(
        &self,
    ) -> Vec<(String, std::collections::BTreeMap<String, String>)> {
        let mut out = Vec::new();
        for id in self.sessions.connected_ids().await {
            let labels = self.store.get(&id).map(|n| n.labels).unwrap_or_default();
            out.push((id, labels));
        }
        out
    }

    /// Push the wave's nodes their OWN per-node desired renders concurrently,
    /// collect each node's Ack/Nack (bounded by the wave timeout), and adjudicate
    /// against each node's OWN expected render_hash. Every node in the wave must
    /// ack its own render for the wave to commit.
    async fn run_wave_nodes(
        &self,
        trigger: &str,
        wave_name: &str,
        node_ids: &[String],
    ) -> WaveOutcome {
        let mut expected: Vec<(String, String)> = Vec::new();
        let mut waiters: Vec<(String, PendingHandle)> = Vec::new();
        let mut wave_version = self.fleet.committed_version();

        for node_id in node_ids {
            let rendered = self.desired_for_node(node_id).await;
            let version = self.fleet.next_version_for(node_id, &rendered.render_hash);
            wave_version = wave_version.max(version);
            expected.push((node_id.clone(), rendered.render_hash.clone()));
            let snapshot = rendered.to_snapshot(node_id, version, now_unix());
            let waiter = self.push_and_await(node_id, snapshot).await;
            waiters.push((node_id.clone(), waiter));
        }

        info!(
            "[rollout]   wave {wave_name:?} (trigger={trigger}) pushing to {} node(s): {:?}",
            node_ids.len(),
            node_ids,
        );

        let mut results: Vec<(String, AckResult)> = Vec::new();
        for (node_id, waiter) in waiters {
            let result = match waiter {
                None => AckResult::Silent,
                Some((push_id, rx)) => match tokio::time::timeout(WAVE_ACK_TIMEOUT, rx).await {
                    Ok(Ok(r)) => r,
                    Ok(Err(_)) | Err(_) => {
                        self.clear_pending(&node_id, push_id).await;
                        AckResult::Silent
                    }
                },
            };
            results.push((node_id, result));
        }

        // A representative hash for the outcome label: the first node's expected.
        let outcome_hash = expected
            .first()
            .map(|(_, h)| h.clone())
            .unwrap_or_default();
        self.fleet
            .conclude_wave_multi(&outcome_hash, &expected, wave_version, &results)
    }

    /// Distribute a HAND-AUTHORED snapshot (raw bytes) to the fleet, bypassing
    /// the repo render gate. This is the break-glass / testing affordance that
    /// exercises the node's INDEPENDENT validation authority (docs/07: "A
    /// snapshot that fails local validation is Nacked and the old one keeps
    /// serving"). The control plane's render gate is the first defense; the
    /// node's NACK is the second, and this path lets an operator (or the demo)
    /// prove the second one is real — a snapshot that does not validate at the
    /// node is NACKed, the wave halts, and the fleet's committed version does
    /// not advance.
    pub async fn roll_out_raw(&self, trigger: &str, bytes: Vec<u8>) -> WaveOutcome {
        // A synthetic hash/commit for the injected bytes so the node's no-op
        // short-circuit still works and the record is honest about its origin.
        let hash = gateway_core::snapshot::content_hash(&String::from_utf8_lossy(&bytes));
        let commit = format!("raw-{}", &hash[..16.min(hash.len())]);
        self.run_wave(trigger, &hash, &commit, &bytes).await
    }

    /// TEST-ONLY: push `bytes` while ADVERTISING `advertised_hash`, which the
    /// caller deliberately makes inconsistent with the bytes. This simulates a
    /// control-plane bug (or tampering) that sets `render_hash` to something
    /// other than the hash of the config it actually ships. An honest node
    /// recomputes the hash of the bytes it binds and ACKs with THAT (not the
    /// advertised value), so the wave adjudicates the node's true hash against
    /// the advertised one and reports a `WrongHash` divergence — the independent
    /// verification docs/07 line 71 requires. Never compiled into a release
    /// build; it exists solely to prove the node's ACK carries a locally-derived
    /// hash, not a parroted one.
    #[cfg(feature = "test-support")]
    pub async fn roll_out_tampered(
        &self,
        trigger: &str,
        advertised_hash: &str,
        bytes: Vec<u8>,
    ) -> WaveOutcome {
        let commit = format!("tampered-{}", &advertised_hash[..16.min(advertised_hash.len())]);
        self.run_wave(trigger, advertised_hash, &commit, &bytes).await
    }

    /// The shared wave body: push `bytes` (addressed per node with `hash`/
    /// `commit`) to every connected node, collect Acks/Nacks, adjudicate.
    async fn run_wave(
        &self,
        trigger: &str,
        hash: &str,
        commit: &str,
        bytes: &[u8],
    ) -> WaveOutcome {
        // Mark the wave in flight for the whole body (dropped on any return or
        // panic). While held, the reconciler tolerates a not-yet-rolled node as
        // mid-rollout instead of racing it with a heal push (docs/07: "the
        // reconciler does not fight a legitimately mid-rollout node").
        let _wave_guard = self.fleet.begin_wave();
        let node_ids = self.sessions.connected_ids().await;

        if node_ids.is_empty() {
            let outcome = self
                .fleet
                .conclude_wave(hash, self.fleet.committed_version(), &[]);
            info!(
                "[rollout] trigger={trigger} render_hash={} no connected nodes; \
                 applied as desired state, no wave ran",
                short(hash)
            );
            return outcome;
        }

        // Push to every node, each at its own next per-node version. Each
        // waiter carries the unique push-id its pending slot lives under, so a
        // timeout clears exactly this wave's slot and never a concurrent push's.
        let mut waiters: Vec<(String, PendingHandle)> = Vec::new();
        // The wave's version is the max per-node version assigned this round;
        // in M1 every node advances in lockstep so they coincide.
        let mut wave_version = self.fleet.committed_version();
        for node_id in &node_ids {
            let version = self.fleet.next_version_for(node_id, hash);
            wave_version = wave_version.max(version);
            let snapshot = RenderedSnapshot {
                node_id: node_id.clone(),
                source_commit: commit.to_string(),
                render_hash: hash.to_string(),
                fleet_version: version,
                config: bytes.to_vec(),
                compiled_at: now_unix(),
            };
            let waiter = self.push_and_await(node_id, snapshot).await;
            waiters.push((node_id.clone(), waiter));
        }

        info!(
            "[rollout] trigger={trigger} pushing render_hash={} v={wave_version} to {} node(s): {:?}",
            short(hash),
            node_ids.len(),
            node_ids,
        );

        // Await each node's answer (bounded by the wave timeout).
        let mut results: Vec<(String, AckResult)> = Vec::new();
        for (node_id, waiter) in waiters {
            let result = match waiter {
                None => AckResult::Silent,
                Some((push_id, rx)) => match tokio::time::timeout(WAVE_ACK_TIMEOUT, rx).await {
                    Ok(Ok(r)) => r,
                    // Sender dropped or timed out: the node did not answer.
                    Ok(Err(_)) | Err(_) => {
                        // Clean up this push's dangling pending entry.
                        self.clear_pending(&node_id, push_id).await;
                        AckResult::Silent
                    }
                },
            };
            results.push((node_id, result));
        }

        let outcome = self.fleet.conclude_wave(hash, wave_version, &results);
        self.log_outcome(trigger, &outcome);
        outcome
    }

    fn log_outcome(&self, trigger: &str, outcome: &WaveOutcome) {
        match outcome {
            WaveOutcome::Committed {
                render_hash,
                node_count,
            } => info!(
                "[rollout] COMMITTED trigger={trigger} render_hash={} across {node_count} node(s); \
                 fleet committed_version=v{}",
                short(render_hash),
                self.fleet.committed_version(),
            ),
            WaveOutcome::NoNodes { render_hash } => info!(
                "[rollout] applied render_hash={} (no nodes)",
                short(render_hash)
            ),
            WaveOutcome::Halted {
                render_hash,
                divergences,
            } => {
                // Loud by design (docs/07): the fleet did NOT advance and every
                // divergent node is named with its reason.
                error!(
                    "[rollout] HALTED trigger={trigger} render_hash={}: the wave did not \
                     fully ack; fleet committed_version STAYS v{} (not advanced). \
                     {} divergence(s):",
                    short(render_hash),
                    self.fleet.committed_version(),
                    divergences.len(),
                );
                for d in divergences {
                    error!("[rollout]   divergent node {}", describe(d));
                }
            }
        }
    }
}

/// A human description of one divergence, for the loud halt log lines.
fn describe(d: &Divergence) -> String {
    match &d.kind {
        DivergenceKind::Nacked { version, reason } => {
            format!("{} NACKed v{version}: {reason}", d.node_id)
        }
        DivergenceKind::Silent { version } => {
            format!("{} SILENT on v{version} (no ack within timeout)", d.node_id)
        }
        DivergenceKind::WrongHash {
            version,
            expected,
            got,
        } => format!(
            "{} acked v{version} with WRONG hash (expected {}, got {})",
            d.node_id,
            short(expected),
            short(got)
        ),
    }
}
