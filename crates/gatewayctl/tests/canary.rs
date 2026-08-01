//! Integration tests for CONFIG CANARIES (Phase 5 — docs/04 Phase 5; docs/07
//! "the canary story is waves with analysis between them"; docs/00 "Kayenta-style
//! analysis + manual judgment gates as Git-native mechanisms, not a pipeline
//! engine").
//!
//! These drive the REAL gRPC `FleetService` over loopback with tonic clients
//! standing in for gatewayd nodes, region-labeled canary/eu/us, on the SAME
//! multi-wave substrate the Phase-2 tests exercise. On top of it they prove the
//! Phase-5 additions, all reusing waves.rs/rollout.rs and the fleet's own
//! telemetry — no new service, no new dependency:
//!
//! - **analysis-passes-advances**: a healthy canary wave passes analysis and the
//!   rollout advances through every wave.
//! - **each metric breach rolls back**: an elevated error rate, an elevated p99,
//!   and a token-spend anomaly each fail the canary and AUTO-ROLL-BACK.
//! - **spend is a per-WINDOW delta**: a fresh canary spiking in the window vs a
//!   long-running high-cumulative baseline rolls back — proving the analysis
//!   compares windowed spend, not lifetime cumulative (over a REAL non-zero
//!   window, so the snapshot/delta path is exercised, not just the zero-window one).
//! - **no telemetry is inconclusive**: a canary that reports nothing fails closed
//!   and surfaces "inconclusive: no telemetry", not a fabricated error-rate breach.
//! - **auto-rollback reverts + freezes**: the failing wave reverts to the prior
//!   commit and all LATER waves are frozen; the tripping metric is named.
//! - **the judgment gate holds then releases**: the rollout pauses at a gated
//!   wave until the Git-expressed approval artifact appears, then proceeds.
//! - **canary policy admission-checks**: a nonsensical `canary.yaml` is BLOCKED
//!   at admission like any config.
//!
//! Telemetry is PRE-SEEDED via the control plane's own telemetry/budget sinks
//! (the same sinks the live `Status`/`UsageReport` stream folds into); most tests
//! override the window to zero, and the spend-delta test uses a short real window.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use gateway_core::budget::CapId;
use gateway_proto::fleet::fleet_service_server::FleetServiceServer;
use gateway_proto::{server_message, Ack, ClientMessage, FleetServiceClient, Hello};
use gatewayctl::canary::CanaryPolicy;
use gatewayctl::fleet::Fleet;
use gatewayctl::render::{read_gatewaysets, read_wave_plan, render_resolved};
use gatewayctl::canary_rollout::{AutoApprove, CanaryOutcome, GateSignal, WaveAnalysis};
use gatewayctl::rollout::WaveStepState;
use gatewayctl::server::{ControlPlane, FleetSvc};
use gatewayctl::source::{ConfigSource, DirectorySource};
use gatewayctl::store::RuntimeStore;
use gatewayctl::telemetry::NodeWindow;
use gatewayctl::token::JoinTokens;

const WAVES_YAML: &str = "\
waves:
  - name: canary
    selector: { region: canary }
  - name: eu
    selector: { region: eu }
  - name: us
    selector: { region: us }
";

/// A running control plane with a 3-wave plan (canary -> eu -> us) and a per-
/// region token so joining nodes carry region labels.
struct Harness {
    addr: String,
    cp: Arc<ControlPlane>,
}

