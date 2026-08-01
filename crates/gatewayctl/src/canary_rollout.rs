//! The CONFIG-CANARY rollout (Phase 5): the analysis-gated superset of the plain
//! multi-wave walk in [`crate::rollout`], plus the Git-native manual judgment
//! gate (docs/04 Phase 5; docs/07 "the canary story is waves with analysis
//! between them"; docs/00 "Kayenta-style analysis + manual judgment gates as
//! Git-native mechanisms, not a pipeline engine").
//!
//! This is a second `impl ControlPlane` block, split out of `rollout.rs` purely
//! to keep each file under the size budget; it shares the wave/session machinery
//! through the `pub(crate)` helpers exactly as `rollout.rs` does. It is NOT a
//! pipeline engine and NOT a separate analysis service (docs/07 anti-goal): the
//! analysis runs from the fleet's OWN telemetry (`crate::telemetry` +
//! `crate::budget`) via `crate::canary`, and the gate is a Git artifact
//! ([`RepoGateSignal`]), not a click endpoint. No new dependency.
//!
//! Reuses: the wave PLAN and assignment ([`crate::waves`]), the per-wave push/
//! ack machinery and the `WaveStep`/`MultiWaveOutcome` surfacing
//! ([`crate::rollout`]), and the wave-in-flight guard + per-wave committed state
//! ([`crate::fleet`]).

use std::collections::BTreeMap;

use log::{error, info, warn};

use crate::canary::{analyze, Analysis, Breach, CanaryPolicy, NodeTelemetry, WaveTelemetry};
use crate::fleet::WaveOutcome;
use crate::rollout::{MultiWaveOutcome, WaveStep, WaveStepState, GATE_POLL_INTERVAL, NEVER_COMMITTED};
use crate::server::{now_unix, short, ControlPlane, WAVE_ACK_TIMEOUT};

/// The result of one wave's canary analysis within a canary rollout, surfaced
/// per wave alongside the [`WaveStep`]. Records whether the wave was analyzed,
/// passed, failed (with the tripping breach), or was held at a judgment gate.
#[derive(Debug, Clone, PartialEq)]
pub enum WaveAnalysis {
    /// Analysis was OFF or the wave had no canary to analyze (empty / final).
    NotAnalyzed,
    /// Every enabled metric was within threshold; the wave advanced.
    Passed,
    /// A metric breached; the rollout auto-rolled-back from this wave.
    Failed(Breach),
    /// The canary reported no telemetry in the window (inconclusive). Fail-closed:
    /// the rollout does NOT advance blind past a canary it could not measure.
    NoData,
}

/// The whole canary rollout's outcome: the underlying multi-wave steps plus, per
/// wave, its analysis verdict, and — when a canary failed — the auto-rollback
/// record. Surfaced so an operator reads exactly which wave tripped, on which
/// metric, and what version the fleet reverted to.
#[derive(Debug, Clone, PartialEq)]
pub struct CanaryOutcome {
    pub multi: MultiWaveOutcome,
    /// Per-wave analysis verdicts, index-aligned with `multi.steps`.
    pub analyses: Vec<WaveAnalysis>,
    /// `Some` when a canary analysis failed and the rollout auto-rolled-back.
    pub rollback: Option<Rollback>,
    /// Wave names where the rollout paused for a Git-expressed manual approval
    /// before proceeding (the judgment gate was satisfied).
    pub gates_released: Vec<String>,
}

/// The record of an auto-rollback: the wave whose canary failed, the metric that
/// tripped it, and the version the fleet reverted to (docs Phase 5: "surface the
/// rollback loudly with the metric that tripped it, the wave, and the version
/// reverted to").
#[derive(Debug, Clone, PartialEq)]
pub struct Rollback {
    pub wave_name: String,
    /// The metric that tripped the rollback, when a metric actually breached.
    /// `None` for a FAIL-CLOSED rollback on INCONCLUSIVE telemetry (the canary
    /// reported nothing in the window): there is no breach to name, and the
    /// surfaced cause is "no telemetry", not a fabricated zero-value breach.
    pub breach: Option<Breach>,
    /// The commit the failing wave (and all later waves) reverted to.
    pub reverted_to: String,
}

