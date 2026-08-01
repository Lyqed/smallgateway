//! Config-canary ANALYSIS between waves + the Git-native judgment gate
//! (docs/04 Phase 5; docs/07 "The canary story (Phase 5) is waves with analysis
//! between them"; docs/00 "steal Spinnaker's two good ideas — Kayenta-style
//! automated canary analysis and manual judgment gates — but as Git-native
//! mechanisms, not a pipeline engine").
//!
//! This module is the PURE analysis substrate: the canary policy (which metrics,
//! thresholds, window duration, and the per-wave hold gate), a per-wave telemetry
//! snapshot assembled from the fleet's OWN already-ingested telemetry, and the
//! plain-Rust statistics (error rate, latency p99, token-spend anomaly) that
//! compare a canary wave against a baseline. The sequencing that pauses between
//! waves and triggers auto-rollback lives in `rollout.rs`; the telemetry these
//! stats run over is the SAME stream the fleet already collects (Ack/Nack/Silent,
//! `Status.health`, and the `UsageReport` spend the budget ledger ingests) — no
//! new metrics service, no new dependency (docs/07 anti-goal: "A dedicated
//! metrics/analysis service for canaries" is OUT; analysis is "a module").
//!
//! ## The three signals, defined plainly
//!
//! - **Error rate**: `errors / requests` observed on the wave, compared against
//!   the baseline error rate. A canary that raises the error rate by more than
//!   `max_error_rate_increase` (an absolute delta in the rate, e.g. 0.05 = five
//!   points) FAILS.
//! - **Latency p99**: the 99th-percentile observed latency on the wave, compared
//!   against baseline p99. A canary whose p99 exceeds baseline by more than a
//!   factor of `max_p99_factor` FAILS. (p99 is a plain sorted-sample percentile —
//!   no histogram library, no new dependency.)
//! - **Token-spend anomaly** (the domain-aware signal nothing else has): the
//!   canary wave's per-node spend *rate* against the baseline's. A config change
//!   that suddenly makes a wave spend far more — a bad route, a retry loop, the
//!   wrong (pricier) model — shows up as a spend-rate that exceeds baseline by
//!   more than `max_spend_factor`, OR, when there is enough baseline spread, as a
//!   spend-rate whose z-score against the baseline nodes crosses `spend_zscore`.
//!   Either test tripping FAILS the canary. This is read from the budget/spend
//!   telemetry the fleet already ingests, not from infra metrics.
//!
//! The policy lives in the Git config repo (`canary.yaml`), rendered and
//! admission-checked like any config (docs/07 "Truth in Git"): the thresholds
//! and the window are themselves reviewed and reproducible.

use std::collections::BTreeMap;

use serde::Deserialize;

/// The default analysis window in seconds when a policy omits it: how long the
/// control plane collects the canary wave's telemetry before adjudicating.
pub const DEFAULT_WINDOW_SECS: u64 = 60;

/// Default absolute error-rate increase (canary minus baseline) that fails the
/// canary — five percentage points.
pub const DEFAULT_MAX_ERROR_RATE_INCREASE: f64 = 0.05;

/// Default multiplicative p99 factor (canary p99 / baseline p99) that fails the
/// canary — a canary more than 1.5x the baseline p99.
pub const DEFAULT_MAX_P99_FACTOR: f64 = 1.5;

/// Default multiplicative spend-rate factor (canary spend-rate / baseline
/// spend-rate) that fails the canary — a canary spending more than 2x per node.
pub const DEFAULT_MAX_SPEND_FACTOR: f64 = 2.0;

/// Default z-score of the canary's spend-rate against the baseline nodes'
/// spend-rates that fails the canary, when there is enough baseline spread to
/// make a z-score meaningful (>= [`MIN_BASELINE_FOR_ZSCORE`] baseline nodes).
pub const DEFAULT_SPEND_ZSCORE: f64 = 3.0;