impl Harness {
    /// Boot serving `env` with the 3-wave plan and the given canary policy.
    async fn start(env: &str, policy: CanaryPolicy) -> Harness {
        let root = gatewayctl::render::testrepo::write(env);
        std::fs::write(root.join("waves.yaml"), WAVES_YAML).unwrap();
        let resolved = DirectorySource::new(&root).resolve().unwrap();
        let rendered = render_resolved(&resolved).unwrap();
        let gatewaysets = read_gatewaysets(&resolved).unwrap();
        let plan = read_wave_plan(&resolved).unwrap();

        let fleet = Arc::new(Fleet::from_source(rendered, resolved, gatewaysets, plan));
        fleet.set_canary_policy(policy);
        let store = Arc::new(RuntimeStore::new());
        let tokens = Arc::new(JoinTokens::new(300));
        tokens.mint("tok-canary", region_label("canary"));
        tokens.mint("tok-eu", region_label("eu"));
        tokens.mint("tok-us", region_label("us"));
        let cp = ControlPlane::new(fleet, store, tokens);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
        let svc = FleetSvc::new(cp.clone());
        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(FleetServiceServer::new(svc))
                .serve_with_incoming(incoming)
                .await
                .unwrap();
        });
        let endpoint = format!("http://{addr}");
        for _ in 0..50 {
            if FleetServiceClient::connect(endpoint.clone()).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Harness { addr: endpoint, cp }
    }

    /// Seed one node's observed infra telemetry (requests/errors/latency) — the
    /// same shape the `Status` heartbeat folds in.
    fn seed_infra(&self, node_id: &str, requests: u64, errors: u64, p99_ms: f64) {
        self.cp.telemetry.set_window(
            node_id,
            NodeWindow {
                requests,
                errors,
                latencies_ms: vec![p99_ms],
            },
        );
    }

    /// Seed one node's observed token spend — the same figure a `UsageReport`
    /// folds into the budget ledger.
    fn seed_spend(&self, node_id: &str, tokens: u64) {
        self.cp
            .budgets
            .report_spend(node_id, &CapId::new("team", "ml"), 10_000_000, tokens);
    }

    /// A healthy baseline reading for a node (low error rate, ~100ms, ~1000 tok).
    fn seed_healthy(&self, node_id: &str) {
        self.seed_infra(node_id, 1000, 5, 100.0);
        self.seed_spend(node_id, 1000);
    }
}

fn region_label(region: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("region".to_string(), region.to_string())])
}

/// An enabled canary policy with a zero analysis window (tests pre-seed the
/// telemetry, so no wait is needed) and the documented default thresholds.
fn enabled_policy() -> CanaryPolicy {
    CanaryPolicy {
        enabled: true,
        window_secs: 0,
        ..CanaryPolicy::default()
    }
}

/// One test node: an outbound channel + the inbound stream.
struct Node {
    out: mpsc::Sender<ClientMessage>,
    inbound: tonic::Streaming<gateway_proto::ServerMessage>,
}

impl Node {
    async fn join(addr: &str, node_id: &str, token: &str) -> Node {
        let mut client = FleetServiceClient::connect(addr.to_string())
            .await
            .expect("dial");
        let (out, out_rx) = mpsc::channel::<ClientMessage>(16);
        out.send(ClientMessage::hello(Hello {
            node_id: node_id.to_string(),
            join_token: token.to_string(),
            labels: Default::default(),
            current_fleet_version: 0,
        }))
        .await
        .unwrap();
        let response = client
            .session(ReceiverStream::new(out_rx))
            .await
            .expect("join");
        Node {
            out,
            inbound: response.into_inner(),
        }
    }

    async fn next_push(&mut self, timeout: Duration) -> Option<(u64, String)> {
        loop {
            let msg = match tokio::time::timeout(timeout, self.inbound.next()).await {
                Ok(Some(Ok(m))) => m,
                _ => return None,
            };
            if let Some(server_message::Kind::Push(snap)) = msg.kind {
                return Some((snap.fleet_version, snap.render_hash));
            }
        }
    }

    async fn ack(&self, version: u64, hash: &str) {
        self.out
            .send(ClientMessage::ack(Ack {
                fleet_version: version,
                render_hash: hash.to_string(),
            }))
            .await
            .unwrap();
    }

    async fn bootstrap(&mut self) {
        let (v, h) = self
            .next_push(Duration::from_secs(3))
            .await
            .expect("bootstrap push");
        self.ack(v, &h).await;
    }