impl Rollback {
    /// The one-line surfaced cause of this rollback: the tripping metric when a
    /// metric breached, or the honest "inconclusive" reason when the canary
    /// reported no telemetry (fail-closed). Never fabricates a zero-value breach.
    pub fn cause(&self) -> String {
        match &self.breach {
            Some(b) => b.describe(),
            None => "inconclusive: no telemetry from the canary this window".to_string(),
        }
    }
}

impl CanaryOutcome {
    /// Whether the rollout fully applied: every wave advanced AND no canary
    /// analysis failed.
    pub fn is_fully_applied(&self) -> bool {
        self.multi.is_fully_applied() && self.rollback.is_none()
    }

    /// Whether the rollout auto-rolled-back on a failed canary.
    pub fn rolled_back(&self) -> bool {
        self.rollback.is_some()
    }

    /// A one-line surfaced summary naming the outcome and, on rollback, the
    /// tripping metric + wave + reverted-to version.
    pub fn summary(&self) -> String {
        match &self.rollback {
            None => format!("canary rollout OK: {}", self.multi.summary()),
            Some(rb) => format!(
                "canary rollout ROLLED BACK at wave {:?} on {} -> reverted to {}; later waves frozen",
                rb.wave_name,
                rb.cause(),
                // Show the never-committed sentinel in full; short-hash a real commit.
                if rb.reverted_to == NEVER_COMMITTED {
                    rb.reverted_to.clone()
                } else {
                    short(&rb.reverted_to).to_string()
                },
            ),
        }
    }
}

/// The Git-native manual judgment gate: how the control plane learns an operator
/// has APPROVED proceeding past a gated wave boundary. It is NOT a pipeline
/// "click to approve" engine and NOT a running-state stages machine — the gate
/// is satisfied by an ARTIFACT in the config repo (docs/00: "manual judgment
/// gates as Git-native mechanisms, not a pipeline engine"; docs/04 Phase 5:
/// "approvals on the wave PR").
///
/// The mechanism, stated plainly: an `approvals/<wave>.approved` file committed
/// to the config repo IS the approval. The operator approves the wave PR by
/// committing that artifact (a reviewed, audited, revertable Git change), the
/// same way every other desired-state change is expressed. The control plane
/// polls the repo for the artifact; when it appears, the gate releases and the
/// rollout proceeds. There is no separate approval database, no click endpoint,
/// no stages engine — the approval lives in Git like all other truth.
pub trait GateSignal: Send + Sync {
    /// Whether the manual approval for `wave_name` is present in the config repo
    /// right now. The control plane calls this to decide whether the gate has
    /// been satisfied. For the file-artifact mechanism this checks whether
    /// `approvals/<wave_name>.approved` exists in the config source.
    fn approved(&self, wave_name: &str) -> bool;
}

/// The concrete Git-native gate mechanism: an approval is a FILE in the config
/// source, `approvals/<wave>.approved`. Constructed by re-resolving the config
/// source each check, so the freshest committed state decides — the operator
/// approving the wave PR (committing `approvals/<wave>.approved`) is exactly what
/// releases the gate. No approval database, no click endpoint, no stages engine.
///
/// The mechanism, plainly: to APPROVE a held wave, the operator adds
/// `approvals/<wave>.approved` to the config repo and commits it (the reviewed,
/// audited, revertable wave-PR approval docs/04 names). The control plane, paused
/// at the gate, polls the source; when the artifact is present, it proceeds.
pub struct RepoGateSignal {
    source: std::sync::Arc<dyn crate::source::ConfigSource>,
}

impl RepoGateSignal {
    /// Build a gate that checks `source` for the approval artifact on each poll.
    pub fn new(source: std::sync::Arc<dyn crate::source::ConfigSource>) -> RepoGateSignal {
        RepoGateSignal { source }
    }

    /// The repo-relative path of the approval artifact for `wave`.
    pub fn approval_path(wave: &str) -> String {
        format!("approvals/{wave}.approved")
    }
}