/// The z-score test needs a baseline of at least this many nodes to have a
/// meaningful standard deviation; below it, only the factor test applies.
pub const MIN_BASELINE_FOR_ZSCORE: usize = 3;

/// Which metrics a policy enables. A metric that is off is never evaluated and
/// never trips the canary — an operator opts into each signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnabledMetrics {
    pub error_rate: bool,
    pub p99: bool,
    pub spend_anomaly: bool,
}

impl Default for EnabledMetrics {
    /// All three signals on by default — the honest safe default for a canary.
    fn default() -> EnabledMetrics {
        EnabledMetrics {
            error_rate: true,
            p99: true,
            spend_anomaly: true,
        }
    }
}

/// The canary analysis + judgment-gate policy for a rollout. Parsed from the Git
/// config repo (`canary.yaml`), so the thresholds, window, and manual-hold gates
/// are themselves reviewed config, not code constants.
#[derive(Debug, Clone, PartialEq)]
pub struct CanaryPolicy {
    /// Whether analysis runs at all. When `false` the rollout is the plain
    /// multi-wave walk (Phase 2 behavior) — the degenerate no-canary case.
    pub enabled: bool,
    pub metrics: EnabledMetrics,
    pub window_secs: u64,
    pub max_error_rate_increase: f64,
    pub max_p99_factor: f64,
    pub max_spend_factor: f64,
    pub spend_zscore: f64,
    /// Wave names at whose boundary the rollout must PAUSE for a Git-expressed
    /// manual approval before proceeding (the judgment gate). A wave named here
    /// is held after it advances (and passes analysis) until the approval signal
    /// for it is present in the config repo. Empty = no manual gates.
    pub manual_gate_after: Vec<String>,
}

impl Default for CanaryPolicy {
    /// The degenerate policy: analysis OFF (plain multi-wave), all thresholds at
    /// their documented defaults so an enabling edit only flips `enabled`.
    fn default() -> CanaryPolicy {
        CanaryPolicy {
            enabled: false,
            metrics: EnabledMetrics::default(),
            window_secs: DEFAULT_WINDOW_SECS,
            max_error_rate_increase: DEFAULT_MAX_ERROR_RATE_INCREASE,
            max_p99_factor: DEFAULT_MAX_P99_FACTOR,
            max_spend_factor: DEFAULT_MAX_SPEND_FACTOR,
            spend_zscore: DEFAULT_SPEND_ZSCORE,
            manual_gate_after: Vec::new(),
        }
    }
}

impl CanaryPolicy {
    /// Whether the rollout must pause for a manual judgment gate after `wave`.
    pub fn gates_after(&self, wave: &str) -> bool {
        self.manual_gate_after.iter().any(|w| w == wave)
    }

    /// The window as a `Duration` for the sequencer's collection sleep.
    pub fn window(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.window_secs)
    }
}

/// One node's observed telemetry over the analysis window, assembled by the
/// control plane from its OWN ingested signals (no new collection path):
/// - `requests` / `errors`: the request and error counts the node reported this
///   window (Ack/Nack/Silent contribute; the node's `Status.health` string
///   carries the observed request/error tallies — see `server.rs`).
/// - `latencies_ms`: the observed per-request latency samples the node reported
///   this window (for the p99 percentile).
/// - `spent`: cumulative tokens the node spent this window (from the budget
///   ledger's `UsageReport` ingest — the domain-aware spend signal).
#[derive(Debug, Clone, Default)]
pub struct NodeTelemetry {
    pub requests: u64,
    pub errors: u64,
    pub latencies_ms: Vec<f64>,
    pub spent: u64,
}

impl NodeTelemetry {
    /// This node's error rate (`errors / requests`), 0 when it served nothing.
    pub fn error_rate(&self) -> f64 {
        if self.requests == 0 {
            0.0
        } else {
            self.errors as f64 / self.requests as f64
        }
    }
}