    /// Auto-ack every push this node receives for `dur` (a node that keeps
    /// accepting whatever it is sent — used for baseline waves and rollback
    /// re-pushes that we do not want to gate the test on).
    fn auto_ack(mut self, dur: Duration) {
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + dur;
            loop {
                let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                if left.is_zero() {
                    return;
                }
                match self.next_push(left).await {
                    Some((v, h)) => self.ack(v, &h).await,
                    None => return,
                }
            }
        });
    }
}

/// Apply a new render (env change) so the rollout has something to roll out.
fn apply_new_render(cp: &Arc<ControlPlane>, env: &str, policy: CanaryPolicy) {
    let root = gatewayctl::render::testrepo::write(env);
    std::fs::write(root.join("waves.yaml"), WAVES_YAML).unwrap();
    let resolved = DirectorySource::new(&root).resolve().unwrap();
    let rendered = render_resolved(&resolved).unwrap();
    let gs = read_gatewaysets(&resolved).unwrap();
    let plan = read_wave_plan(&resolved).unwrap();
    cp.fleet.set_canary_policy(policy);
    cp.fleet.set_applied_from_source(rendered, resolved, gs, plan);
}

// --- analysis passes -> advances -------------------------------------------

#[tokio::test]
async fn a_healthy_canary_passes_analysis_and_the_rollout_advances() {
    let h = Harness::start("prod", enabled_policy()).await;
    let mut canary = Node::join(&h.addr, "n-canary", "tok-canary").await;
    let mut eu = Node::join(&h.addr, "n-eu", "tok-eu").await;
    canary.bootstrap().await;
    eu.bootstrap().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    apply_new_render(&h.cp, "canary", enabled_policy());
    // Both waves read healthy telemetry, so every analysis passes.
    h.seed_healthy("n-canary");
    h.seed_healthy("n-eu");

    let prior = h.cp.fleet.applied();
    let cp = h.cp.clone();
    let rollout = tokio::spawn(async move {
        cp.roll_out_plan_canary("test", &AutoApprove, Some(Duration::ZERO), Some(prior))
            .await
    });

    // canary is wave 1, eu wave 2; both ack, both pass analysis.
    let (cv, ch) = canary.next_push(Duration::from_secs(3)).await.unwrap();
    canary.ack(cv, &ch).await;
    let (ev, eh) = eu.next_push(Duration::from_secs(3)).await.unwrap();
    eu.ack(ev, &eh).await;

    let out: CanaryOutcome = rollout.await.unwrap();
    assert!(out.is_fully_applied(), "{}", out.summary());
    assert!(!out.rolled_back());
    assert_eq!(out.analyses[0], WaveAnalysis::Passed, "canary passed");
    assert_eq!(out.analyses[1], WaveAnalysis::Passed, "eu passed");
}

// --- each metric breach -> rollback ----------------------------------------

/// Drive a canary rollout where the canary wave (n-canary) reads a breaching
/// metric while the baseline (n-eu, later wave) reads healthy. Returns the
/// outcome after the canary acks and the rollback re-push is auto-acked.
async fn run_breaching_canary(seed_canary: impl FnOnce(&Harness)) -> CanaryOutcome {
    let h = Harness::start("prod", enabled_policy()).await;
    let mut canary = Node::join(&h.addr, "n-canary", "tok-canary").await;
    let eu = Node::join(&h.addr, "n-eu", "tok-eu").await;
    let mut us = Node::join(&h.addr, "n-us", "tok-us").await;
    canary.bootstrap().await;
    // eu and us just keep acking (baseline + frozen-wave nodes we don't gate on).
    let mut eu = eu;
    eu.bootstrap().await;
    us.bootstrap().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    apply_new_render(&h.cp, "canary", enabled_policy());
    // Baseline (eu, us) healthy; the canary seeded to breach.
    h.seed_healthy("n-eu");
    h.seed_healthy("n-us");
    seed_canary(&h);

    let prior = h.cp.fleet.applied();
    let cp = h.cp.clone();
    let rollout = tokio::spawn(async move {
        cp.roll_out_plan_canary("test", &AutoApprove, Some(Duration::ZERO), Some(prior))
            .await
    });

    // canary (wave 1) acks the new render; analysis then fails and rolls back.
    let (cv, ch) = canary.next_push(Duration::from_secs(3)).await.unwrap();
    canary.ack(cv, &ch).await;
    // The rollback re-pushes the prior render to n-canary; auto-ack it so the
    // rollback completes promptly.
    canary.auto_ack(Duration::from_secs(3));
    // eu/us must NEVER be pushed the target — they are the frozen later waves.
    eu.auto_ack(Duration::from_secs(3));
    us.auto_ack(Duration::from_secs(3));

    rollout.await.unwrap()
}