impl GateSignal for RepoGateSignal {
    fn approved(&self, wave_name: &str) -> bool {
        // Re-resolve so a just-committed approval is seen (poll is the floor).
        match self.source.resolve() {
            Ok(resolved) => resolved.contains(&Self::approval_path(wave_name)),
            Err(_) => false,
        }
    }
}

/// A gate that is ALWAYS satisfied — the no-manual-gate default and the
/// convenience for a rollout whose policy configures no `manual_gate_after`.
pub struct AutoApprove;

impl GateSignal for AutoApprove {
    fn approved(&self, _wave_name: &str) -> bool {
        true
    }
}

impl ControlPlane {
    /// The CANARY rollout (Phase 5): the ordered multi-wave walk with a config-
    /// canary ANALYSIS window between waves and AUTO-ROLLBACK on a breach, plus
    /// the Git-native manual judgment gate. This is the analysis-gated superset of
    /// [`ControlPlane::roll_out_plan`]; when the fleet's canary policy is disabled
    /// it degenerates to exactly that plain walk (so the Phase-2 behavior is the
    /// no-canary case).
    ///
    /// Per wave, in order:
    /// 1. Push the wave and await acks (the existing all-or-nothing wave). A
    ///    Nack/timeout halts and freezes forward, exactly as before.
    /// 2. If the wave advanced AND analysis is on, open an ANALYSIS WINDOW: reset
    ///    the canary wave's and the baseline pool's observed windows, wait the
    ///    policy's `window_secs` while telemetry flows up the existing stream,
    ///    then assemble the canary-vs-baseline snapshots from the fleet's OWN
    ///    telemetry (error rate, p99 from `Status`/NACK; token-spend from the
    ///    budget ledger) and run [`crate::canary::analyze`].
    ///    - PASS: advance to the next wave.
    ///    - FAIL / NoData (fail-closed): AUTO-ROLLBACK — revert the failing wave
    ///      and all later waves to their prior committed commit, do NOT advance
    ///      the fleet, and surface the tripping metric loudly. Stop the walk.
    /// 3. If the policy marks a manual judgment gate after this wave, PAUSE and
    ///    wait for the Git-expressed approval (`gate.approved(wave)`) before
    ///    proceeding — the operator commits the approval artifact to the repo.
    ///
    /// The BASELINE is the pool of nodes NOT in the canary wave that are still on
    /// the OLD version — the not-yet-rolled later waves (docs Phase 5: "compares
    /// the canary wave against a baseline: the not-yet-rolled waves still on the
    /// old version"). When no such pool exists (the last wave), analysis compares
    /// against the already-analyzed-healthy earlier waves' stored baseline, or,
    /// absent that, treats the canary's own absolute thresholds.
    ///
    /// `window_override` lets tests/demo shorten the analysis window; production
    /// passes `None` to use the policy's `window_secs`.
    ///
    /// `prior_render` is the fleet-wide render the fleet was on BEFORE this
    /// rollout applied its target — the version to re-push on an auto-rollback so
    /// the canary wave's nodes actually return to the old config (not just a
    /// bookkeeping revert). The caller (the reload path) captures it with
    /// `fleet.applied()` before calling `set_applied_from_source`. `None` when
    /// there is no prior render (a first-ever rollout); a rollback then reverts
    /// bookkeeping only, since there is no earlier version to return to.
    pub async fn roll_out_plan_canary(
        &self,
        trigger: &str,
        gate: &dyn GateSignal,
        window_override: Option<std::time::Duration>,
        prior_render: Option<crate::render::Rendered>,
    ) -> CanaryOutcome {
        let policy = self.fleet.canary_policy();

        // Canary OFF -> the plain multi-wave walk, wrapped as a CanaryOutcome
        // with every wave NotAnalyzed. The Phase-2 behavior is the degenerate case.
        if !policy.enabled {
            let multi = self.roll_out_plan(trigger).await;
            let analyses = vec![WaveAnalysis::NotAnalyzed; multi.steps.len()];
            return CanaryOutcome {
                multi,
                analyses,
                rollback: None,
                gates_released: Vec::new(),
            };
        }

        let _wave_guard = self.fleet.begin_wave();
        let plan = self.fleet.wave_plan();
        let target = self.fleet.applied();
        let target_commit = target.source_commit.clone();

        let node_labels = self.connected_node_labels().await;
        let label_refs: Vec<(&str, &BTreeMap<String, String>)> =
            node_labels.iter().map(|(id, l)| (id.as_str(), l)).collect();
        let assigned = plan.assign(label_refs.iter().copied());

        info!(
            "[canary] trigger={trigger} analysis-gated rollout of commit={} over {} wave(s); \
             policy: window={}s err+{:.3} p99x{:.2} spendx{:.2}/z{:.1}",
            short(&target_commit),
            assigned.len(),
            policy.window_secs,
            policy.max_error_rate_increase,
            policy.max_p99_factor,
            policy.max_spend_factor,
            policy.spend_zscore,
        );

        let mut steps: Vec<WaveStep> = Vec::new();
        let mut analyses: Vec<WaveAnalysis> = Vec::new();
        let mut halted_at: Option<usize> = None;
        let mut rollback: Option<Rollback> = None;
        let mut gates_released: Vec<String> = Vec::new();

        for (idx, wave) in assigned.iter().enumerate() {
            // Once anything halted or rolled back, later waves are FROZEN.
            if halted_at.is_some() || rollback.is_some() {
                steps.push(WaveStep {
                    wave_name: wave.name.clone(),
                    node_count: wave.node_ids.len(),
                    state: WaveStepState::Frozen {
                        commit: self.frozen_commit(&wave.name),
                    },
                });
                analyses.push(WaveAnalysis::NotAnalyzed);
                info!("[canary]   wave {:?}: FROZEN (an earlier wave failed/halted)", wave.name);
                continue;
            }

            if wave.node_ids.is_empty() {
                steps.push(WaveStep {
                    wave_name: wave.name.clone(),
                    node_count: 0,
                    state: WaveStepState::Empty,
                });
                analyses.push(WaveAnalysis::NotAnalyzed);
                continue;
            }

            // Step 1: push the wave and await acks (the existing wave machinery).
            let outcome = self.run_wave_nodes(trigger, &wave.name, &wave.node_ids).await;
            match outcome {
                WaveOutcome::Halted { divergences, .. } => {
                    halted_at = Some(idx);
                    let prior = self.frozen_commit(&wave.name);
                    error!(
                        "[canary]   wave {:?}: HALTED on ack ({} divergence(s)); freezing forward",
                        wave.name,
                        divergences.len()
                    );
                    steps.push(WaveStep {
                        wave_name: wave.name.clone(),
                        node_count: wave.node_ids.len(),
                        state: WaveStepState::Halted { commit: prior, divergences },
                    });
                    analyses.push(WaveAnalysis::NotAnalyzed);
                    continue;
                }
                WaveOutcome::NoNodes { .. } => {
                    steps.push(WaveStep {
                        wave_name: wave.name.clone(),
                        node_count: 0,
                        state: WaveStepState::Empty,
                    });
                    analyses.push(WaveAnalysis::NotAnalyzed);
                    continue;
                }
                WaveOutcome::Committed { node_count, .. } => {
                    // The wave ACKED the new render but is NOT yet committed to
                    // it — the per-wave commit stays on its PRIOR value until the
                    // canary analysis clears it. Capturing the prior commit now is
                    // what makes the rollback target honest (the version to revert
                    // to on a breach).
                    let prior_commit = self.frozen_commit(&wave.name);
                    info!(
                        "[canary]   wave {:?}: acked across {node_count} node(s); opening analysis window",
                        wave.name
                    );

                    // Step 2: the analysis window over the fleet's OWN telemetry.
                    let baseline_ids = self.baseline_ids(&assigned, idx);
                    let analysis = self
                        .analyze_wave(&policy, &wave.node_ids, &baseline_ids, window_override)
                        .await;

                    match analysis {
                        Analysis::Pass => {
                            // Analysis cleared it: NOW commit the wave to target.
                            self.fleet.set_wave_commit(&wave.name, &target_commit);
                            info!("[canary]   wave {:?}: analysis PASSED -> advance", wave.name);
                            steps.push(WaveStep {
                                wave_name: wave.name.clone(),
                                node_count,
                                state: WaveStepState::Advanced { commit: target_commit.clone() },
                            });
                            analyses.push(WaveAnalysis::Passed);
                        }
                        Analysis::Fail(breach) => {
                            // AUTO-ROLLBACK: re-push the PRIOR render to this
                            // wave's nodes and freeze all later waves.
                            let reverted_to = self
                                .rollback_wave(&wave.name, &wave.node_ids, &prior_commit, prior_render.as_ref())
                                .await;
                            error!(
                                "[canary]   wave {:?}: analysis FAILED -> AUTO-ROLLBACK. \
                                 {}. reverted to {}; later waves FROZEN.",
                                wave.name,
                                breach.describe(),
                                short(&reverted_to),
                            );
                            steps.push(WaveStep {
                                wave_name: wave.name.clone(),
                                node_count,
                                state: WaveStepState::Halted {
                                    commit: reverted_to.clone(),
                                    divergences: Vec::new(),
                                },
                            });
                            analyses.push(WaveAnalysis::Failed(breach.clone()));
                            rollback = Some(Rollback {
                                wave_name: wave.name.clone(),
                                breach: Some(breach),
                                reverted_to,
                            });
                            continue;
                        }
                        Analysis::NoData => {
                            // Fail-closed: an unmeasurable canary does not advance
                            // blind. Roll it back like a breach, but tagged NoData.
                            let reverted_to = self
                                .rollback_wave(&wave.name, &wave.node_ids, &prior_commit, prior_render.as_ref())
                                .await;
                            warn!(
                                "[canary]   wave {:?}: analysis INCONCLUSIVE (no telemetry) -> \
                                 fail-closed AUTO-ROLLBACK to {}; later waves FROZEN.",
                                wave.name,
                                short(&reverted_to),
                            );
                            steps.push(WaveStep {
                                wave_name: wave.name.clone(),
                                node_count,
                                state: WaveStepState::Halted {
                                    commit: reverted_to.clone(),
                                    divergences: Vec::new(),
                                },
                            });
                            analyses.push(WaveAnalysis::NoData);
                            // Fail-closed on INCONCLUSIVE telemetry: there is no
                            // metric breach to name, so `breach` is None and the
                            // surfaced cause reads "no telemetry" — not a
                            // fabricated all-zero error-rate breach.
                            rollback = Some(Rollback {
                                wave_name: wave.name.clone(),
                                breach: None,
                                reverted_to,
                            });
                            continue;
                        }
                    }

                    // Step 3: the Git-native manual judgment gate, if configured
                    // after this wave. Pause until the approval artifact appears.
                    if policy.gates_after(&wave.name) {
                        info!(
                            "[canary]   wave {:?}: PAUSED at manual judgment gate; \
                             waiting for the Git-expressed approval (approvals/{}.approved)",
                            wave.name, wave.name
                        );
                        self.await_gate(gate, &wave.name).await;
                        info!(
                            "[canary]   wave {:?}: gate RELEASED (approval committed) -> proceed",
                            wave.name
                        );
                        gates_released.push(wave.name.clone());
                    }
                }
            }
        }

        let multi = MultiWaveOutcome {
            steps,
            halted_at,
            target_commit,
        };
        let out = CanaryOutcome {
            multi,
            analyses,
            rollback,
            gates_released,
        };
        if out.rolled_back() {
            error!("[canary] trigger={trigger} {}", out.summary());
        } else {
            info!("[canary] trigger={trigger} {}", out.summary());
        }
        out
    }