/// The telemetry for a set of nodes (a wave, or the baseline pool), keyed by
/// node id. Assembled per-wave from the fleet's ingested telemetry.
#[derive(Debug, Clone, Default)]
pub struct WaveTelemetry {
    pub per_node: BTreeMap<String, NodeTelemetry>,
}

impl WaveTelemetry {
    pub fn new() -> WaveTelemetry {
        WaveTelemetry::default()
    }

    /// Insert one node's telemetry.
    pub fn insert(&mut self, node_id: &str, t: NodeTelemetry) {
        self.per_node.insert(node_id.to_string(), t);
    }

    /// Aggregate error rate across the whole pool: total errors / total requests.
    /// A pool that served nothing has error rate 0.
    fn aggregate_error_rate(&self) -> f64 {
        let mut req = 0u64;
        let mut err = 0u64;
        for t in self.per_node.values() {
            req += t.requests;
            err += t.errors;
        }
        if req == 0 {
            0.0
        } else {
            err as f64 / req as f64
        }
    }

    /// The p99 of every latency sample across the pool (plain sorted-sample
    /// percentile). `None` when the pool reported no samples.
    fn pooled_p99(&self) -> Option<f64> {
        let mut samples: Vec<f64> = self
            .per_node
            .values()
            .flat_map(|t| t.latencies_ms.iter().copied())
            .collect();
        percentile(&mut samples, 0.99)
    }

    /// Each node's spend as a plain list (for the spend-rate mean / z-score).
    fn spend_rates(&self) -> Vec<f64> {
        self.per_node.values().map(|t| t.spent as f64).collect()
    }

    /// The mean per-node spend across the pool (the "spend rate" comparison
    /// point). 0 for an empty pool.
    fn mean_spend(&self) -> f64 {
        let rates = self.spend_rates();
        if rates.is_empty() {
            0.0
        } else {
            rates.iter().sum::<f64>() / rates.len() as f64
        }
    }

    /// Whether any node in this pool reported telemetry (a wave with connected
    /// nodes that reported nothing this window is inconclusive, not healthy).
    pub fn has_samples(&self) -> bool {
        self.per_node.values().any(|t| {
            t.requests > 0 || !t.latencies_ms.is_empty() || t.spent > 0
        })
    }
}

/// The 99th-percentile (or any `q` in [0,1]) of a sample set, by the
/// nearest-rank method over the sorted samples. Mutates `samples` (sorts it).
/// `None` for an empty set. Plain Rust — no histogram/statistics dependency.
fn percentile(samples: &mut [f64], q: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = (q * (samples.len() as f64 - 1.0)).round() as usize;
    Some(samples[rank.min(samples.len() - 1)])
}