#[tokio::test]
async fn an_elevated_error_rate_on_the_canary_auto_rolls_back() {
    // 300/1000 = 30% error rate vs ~0.5% baseline — well past +5 points.
    let out = run_breaching_canary(|h| {
        h.seed_infra("n-canary", 1000, 300, 100.0);
        h.seed_spend("n-canary", 1000);
    })
    .await;
    assert!(out.rolled_back(), "{}", out.summary());
    let rb = out.rollback.as_ref().unwrap();
    assert_eq!(rb.wave_name, "canary");
    assert!(
        matches!(out.analyses[0], WaveAnalysis::Failed(_)),
        "canary analysis failed: {:?}",
        out.analyses[0]
    );
    assert!(rb.cause().contains("error-rate"), "{}", rb.cause());
}

#[tokio::test]
async fn an_elevated_p99_on_the_canary_auto_rolls_back() {
    // p99 ~900ms vs ~100ms baseline — past the 1.5x factor.
    let out = run_breaching_canary(|h| {
        h.seed_infra("n-canary", 1000, 5, 900.0);
        h.seed_spend("n-canary", 1000);
    })
    .await;
    assert!(out.rolled_back(), "{}", out.summary());
    let rb = out.rollback.as_ref().unwrap();
    assert!(rb.cause().contains("p99"), "{}", rb.cause());
}

#[tokio::test]
async fn a_token_spend_anomaly_on_the_canary_auto_rolls_back() {
    // 8000 tokens vs ~1000 baseline — 8x, past the 2x factor. Infra metrics are
    // healthy, so ONLY the domain-aware spend signal can trip this.
    let out = run_breaching_canary(|h| {
        h.seed_infra("n-canary", 1000, 5, 100.0);
        h.seed_spend("n-canary", 8000);
    })
    .await;
    assert!(out.rolled_back(), "{}", out.summary());
    let rb = out.rollback.as_ref().unwrap();
    assert!(
        rb.cause().contains("token-spend"),
        "{}",
        rb.cause()
    );
}

// --- the spend signal is a per-WINDOW delta, not lifetime cumulative --------