    /// The baseline node pool for the canary wave at `idx`: the nodes in LATER
    /// waves (still on the old version, not yet rolled). This is the "not-yet-
    /// rolled waves still on the old version" baseline docs Phase 5 names. Empty
    /// when the canary is the last non-empty wave.
    fn baseline_ids(&self, assigned: &[crate::waves::AssignedWave], idx: usize) -> Vec<String> {
        assigned
            .iter()
            .skip(idx + 1)
            .flat_map(|w| w.node_ids.iter().cloned())
            .collect()
    }

    /// Open an analysis window and adjudicate: reset the canary + baseline
    /// windows, wait for telemetry to flow (the policy window, or an override),
    /// assemble both snapshots from the fleet's OWN telemetry, and run
    /// [`analyze`]. A window of zero (tests/demo pre-seed telemetry) skips the
    /// sleep and reads whatever is already recorded.
    async fn analyze_wave(
        &self,
        policy: &CanaryPolicy,
        canary_ids: &[String],
        baseline_ids: &[String],
        window_override: Option<std::time::Duration>,
    ) -> Analysis {
        let window = window_override.unwrap_or_else(|| policy.window());
        if !window.is_zero() {
            // Open a FRESH window and collect for the policy duration. For the
            // INFRA signals (error rate, p99) the canary's accumulated window is
            // reset; the baseline keeps its accumulated infra telemetry, since a
            // ratio/percentile stays comparable regardless of how long it ran.
            //
            // For SPEND, cumulative lifetime totals are only apples-to-apples when
            // canary and baseline have comparable uptime — a fresh canary vs a
            // long-running baseline would compare lifetime totals and mask (or
            // fabricate) an anomaly. So snapshot BOTH pools' spend at window open
            // and compare the per-window DELTA (see `node_windowed_spend`). This
            // is the spend analogue of the infra `reset_many`, but non-destructive
            // (the cumulative ledger stays intact for share allocation / GB-6).
            //
            // A zero window (tests/demo pre-seed the telemetry, every node fresh in
            // the same window so cumulative == windowed) skips the reset, the
            // snapshot, and the wait, reading whatever is already recorded.
            self.telemetry.reset_many(canary_ids);
            let mut spend_window: Vec<String> = Vec::with_capacity(canary_ids.len() + baseline_ids.len());
            spend_window.extend_from_slice(canary_ids);
            spend_window.extend_from_slice(baseline_ids);
            self.budgets.open_spend_window(&spend_window);
            tokio::time::sleep(window).await;
        }

        let canary = self.assemble_telemetry(canary_ids);
        let baseline = self.assemble_telemetry(baseline_ids);
        analyze(policy, &canary, &baseline)
    }