/// Population mean and standard deviation of a sample set. `None` for empty.
fn mean_std(values: &[f64]) -> Option<(f64, f64)> {
    if values.is_empty() {
        return None;
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
    Some((mean, var.sqrt()))
}

/// Which metric tripped a failed analysis, carrying the observed vs baseline
/// numbers so the rollback log names exactly what breached (docs Phase 5:
/// "surface the rollback loudly WITH the metric that tripped it").
#[derive(Debug, Clone, PartialEq)]
pub enum Breach {
    /// Error rate rose above baseline by more than the allowed increase.
    ErrorRate {
        baseline: f64,
        canary: f64,
        max_increase: f64,
    },
    /// p99 latency exceeded baseline by more than the allowed factor.
    P99 {
        baseline_ms: f64,
        canary_ms: f64,
        max_factor: f64,
    },
    /// Token-spend anomaly: the canary spend-rate exceeded baseline by more than
    /// the allowed factor, or crossed the z-score against the baseline nodes.
    SpendAnomaly {
        baseline_mean: f64,
        canary_mean: f64,
        factor: f64,
        max_factor: f64,
        zscore: Option<f64>,
        max_zscore: f64,
    },
}

impl Breach {
    /// A one-line, human description for the loud rollback log.
    pub fn describe(&self) -> String {
        match self {
            Breach::ErrorRate {
                baseline,
                canary,
                max_increase,
            } => format!(
                "error-rate breach: canary {:.4} vs baseline {:.4} (+{:.4} > allowed +{:.4})",
                canary,
                baseline,
                canary - baseline,
                max_increase
            ),
            Breach::P99 {
                baseline_ms,
                canary_ms,
                max_factor,
            } => format!(
                "p99 breach: canary {:.1}ms vs baseline {:.1}ms ({:.2}x > allowed {:.2}x)",
                canary_ms,
                baseline_ms,
                canary_ms / baseline_ms.max(f64::MIN_POSITIVE),
                max_factor
            ),
            Breach::SpendAnomaly {
                baseline_mean,
                canary_mean,
                factor,
                max_factor,
                zscore,
                max_zscore,
            } => match zscore {
                Some(z) => format!(
                    "token-spend anomaly: canary mean {:.0} vs baseline mean {:.0} \
                     ({:.2}x > allowed {:.2}x; z={:.2} > allowed {:.2})",
                    canary_mean, baseline_mean, factor, max_factor, z, max_zscore
                ),
                None => format!(
                    "token-spend anomaly: canary mean {:.0} vs baseline mean {:.0} \
                     ({:.2}x > allowed {:.2}x)",
                    canary_mean, baseline_mean, factor, max_factor
                ),
            },
        }
    }
}

/// The verdict of one wave's canary analysis.
#[derive(Debug, Clone, PartialEq)]
pub enum Analysis {
    /// Every enabled metric is within threshold — advance to the next wave.
    Pass,
    /// A metric breached — the rollout must auto-roll-back. Carries the FIRST
    /// breach found (error rate, then p99, then spend), which is the one named
    /// in the loud rollback log.
    Fail(Breach),
    /// The canary reported no telemetry this window — inconclusive. Treated by
    /// the caller as configured (default: fail-closed, do not advance blind).
    NoData,
}

impl Analysis {
    pub fn is_pass(&self) -> bool {
        matches!(self, Analysis::Pass)
    }
}

/// Run the canary analysis for one wave: compare the `canary` wave's telemetry
/// against the `baseline` pool under `policy`. Returns [`Analysis::Pass`] when
/// every ENABLED metric is within threshold, [`Analysis::Fail`] with the first
/// breach otherwise, and [`Analysis::NoData`] when the canary reported nothing.
///
/// Metrics are checked in a fixed order (error rate, then p99, then spend) so the
/// reported breach is deterministic. Pure over the two telemetry snapshots — no
/// I/O, no clock, no dependency — so it is trivially unit-testable and runs
/// inside the control plane between waves.
pub fn analyze(policy: &CanaryPolicy, canary: &WaveTelemetry, baseline: &WaveTelemetry) -> Analysis {
    if !canary.has_samples() {
        return Analysis::NoData;
    }

    // Every enabled metric is a RELATIVE comparison against the baseline pool.
    // When there is no baseline to compare against — the last non-empty wave has
    // no not-yet-rolled peers left on the old version — there is nothing to
    // measure the canary AGAINST, so the relative tests cannot run. The earlier
    // waves already validated this exact config healthily; a final wave with no
    // peer PASSES rather than tripping a false anomaly against an empty baseline
    // (the bug an empty-baseline "infinite factor" would introduce). This is the
    // documented baseline being the "not-yet-rolled waves still on the old
    // version" — absent it, the comparison is a no-op, not a failure.
    if !baseline.has_samples() {
        return Analysis::Pass;
    }

    // 1. Error rate: absolute increase over baseline.
    if policy.metrics.error_rate {
        let b = baseline.aggregate_error_rate();
        let c = canary.aggregate_error_rate();
        if c - b > policy.max_error_rate_increase {
            return Analysis::Fail(Breach::ErrorRate {
                baseline: b,
                canary: c,
                max_increase: policy.max_error_rate_increase,
            });
        }
    }

    // 2. p99 latency: multiplicative factor over baseline. Only meaningful when
    //    both pools reported latency samples; a baseline p99 of 0 (no samples)
    //    cannot form a factor and is skipped rather than dividing by zero.
    if policy.metrics.p99 {
        if let (Some(cp99), Some(bp99)) = (canary.pooled_p99(), baseline.pooled_p99()) {
            if bp99 > 0.0 && cp99 > bp99 * policy.max_p99_factor {
                return Analysis::Fail(Breach::P99 {
                    baseline_ms: bp99,
                    canary_ms: cp99,
                    max_factor: policy.max_p99_factor,
                });
            }
        }
    }

    // 3. Token-spend anomaly: factor OR z-score against the baseline nodes.
    if policy.metrics.spend_anomaly {
        if let Some(breach) = spend_breach(policy, canary, baseline) {
            return Analysis::Fail(breach);
        }
    }

    Analysis::Pass
}

/// The token-spend anomaly test, factored out for clarity. The canary's mean
/// per-node spend is compared against the baseline's two ways; either tripping
/// is a breach:
///
/// 1. **Factor**: `canary_mean / baseline_mean > max_spend_factor`. Catches the
///    common case — a wave that just spends far more per node than its peers.
/// 2. **Z-score**: when the baseline has enough nodes for a meaningful spread
///    (>= [`MIN_BASELINE_FOR_ZSCORE`]) and a non-zero std, how many standard
///    deviations the canary mean sits above the baseline mean. Catches a wave
///    that is an outlier even when the raw factor looks modest because the
///    baseline itself spends a lot.
fn spend_breach(
    policy: &CanaryPolicy,
    canary: &WaveTelemetry,
    baseline: &WaveTelemetry,
) -> Option<Breach> {
    let canary_mean = canary.mean_spend();
    let baseline_rates = baseline.spend_rates();
    let baseline_mean = baseline.mean_spend();

    // A baseline that spent nothing cannot form a factor; if the canary spent
    // ANYTHING against a zero baseline that is itself the anomaly (infinite
    // factor). Guard the divide and treat canary>0 vs baseline==0 as a breach
    // only when the canary actually spent (has_samples already ensured it did
    // something this window, but it may have been requests/latency, not spend).
    if baseline_mean <= 0.0 {
        if canary_mean > 0.0 {
            return Some(Breach::SpendAnomaly {
                baseline_mean,
                canary_mean,
                factor: f64::INFINITY,
                max_factor: policy.max_spend_factor,
                zscore: None,
                max_zscore: policy.spend_zscore,
            });
        }
        return None;
    }

    let factor = canary_mean / baseline_mean;

    // Z-score, only when the baseline is wide enough to make std meaningful.
    let zscore = if baseline_rates.len() >= MIN_BASELINE_FOR_ZSCORE {
        mean_std(&baseline_rates).and_then(|(m, sd)| {
            if sd > 0.0 {
                Some((canary_mean - m) / sd)
            } else {
                None
            }
        })
    } else {
        None
    };

    let factor_breach = factor > policy.max_spend_factor;
    let zscore_breach = zscore.is_some_and(|z| z > policy.spend_zscore);

    if factor_breach || zscore_breach {
        Some(Breach::SpendAnomaly {
            baseline_mean,
            canary_mean,
            factor,
            max_factor: policy.max_spend_factor,
            zscore,
            max_zscore: policy.spend_zscore,
        })
    } else {
        None
    }
}

// --- Parsing the policy from the config repo -------------------------------

/// The on-disk shape of `canary.yaml`. Absent file → analysis OFF (the plain
/// multi-wave walk). Every field is optional and falls back to the documented
/// default, so an enabling edit can be as small as `enabled: true`.
#[derive(Debug, Deserialize)]
struct CanaryFile {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    metrics: Option<MetricsFile>,
    #[serde(default)]
    window_secs: Option<u64>,
    #[serde(default)]
    max_error_rate_increase: Option<f64>,
    #[serde(default)]
    max_p99_factor: Option<f64>,
    #[serde(default)]
    max_spend_factor: Option<f64>,
    #[serde(default)]
    spend_zscore: Option<f64>,
    #[serde(default)]
    manual_gate_after: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct MetricsFile {
    error_rate: Option<bool>,
    p99: Option<bool>,
    spend_anomaly: Option<bool>,
}

/// A malformed `canary.yaml`.
#[derive(Debug)]
pub struct CanaryPolicyError(pub String);

impl std::fmt::Display for CanaryPolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid canary.yaml: {}", self.0)
    }
}