#[tokio::test]
async fn the_spend_anomaly_uses_per_window_delta_so_a_fresh_canary_vs_a_long_running_baseline_is_caught(
) {
    // The MEDIUM proven END TO END through a REAL (non-zero) analysis window, so
    // `open_spend_window` snapshots and `assemble_telemetry` reads the per-window
    // delta. Baseline (n-eu, a later wave) is LONG-RUNNING (900_000 tok lifetime);
    // the canary just joined (1_000 tok). DURING the window the canary spikes
    // (+6_000) while the baseline ticks along (+500).
    //   cumulative: baseline 900_500 vs canary 7_000 -> ~130x -> NO anomaly.
    //   windowed:   baseline 500 vs canary 6_000 -> 12x -> past 2x -> ROLLBACK.
    // The test passes ONLY because analysis compares the per-window delta; read
    // cumulative and the canary looks cheaper, the rollout advances, and
    // `out.rolled_back()` fails. That is the regression guard.
    let h = Harness::start("prod", enabled_policy()).await;
    let mut canary = Node::join(&h.addr, "n-canary", "tok-canary").await;
    let mut eu = Node::join(&h.addr, "n-eu", "tok-eu").await;
    let mut us = Node::join(&h.addr, "n-us", "tok-us").await;
    canary.bootstrap().await;
    eu.bootstrap().await;
    us.bootstrap().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    apply_new_render(&h.cp, "canary", enabled_policy());

    // Pre-window cumulative: the baseline has spent a LOT over its lifetime; the
    // canary almost nothing. Infra is healthy on both so ONLY spend can trip.
    h.seed_infra("n-canary", 1000, 5, 100.0);
    h.seed_spend("n-canary", 1_000);
    h.seed_infra("n-eu", 1000, 5, 100.0);
    h.seed_spend("n-eu", 900_000);
    h.seed_infra("n-us", 1000, 5, 100.0);
    h.seed_spend("n-us", 900_000);

    // A real (short) analysis window, NOT zero — this is what makes
    // `open_spend_window` snapshot the pre-window totals and the assembler read
    // the delta. The window is long enough to seed the deltas mid-flight below.
    let window = Duration::from_millis(600);

    let prior = h.cp.fleet.applied();
    let cp = h.cp.clone();
    let rollout = tokio::spawn(async move {
        cp.roll_out_plan_canary("test", &AutoApprove, Some(window), Some(prior))
            .await
    });

    // Wave 1 (canary) acks the target; the control plane then opens the analysis
    // window (snapshotting cumulative spend for canary + baseline) and sleeps.
    let (cv, ch) = canary.next_push(Duration::from_secs(3)).await.unwrap();
    canary.ack(cv, &ch).await;
    // The rollback re-push (once analysis fails) is auto-acked; eu/us are frozen.
    canary.auto_ack(Duration::from_secs(3));
    eu.auto_ack(Duration::from_secs(3));
    us.auto_ack(Duration::from_secs(3));

    // Let the window OPEN (open_spend_window runs right after the ack, before the
    // sleep), then seed the per-window deltas well inside the 600ms window.
    // `reset_many` cleared the canary infra at open, so re-seed it healthy too.
    tokio::time::sleep(Duration::from_millis(150)).await;
    // Canary spends 6_000 THIS window (a spike); cumulative 1_000 -> 7_000.
    h.seed_infra("n-canary", 1000, 5, 100.0);
    h.seed_spend("n-canary", 7_000);
    // Baseline spends its usual 500 this window; cumulative 900_000 -> 900_500.
    h.seed_spend("n-eu", 900_500);
    h.seed_spend("n-us", 900_500);

    let out: CanaryOutcome = rollout.await.unwrap();

    // Windowed delta caught the spike. A cumulative comparison would NOT have:
    // the baseline's lifetime 900_500 dwarfs the canary's 7_000.
    assert!(
        out.rolled_back(),
        "the per-window spend spike must roll back even though the canary's \
         CUMULATIVE spend is far below the long-running baseline's: {}",
        out.summary()
    );
    let rb = out.rollback.as_ref().unwrap();
    assert_eq!(rb.wave_name, "canary");
    assert!(
        rb.cause().contains("token-spend"),
        "the tripping metric is the token-spend anomaly, not infra: {}",
        rb.cause()
    );
    // Sanity that cumulative WOULD have masked it: the canary's lifetime total
    // is far below the baseline's, so a cumulative factor would be < 1.
    assert!(
        h.cp.budgets.node_total_spend("n-canary") < h.cp.budgets.node_total_spend("n-eu"),
        "precondition: canary lifetime spend < baseline lifetime spend, so only \
         the WINDOWED delta could have tripped this"
    );
}

// --- a canary that reports NO telemetry is honestly surfaced as inconclusive -