    /// Assemble a [`WaveTelemetry`] snapshot for a set of nodes from the fleet's
    /// OWN ingested telemetry: request/error/latency from the observed-telemetry
    /// sink (`Status`/NACK), and per-node token spend from the budget ledger.
    /// No new collection path — both sources already exist.
    fn assemble_telemetry(&self, node_ids: &[String]) -> WaveTelemetry {
        let mut wt = WaveTelemetry::new();
        for id in node_ids {
            let w = self.telemetry.window_for(id);
            wt.insert(
                id,
                NodeTelemetry {
                    requests: w.requests,
                    errors: w.errors,
                    latencies_ms: w.latencies_ms,
                    // The per-WINDOW spend delta (since `open_spend_window`), not
                    // the cumulative lifetime total — so a fresh canary is not
                    // judged against a long-running baseline's lifetime spend.
                    spent: self.budgets.node_windowed_spend(id),
                },
            );
        }
        wt
    }

    /// Auto-rollback one wave whose canary FAILED: re-push the PRIOR render to
    /// the wave's nodes so they actually return to the old version, and record
    /// the wave back on `prior_commit`. Returns the commit reverted to. Because
    /// the walk freezes all LATER waves after a rollback, this plus the freeze
    /// reverts the whole not-yet-committed canary (docs Phase 5: "a full rollback
    /// of a canary that never fully committed is cleanest"); earlier already-
    /// analyzed-healthy waves stay advanced.
    ///
    /// The re-push is the SAME wave machinery aimed backward: it pushes the prior
    /// render bytes at a fresh per-node version and does not require the node to
    /// ack for the rollback to be recorded (a node that already swapped to the bad
    /// target and cannot swap back is surfaced by the reconciler; the fleet's
    /// committed state is reverted regardless so it never reads as advanced). When
    /// there is no prior render (a first-ever rollout), only the bookkeeping is
    /// reverted — there is no earlier version to return to.
    async fn rollback_wave(
        &self,
        wave_name: &str,
        node_ids: &[String],
        prior_commit: &str,
        prior_render: Option<&crate::render::Rendered>,
    ) -> String {
        // Record the wave back on its prior commit FIRST, so the surfaced state
        // is correct even if a re-push ack is slow.
        self.fleet.revert_wave_commit(wave_name, prior_commit);

        // Re-push the prior render to each node so it returns to the old config.
        // The revert is already RECORDED above, so a slow/failed swap-back does
        // not leave the fleet reading as advanced; a node that cannot swap back is
        // caught by the reconciler's drift self-heal. We still await each ack
        // (bounded by the wave timeout) so the common case — the node swaps back
        // cleanly — completes before the walk returns and the log reads honestly.
        if let Some(prior) = prior_render {
            for node_id in node_ids {
                let version = self.fleet.next_version_for(node_id, &prior.render_hash);
                let snapshot = prior.to_snapshot(node_id, version, now_unix());
                if let Some((push_id, waiter)) = self.push_and_await(node_id, snapshot).await {
                    if tokio::time::timeout(WAVE_ACK_TIMEOUT, waiter).await.is_err() {
                        self.clear_pending(node_id, push_id).await;
                    }
                }
            }
        }
        prior_commit.to_string()
    }