impl std::error::Error for CanaryPolicyError {}

/// Parse a `canary.yaml` document into a [`CanaryPolicy`]. Absent or empty file
/// → the default (analysis OFF). Validates that thresholds are sane (positive
/// factors, a non-negative window) so a nonsensical policy is rejected at
/// admission, not discovered mid-rollout.
pub fn parse_canary_policy(yaml: &str) -> Result<CanaryPolicy, CanaryPolicyError> {
    if yaml.trim().is_empty() {
        return Ok(CanaryPolicy::default());
    }
    let parsed: CanaryFile =
        serde_yaml::from_str(yaml).map_err(|e| CanaryPolicyError(e.to_string()))?;

    let d = CanaryPolicy::default();
    let metrics = match parsed.metrics {
        None => EnabledMetrics::default(),
        Some(m) => EnabledMetrics {
            error_rate: m.error_rate.unwrap_or(true),
            p99: m.p99.unwrap_or(true),
            spend_anomaly: m.spend_anomaly.unwrap_or(true),
        },
    };

    let policy = CanaryPolicy {
        enabled: parsed.enabled,
        metrics,
        window_secs: parsed.window_secs.unwrap_or(d.window_secs),
        max_error_rate_increase: parsed
            .max_error_rate_increase
            .unwrap_or(d.max_error_rate_increase),
        max_p99_factor: parsed.max_p99_factor.unwrap_or(d.max_p99_factor),
        max_spend_factor: parsed.max_spend_factor.unwrap_or(d.max_spend_factor),
        spend_zscore: parsed.spend_zscore.unwrap_or(d.spend_zscore),
        manual_gate_after: parsed.manual_gate_after,
    };

    // Validate: factors must be > 0 (a factor <= 0 would fail every canary or
    // be meaningless), the error-rate increase must be in [0, 1], the z-score
    // must be positive.
    if policy.max_p99_factor <= 0.0 {
        return Err(CanaryPolicyError("max_p99_factor must be > 0".into()));
    }
    if policy.max_spend_factor <= 0.0 {
        return Err(CanaryPolicyError("max_spend_factor must be > 0".into()));
    }
    if !(0.0..=1.0).contains(&policy.max_error_rate_increase) {
        return Err(CanaryPolicyError(
            "max_error_rate_increase must be in [0, 1]".into(),
        ));
    }
    if policy.spend_zscore <= 0.0 {
        return Err(CanaryPolicyError("spend_zscore must be > 0".into()));
    }
    Ok(policy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(requests: u64, errors: u64, latencies: &[f64], spent: u64) -> NodeTelemetry {
        NodeTelemetry {
            requests,
            errors,
            latencies_ms: latencies.to_vec(),
            spent,
        }
    }

    fn healthy_baseline() -> WaveTelemetry {
        let mut t = WaveTelemetry::new();
        // Three baseline nodes: low error rate, ~100ms p99, ~1000 tokens each.
        t.insert("b1", node(1000, 5, &[90.0, 100.0, 110.0], 1000));
        t.insert("b2", node(1000, 4, &[95.0, 100.0, 105.0], 1050));
        t.insert("b3", node(1000, 6, &[92.0, 100.0, 108.0], 980));
        t
    }

    fn enabled_policy() -> CanaryPolicy {
        CanaryPolicy {
            enabled: true,
            ..CanaryPolicy::default()
        }
    }

    // --- Pure statistics -----------------------------------------------------

    #[test]
    fn percentile_of_a_sorted_sample_is_the_nearest_rank() {
        let mut s: Vec<f64> = (1..=100).map(|i| i as f64).collect();
        assert_eq!(percentile(&mut s, 0.99), Some(99.0));
        let mut single = vec![42.0];
        assert_eq!(percentile(&mut single, 0.99), Some(42.0));
        let mut empty: Vec<f64> = vec![];
        assert_eq!(percentile(&mut empty, 0.99), None);
    }

    #[test]
    fn mean_std_of_a_flat_sample_is_zero_spread() {
        let (m, sd) = mean_std(&[5.0, 5.0, 5.0]).unwrap();
        assert_eq!(m, 5.0);
        assert_eq!(sd, 0.0);
    }

    // --- Analysis passes -----------------------------------------------------

    #[test]
    fn a_healthy_canary_within_every_threshold_passes() {
        let baseline = healthy_baseline();
        let mut canary = WaveTelemetry::new();
        // Canary matches the baseline closely: no metric breaches.
        canary.insert("c1", node(1000, 6, &[95.0, 105.0, 112.0], 1020));
        assert_eq!(analyze(&enabled_policy(), &canary, &baseline), Analysis::Pass);
    }

    // --- Each metric breach fails --------------------------------------------

    #[test]
    fn an_elevated_error_rate_fails_the_canary() {
        let baseline = healthy_baseline(); // ~0.5% error rate
        let mut canary = WaveTelemetry::new();
        // 200/1000 = 20% error rate — +19.5 points over baseline, well past +5.
        canary.insert("c1", node(1000, 200, &[95.0, 100.0, 110.0], 1000));
        match analyze(&enabled_policy(), &canary, &baseline) {
            Analysis::Fail(Breach::ErrorRate { canary, baseline, .. }) => {
                assert!(canary > baseline, "canary error rate {canary} > {baseline}");
            }
            other => panic!("expected an error-rate breach, got {other:?}"),
        }
    }

    #[test]
    fn an_elevated_p99_fails_the_canary() {
        let baseline = healthy_baseline(); // p99 ~110ms
        let mut canary = WaveTelemetry::new();
        // p99 ~900ms — well past 1.5x the ~110ms baseline. Error rate/spend fine.
        canary.insert("c1", node(1000, 5, &[850.0, 880.0, 900.0], 1000));
        match analyze(&enabled_policy(), &canary, &baseline) {
            Analysis::Fail(Breach::P99 { canary_ms, baseline_ms, .. }) => {
                assert!(canary_ms > baseline_ms * 1.5, "p99 {canary_ms} > 1.5x {baseline_ms}");
            }
            other => panic!("expected a p99 breach, got {other:?}"),
        }
    }

    #[test]
    fn a_token_spend_anomaly_fails_the_canary_on_the_factor_test() {
        let baseline = healthy_baseline(); // ~1000 tokens/node
        let mut canary = WaveTelemetry::new();
        // 5000 tokens — 5x the baseline mean, past the 2x factor. A bad route or
        // a retry loop that suddenly spends far more.
        canary.insert("c1", node(1000, 5, &[95.0, 100.0, 110.0], 5000));
        match analyze(&enabled_policy(), &canary, &baseline) {
            Analysis::Fail(Breach::SpendAnomaly { factor, max_factor, .. }) => {
                assert!(factor > max_factor, "spend factor {factor} > {max_factor}");
            }
            other => panic!("expected a spend anomaly, got {other:?}"),
        }
    }

    #[test]
    fn a_spend_zscore_outlier_fails_even_when_the_factor_is_modest() {
        // A tight baseline (all ~1000) makes a small absolute increase a large
        // z-score. Set the factor threshold high so ONLY the z-score can trip.
        let policy = CanaryPolicy {
            enabled: true,
            max_spend_factor: 100.0, // effectively disable the factor test
            spend_zscore: 3.0,
            ..CanaryPolicy::default()
        };
        let mut baseline = WaveTelemetry::new();
        baseline.insert("b1", node(1000, 5, &[100.0], 1000));
        baseline.insert("b2", node(1000, 5, &[100.0], 1010));
        baseline.insert("b3", node(1000, 5, &[100.0], 990));
        let mut canary = WaveTelemetry::new();
        // 1200 is only 1.19x the mean (under 100x) but many std devs out.
        canary.insert("c1", node(1000, 5, &[100.0], 1200));
        match analyze(&policy, &canary, &baseline) {
            Analysis::Fail(Breach::SpendAnomaly { zscore: Some(z), .. }) => {
                assert!(z > 3.0, "z-score {z} crossed the threshold");
            }
            other => panic!("expected a z-score spend anomaly, got {other:?}"),
        }
    }

    #[test]
    fn a_disabled_metric_never_trips() {
        // Error rate off: a wildly elevated error rate must NOT fail the canary.
        let policy = CanaryPolicy {
            enabled: true,
            metrics: EnabledMetrics {
                error_rate: false,
                p99: true,
                spend_anomaly: true,
            },
            ..CanaryPolicy::default()
        };
        let baseline = healthy_baseline();
        let mut canary = WaveTelemetry::new();
        canary.insert("c1", node(1000, 900, &[95.0, 100.0, 110.0], 1000));
        assert_eq!(analyze(&policy, &canary, &baseline), Analysis::Pass);
    }

    #[test]
    fn a_canary_that_reported_nothing_is_no_data_not_a_silent_pass() {
        let baseline = healthy_baseline();
        let canary = WaveTelemetry::new(); // no nodes reported
        assert_eq!(analyze(&enabled_policy(), &canary, &baseline), Analysis::NoData);
    }

    // --- Policy parsing / composition ---------------------------------------

    #[test]
    fn an_absent_or_empty_policy_is_analysis_off() {
        assert_eq!(parse_canary_policy("").unwrap(), CanaryPolicy::default());
        assert!(!parse_canary_policy("").unwrap().enabled);
    }

    #[test]
    fn a_minimal_enabling_edit_flips_only_enabled() {
        let p = parse_canary_policy("enabled: true\n").unwrap();
        assert!(p.enabled);
        assert_eq!(p.window_secs, DEFAULT_WINDOW_SECS);
        assert_eq!(p.max_p99_factor, DEFAULT_MAX_P99_FACTOR);
    }

    #[test]
    fn a_full_policy_parses_thresholds_metrics_and_gates() {
        let yaml = "\
enabled: true
window_secs: 30
max_error_rate_increase: 0.02
max_p99_factor: 1.25
max_spend_factor: 1.5
spend_zscore: 2.5
metrics:
  error_rate: true
  p99: false
  spend_anomaly: true
manual_gate_after:
  - canary
  - eu
";
        let p = parse_canary_policy(yaml).unwrap();
        assert!(p.enabled);
        assert_eq!(p.window_secs, 30);
        assert_eq!(p.max_error_rate_increase, 0.02);
        assert_eq!(p.max_p99_factor, 1.25);
        assert!(!p.metrics.p99, "p99 disabled");
        assert!(p.gates_after("canary"));
        assert!(p.gates_after("eu"));
        assert!(!p.gates_after("us"));
    }

    #[test]
    fn a_nonsensical_threshold_is_rejected_at_parse() {
        assert!(parse_canary_policy("enabled: true\nmax_p99_factor: 0\n").is_err());
        assert!(parse_canary_policy("enabled: true\nmax_spend_factor: -1\n").is_err());
        assert!(parse_canary_policy("enabled: true\nmax_error_rate_increase: 2\n").is_err());
        assert!(parse_canary_policy("enabled: true\nspend_zscore: 0\n").is_err());
    }
}