#[tokio::test]
async fn a_canary_with_no_telemetry_rolls_back_as_inconclusive_not_a_fabricated_error_rate() {
    // The second MEDIUM, proven END TO END: a canary that reports NO telemetry
    // is fail-closed rolled back (does not advance blind), and the surfaced cause
    // honestly says "inconclusive: no telemetry" instead of a fabricated all-zero
    // "error-rate breach: canary 0.0000 vs baseline 0.0000". Reuses the breaching-
    // canary harness with a no-op seed: the canary is left UN-seeded, so
    // has_samples() is false -> Analysis::NoData -> fail-closed rollback.
    let out = run_breaching_canary(|_h| { /* canary deliberately un-seeded */ }).await;

    assert!(out.rolled_back(), "a no-telemetry canary fails closed: {}", out.summary());
    // The verdict is NoData (already honest), and the surfaced cause/summary now
    // match it — no fabricated error-rate breach.
    assert_eq!(out.analyses[0], WaveAnalysis::NoData, "{:?}", out.analyses[0]);
    let rb = out.rollback.as_ref().unwrap();
    assert_eq!(rb.wave_name, "canary");
    assert!(rb.breach.is_none(), "a NoData rollback names no metric breach");
    let cause = rb.cause();
    assert!(cause.contains("inconclusive"), "{cause}");
    assert!(cause.contains("no telemetry"), "{cause}");
    assert!(
        !cause.contains("error-rate"),
        "the surfaced cause must NOT fabricate a zero-value error-rate breach: {cause}"
    );
    let summary = out.summary();
    assert!(summary.contains("inconclusive"), "{summary}");
    assert!(
        !summary.contains("error-rate"),
        "the surfaced summary must NOT falsely name an error-rate breach: {summary}"
    );

    // The later waves are still frozen and the canary reverted — fail-closed did
    // not silently advance past the unmeasurable canary.
    assert!(
        matches!(out.multi.steps[0].state, WaveStepState::Halted { .. }),
        "canary reverted: {:?}",
        out.multi.steps[0].state
    );
    assert!(
        matches!(out.multi.steps[1].state, WaveStepState::Frozen { .. }),
        "eu frozen: {:?}",
        out.multi.steps[1].state
    );
}

// --- auto-rollback reverts to prior version and freezes forward -------------

#[tokio::test]
async fn auto_rollback_reverts_the_failing_wave_and_freezes_later_waves() {
    let out = run_breaching_canary(|h| {
        h.seed_infra("n-canary", 1000, 500, 100.0); // 50% errors
        h.seed_spend("n-canary", 1000);
    })
    .await;

    assert!(out.rolled_back());
    // The failing wave (canary) is recorded HALTED (reverted), not advanced.
    assert!(
        matches!(out.multi.steps[0].state, WaveStepState::Halted { .. }),
        "canary reverted: {:?}",
        out.multi.steps[0].state
    );
    // eu and us (later waves) are FROZEN — never advanced past the failed canary.
    assert!(
        matches!(out.multi.steps[1].state, WaveStepState::Frozen { .. }),
        "eu frozen: {:?}",
        out.multi.steps[1].state
    );
    assert!(
        matches!(out.multi.steps[2].state, WaveStepState::Frozen { .. }),
        "us frozen: {:?}",
        out.multi.steps[2].state
    );
    // The surfaced summary names the tripping metric, the wave, and reverted-to.
    let s = out.summary();
    assert!(s.contains("ROLLED BACK"), "{s}");
    assert!(s.contains("canary"), "{s}");
    assert!(s.contains("reverted to"), "{s}");
}

// --- an EARLIER wave advances, a LATER wave rolls back ----------------------