    /// Wait for the Git-native manual judgment gate to be satisfied: poll the
    /// config repo (via `gate`) for the approval artifact, sleeping between
    /// checks, until it appears. This is a PAUSE on a Git-expressed signal, not a
    /// pipeline stage — the control plane simply does not proceed until the
    /// operator has committed the approval.
    async fn await_gate(&self, gate: &dyn GateSignal, wave_name: &str) {
        loop {
            if gate.approved(wave_name) {
                return;
            }
            tokio::time::sleep(GATE_POLL_INTERVAL).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rollout::MultiWaveOutcome;

    fn outcome_with(rollback: Option<Rollback>) -> CanaryOutcome {
        CanaryOutcome {
            multi: MultiWaveOutcome {
                steps: Vec::new(),
                halted_at: None,
                target_commit: "deadbeefcafebabe".to_string(),
            },
            analyses: Vec::new(),
            rollback,
            gates_released: Vec::new(),
        }
    }

    #[test]
    fn a_metric_breach_rollback_names_the_tripping_metric() {
        let rb = Rollback {
            wave_name: "canary".to_string(),
            breach: Some(Breach::ErrorRate {
                baseline: 0.01,
                canary: 0.30,
                max_increase: 0.05,
            }),
            reverted_to: "abc123def456".to_string(),
        };
        assert!(rb.cause().contains("error-rate"), "{}", rb.cause());
        let summary = outcome_with(Some(rb)).summary();
        assert!(summary.contains("ROLLED BACK"), "{summary}");
        assert!(summary.contains("error-rate"), "{summary}");
    }

    #[test]
    fn a_no_data_rollback_says_inconclusive_not_a_fabricated_error_rate_breach() {
        // The MEDIUM the critique flagged: a fail-closed NoData rollback must NOT
        // surface a zero-value "error-rate breach: canary 0.0000 vs baseline
        // 0.0000" — it must honestly say the telemetry was inconclusive.
        let rb = Rollback {
            wave_name: "canary".to_string(),
            breach: None,
            reverted_to: "abc123def456".to_string(),
        };
        let cause = rb.cause();
        assert!(cause.contains("inconclusive"), "{cause}");
        assert!(cause.contains("no telemetry"), "{cause}");
        assert!(
            !cause.contains("error-rate"),
            "a NoData rollback must not name a fabricated error-rate breach: {cause}"
        );
        let summary = outcome_with(Some(rb)).summary();
        assert!(summary.contains("ROLLED BACK"), "{summary}");
        assert!(summary.contains("inconclusive"), "{summary}");
        assert!(
            !summary.contains("error-rate"),
            "the surfaced summary must not falsely name an error-rate breach: {summary}"
        );
    }
}