#[tokio::test]
async fn an_earlier_passing_wave_stays_committed_while_a_later_wave_rolls_back() {
    // The mixed-state outcome the docs care about ("waves 1-2 on abc, wave 3
    // reverted"): wave 1 (canary) passes analysis and ADVANCES (set_wave_commit
    // to target), then wave 2 (eu) fails analysis and rolls back. The already-
    // advanced wave 1 must stay committed to the target (the rollback only
    // touches the failing wave), and wave 3 (us) must freeze.
    let h = Harness::start("prod", enabled_policy()).await;
    let mut canary = Node::join(&h.addr, "n-canary", "tok-canary").await;
    let mut eu = Node::join(&h.addr, "n-eu", "tok-eu").await;
    let mut us = Node::join(&h.addr, "n-us", "tok-us").await;
    canary.bootstrap().await;
    eu.bootstrap().await;
    us.bootstrap().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    apply_new_render(&h.cp, "canary", enabled_policy());
    // Wave 1 (canary) healthy -> passes and advances. Its baseline (eu+us) is
    // healthy at this point. Wave 2 (eu) breaches on error rate; its baseline
    // (us, the later wave) stays healthy.
    h.seed_healthy("n-canary");
    h.seed_infra("n-eu", 1000, 400, 100.0); // 40% error rate -> eu breaches
    h.seed_spend("n-eu", 1000);
    h.seed_healthy("n-us");

    let target = h.cp.fleet.applied().source_commit.clone();
    let prior = h.cp.fleet.applied();
    let cp = h.cp.clone();
    let rollout = tokio::spawn(async move {
        cp.roll_out_plan_canary("test", &AutoApprove, Some(Duration::ZERO), Some(prior))
            .await
    });

    // Wave 1: canary acks the target and passes analysis -> advances.
    let (cv, ch) = canary.next_push(Duration::from_secs(3)).await.unwrap();
    canary.ack(cv, &ch).await;
    // Wave 2: eu acks the target, then analysis fails -> rollback re-push to eu.
    let (ev, eh) = eu.next_push(Duration::from_secs(3)).await.unwrap();
    eu.ack(ev, &eh).await;
    eu.auto_ack(Duration::from_secs(3)); // ack the rollback re-push
    us.auto_ack(Duration::from_secs(3)); // us is frozen, never gets the target

    let out: CanaryOutcome = rollout.await.unwrap();

    assert!(out.rolled_back(), "{}", out.summary());
    let rb = out.rollback.as_ref().unwrap();
    assert_eq!(rb.wave_name, "eu", "the LATER wave rolled back, not the first");

    // Wave 1 (canary) ADVANCED and stays advanced — the rollback never touched it.
    match &out.multi.steps[0].state {
        WaveStepState::Advanced { commit } => {
            assert_eq!(*commit, target, "canary advanced to the target commit");
        }
        other => panic!("canary must be Advanced, got {other:?}"),
    }
    assert_eq!(out.analyses[0], WaveAnalysis::Passed, "canary passed analysis");

    // Wave 2 (eu) is Halted (reverted); wave 3 (us) is Frozen.
    assert!(
        matches!(out.multi.steps[1].state, WaveStepState::Halted { .. }),
        "eu reverted: {:?}",
        out.multi.steps[1].state
    );
    assert!(
        matches!(out.multi.steps[2].state, WaveStepState::Frozen { .. }),
        "us frozen: {:?}",
        out.multi.steps[2].state
    );

    // The fleet's committed wave state proves it: canary is on the target commit,
    // eu is NOT (reverted), so an operator reads "wave 1 on abc, wave 2 reverted".
    let commits = h.cp.fleet.wave_commits();
    assert_eq!(
        commits.get("canary"),
        Some(&target),
        "the advanced earlier wave stays committed to the target: {commits:?}"
    );
    assert_ne!(
        commits.get("eu"),
        Some(&target),
        "the failing later wave did NOT stay on the target: {commits:?}"
    );
}

// --- the Git-native judgment gate holds then releases ----------------------

/// A gate whose approval is flipped by the test — stands in for the operator
/// committing the `approvals/<wave>.approved` artifact to the config repo. This
/// is the SAME `GateSignal` contract `RepoGateSignal` implements over a real
/// config source; here the "commit" is `approve()`.
struct FlagGate {
    approved_wave: Mutex<Option<String>>,
}

impl FlagGate {
    fn new() -> Arc<FlagGate> {
        Arc::new(FlagGate {
            approved_wave: Mutex::new(None),
        })
    }
    fn approve(&self, wave: &str) {
        *self.approved_wave.lock().unwrap() = Some(wave.to_string());
    }
}

impl GateSignal for FlagGate {
    fn approved(&self, wave_name: &str) -> bool {
        self.approved_wave
            .lock()
            .unwrap()
            .as_deref()
            .is_some_and(|w| w == wave_name)
    }
}

#[tokio::test]
async fn the_judgment_gate_holds_the_rollout_until_the_git_approval_arrives() {
    // Policy: gate AFTER the canary wave. The rollout must pause there until the
    // Git-expressed approval appears, then proceed to eu.
    let policy = CanaryPolicy {
        enabled: true,
        window_secs: 0,
        manual_gate_after: vec!["canary".to_string()],
        ..CanaryPolicy::default()
    };
    let h = Harness::start("prod", policy.clone()).await;
    let mut canary = Node::join(&h.addr, "n-canary", "tok-canary").await;
    let mut eu = Node::join(&h.addr, "n-eu", "tok-eu").await;
    canary.bootstrap().await;
    eu.bootstrap().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    apply_new_render(&h.cp, "canary", policy);
    h.seed_healthy("n-canary");
    h.seed_healthy("n-eu");

    let gate = FlagGate::new();
    let gate_for_rollout = gate.clone();
    let prior = h.cp.fleet.applied();
    let cp = h.cp.clone();
    let rollout = tokio::spawn(async move {
        cp.roll_out_plan_canary(
            "test",
            gate_for_rollout.as_ref(),
            Some(Duration::ZERO),
            Some(prior),
        )
        .await
    });

    // canary (wave 1) acks and passes analysis, then the rollout PAUSES at the
    // gate. eu (wave 2) must NOT be pushed while the gate is unapproved.
    let (cv, ch) = canary.next_push(Duration::from_secs(3)).await.unwrap();
    canary.ack(cv, &ch).await;
    let eu_early = eu.next_push(Duration::from_millis(800)).await;
    assert!(
        eu_early.is_none(),
        "eu must not be pushed while the judgment gate is unapproved"
    );

    // The operator commits the approval artifact -> the gate releases.
    gate.approve("canary");
    let (ev, eh) = eu
        .next_push(Duration::from_secs(3))
        .await
        .expect("eu pushed once the gate is approved");
    eu.ack(ev, &eh).await;

    let out: CanaryOutcome = rollout.await.unwrap();
    assert!(out.is_fully_applied(), "{}", out.summary());
    assert_eq!(out.gates_released, vec!["canary".to_string()]);
}

// --- canary policy composes / admission-checks like config -----------------

#[tokio::test]
async fn a_nonsensical_canary_policy_is_blocked_at_admission() {
    use gatewayctl::admission::AdmissionPolicy;

    let root = gatewayctl::render::testrepo::write("prod");
    // A canary.yaml with a non-positive factor is nonsensical config.
    std::fs::write(
        root.join("canary.yaml"),
        "enabled: true\nmax_p99_factor: 0\n",
    )
    .unwrap();
    let source = DirectorySource::new(&root);
    let verdict = AdmissionPolicy::new().admit_source(&source).unwrap();
    assert!(
        !verdict.is_admitted(),
        "a nonsensical canary policy must be blocked at admission"
    );
    assert!(
        verdict.failures().iter().any(|f| f.rule == "canary-policy"),
        "the block is attributed to the canary policy: {:?}",
        verdict.failures()
    );
}

#[tokio::test]
async fn a_valid_canary_policy_admits_and_composes() {
    use gatewayctl::admission::AdmissionPolicy;

    let root = gatewayctl::render::testrepo::write("prod");
    std::fs::write(
        root.join("canary.yaml"),
        "\
enabled: true
window_secs: 30
max_error_rate_increase: 0.03
max_p99_factor: 1.4
max_spend_factor: 2.0
manual_gate_after:
  - canary
",
    )
    .unwrap();
    let source = DirectorySource::new(&root);
    // Admits like any config.
    let verdict = AdmissionPolicy::new().admit_source(&source).unwrap();
    assert!(verdict.is_admitted(), "{:?}", verdict.failures());
    // And composes onto a fleet: the policy round-trips through the render read.
    let resolved = source.resolve().unwrap();
    let policy = gatewayctl::render::read_canary_policy(&resolved).unwrap();
    assert!(policy.enabled);
    assert_eq!(policy.window_secs, 30);
    assert!(policy.gates_after("canary"));
}
