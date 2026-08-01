//! End-to-end PROOF for Phase 5 — config canaries and the Git-native judgment
//! gate — as a narrated in-process demo over the REAL gRPC `FleetService`.
//!
//! This is a dev example (not a shipped binary; the two-binary budget is
//! gatewayctl + gatewayd), run once to capture `../canary-demo.log`. It stands up
//! one control plane and three region-labeled gatewayd stand-in nodes over
//! loopback gRPC, on the SAME multi-wave substrate Phase 2 built, and drives
//! three scenarios that exercise the Phase-5 additions — analysis between waves,
//! auto-rollback, and the Git-native manual judgment gate — all from the fleet's
//! OWN telemetry (no new service, no new dependency):
//!
//!   (A) HEALTHY CANARY PASSES: a multi-wave rollout where wave 1 (canary) reads
//!       healthy telemetry, passes analysis, and the rollout advances through
//!       every wave.
//!   (B) TOKEN-SPEND ANOMALY AUTO-ROLLS-BACK: the canary wave suddenly spends far
//!       more per node than the baseline (a bad route / retry loop / wrong model)
//!       — the domain-aware signal nothing else has. Analysis FAILS, the rollout
//!       AUTO-ROLLS-BACK the canary and FREEZES the later waves, and the tripping
//!       metric + wave + reverted-to version are surfaced loudly.
//!   (C) MANUAL JUDGMENT GATE: a rollout paused at a Git-native judgment gate
//!       after the canary wave, held until the approval ARTIFACT is committed to
//!       the config repo (approvals/canary.approved), then proceeding.
//!
//! Telemetry is pre-seeded through the control plane's OWN telemetry/budget
//! sinks (the same sinks the live `Status`/`UsageReport` stream folds into), and
//! the analysis window is set to zero, so the analysis runs deterministically
//! over known telemetry without a live traffic generator — the analysis LOGIC is
//! identical to production; only the telemetry source is seeded.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use gateway_core::budget::CapId;
use gateway_proto::fleet::fleet_service_server::FleetServiceServer;
use gateway_proto::{server_message, Ack, ClientMessage, FleetServiceClient, Hello};
use gatewayctl::canary::CanaryPolicy;
use gatewayctl::canary_rollout::{AutoApprove, CanaryOutcome, GateSignal};
use gatewayctl::fleet::Fleet;
use gatewayctl::render::{read_gatewaysets, read_wave_plan, render_resolved};
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

fn region_label(region: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("region".to_string(), region.to_string())])
}

fn banner(s: &str) {
    println!("\n========================================================================");
    println!("{s}");
    println!("========================================================================");
}

/// A gate flipped in-process, standing in for the operator committing the
/// approvals/<wave>.approved artifact to the config repo (the SAME GateSignal
/// contract RepoGateSignal implements over a real config source).
struct FlagGate(std::sync::Mutex<Option<String>>);
impl FlagGate {
    fn new() -> Arc<FlagGate> {
        Arc::new(FlagGate(std::sync::Mutex::new(None)))
    }
    fn approve(&self, wave: &str) {
        *self.0.lock().unwrap() = Some(wave.to_string());
    }
}
impl GateSignal for FlagGate {
    fn approved(&self, wave: &str) -> bool {
        self.0.lock().unwrap().as_deref() == Some(wave)
    }
}

struct Node {
    out: mpsc::Sender<ClientMessage>,
    inbound: tonic::Streaming<gateway_proto::ServerMessage>,
}
impl Node {
    async fn join(addr: &str, node_id: &str, token: &str) -> Node {
        let mut client = FleetServiceClient::connect(addr.to_string()).await.unwrap();
        let (out, out_rx) = mpsc::channel::<ClientMessage>(16);
        out.send(ClientMessage::hello(Hello {
            node_id: node_id.to_string(),
            join_token: token.to_string(),
            labels: Default::default(),
            current_fleet_version: 0,
        }))
        .await
        .unwrap();
        let response = client.session(ReceiverStream::new(out_rx)).await.unwrap();
        Node { out, inbound: response.into_inner() }
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
    async fn ack(&self, v: u64, h: &str) {
        self.out
            .send(ClientMessage::ack(Ack { fleet_version: v, render_hash: h.to_string() }))
            .await
            .unwrap();
    }
    async fn bootstrap(&mut self) {
        let (v, h) = self.next_push(Duration::from_secs(3)).await.expect("bootstrap");
        self.ack(v, &h).await;
    }
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

async fn boot(policy: CanaryPolicy) -> (String, Arc<ControlPlane>) {
    let root = gatewayctl::render::testrepo::write("prod");
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
    (endpoint, cp)
}

fn apply_render(cp: &Arc<ControlPlane>, env: &str, policy: CanaryPolicy) {
    let root = gatewayctl::render::testrepo::write(env);
    std::fs::write(root.join("waves.yaml"), WAVES_YAML).unwrap();
    let resolved = DirectorySource::new(&root).resolve().unwrap();
    let rendered = render_resolved(&resolved).unwrap();
    let gs = read_gatewaysets(&resolved).unwrap();
    let plan = read_wave_plan(&resolved).unwrap();
    cp.fleet.set_canary_policy(policy);
    cp.fleet.set_applied_from_source(rendered, resolved, gs, plan);
}

fn seed_infra(cp: &Arc<ControlPlane>, id: &str, requests: u64, errors: u64, p99: f64) {
    cp.telemetry
        .set_window(id, NodeWindow { requests, errors, latencies_ms: vec![p99] });
}
fn seed_spend(cp: &Arc<ControlPlane>, id: &str, tokens: u64) {
    cp.budgets
        .report_spend(id, &CapId::new("team", "ml"), 10_000_000, tokens);
}
fn seed_healthy(cp: &Arc<ControlPlane>, id: &str) {
    seed_infra(cp, id, 1000, 5, 100.0);
    seed_spend(cp, id, 1000);
}

fn policy_on() -> CanaryPolicy {
    CanaryPolicy { enabled: true, window_secs: 0, ..CanaryPolicy::default() }
}

fn report(out: &CanaryOutcome) {
    println!("\n[outcome] {}", out.summary());
    for (i, step) in out.multi.steps.iter().enumerate() {
        let a = &out.analyses[i];
        println!(
            "  wave {:>8?}  state={:<40}  analysis={:?}",
            step.wave_name,
            format!("{:?}", step.state),
            a
        );
    }
    if !out.gates_released.is_empty() {
        println!("  gates released (Git-approved): {:?}", out.gates_released);
    }
}

#[tokio::main]
async fn main() {
    // -------------------------------------------------------------------- (A)
    banner("(A) HEALTHY CANARY PASSES ANALYSIS AND THE ROLLOUT ADVANCES");
    {
        let (addr, cp) = boot(policy_on()).await;
        let mut canary = Node::join(&addr, "n-canary", "tok-canary").await;
        let mut eu = Node::join(&addr, "n-eu", "tok-eu").await;
        canary.bootstrap().await;
        eu.bootstrap().await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        apply_render(&cp, "canary", policy_on());
        println!("config changed (env prod -> canary); canary analysis ON.");
        seed_healthy(&cp, "n-canary");
        seed_healthy(&cp, "n-eu");
        println!("telemetry: canary wave reads healthy (err~0.5%, p99~100ms, ~1000 tok/node),");
        println!("           baseline (eu, still on old version) reads the same.");

        let prior = cp.fleet.applied();
        let cp2 = cp.clone();
        let rollout =
            tokio::spawn(async move { cp2.roll_out_plan_canary("demo-A", &AutoApprove, Some(Duration::ZERO), Some(prior)).await });
        let (cv, ch) = canary.next_push(Duration::from_secs(3)).await.unwrap();
        println!("wave 1 (canary): pushed, node acks; opening analysis window...");
        canary.ack(cv, &ch).await;
        let (ev, eh) = eu.next_push(Duration::from_secs(3)).await.unwrap();
        println!("wave 1 PASSED analysis -> wave 2 (eu) pushed, node acks.");
        eu.ack(ev, &eh).await;
        let out = rollout.await.unwrap();
        report(&out);
        assert!(out.is_fully_applied());
    }

    // -------------------------------------------------------------------- (B)
    banner("(B) TOKEN-SPEND ANOMALY ON THE CANARY -> AUTO-ROLLBACK, LATER WAVES FROZEN");
    {
        let (addr, cp) = boot(policy_on()).await;
        let mut canary = Node::join(&addr, "n-canary", "tok-canary").await;
        let eu = Node::join(&addr, "n-eu", "tok-eu").await;
        let us = Node::join(&addr, "n-us", "tok-us").await;
        canary.bootstrap().await;
        let mut eu = eu;
        let mut us = us;
        eu.bootstrap().await;
        us.bootstrap().await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        apply_render(&cp, "canary", policy_on());
        println!("config changed; canary analysis ON.");
        seed_healthy(&cp, "n-eu");
        seed_healthy(&cp, "n-us");
        // The domain-aware signal: the canary's infra metrics are FINE, but it
        // suddenly spends 8x the baseline per node (a bad route / loop / wrong
        // model). Nothing but spend telemetry catches this.
        seed_infra(&cp, "n-canary", 1000, 5, 100.0);
        seed_spend(&cp, "n-canary", 8000);
        println!("telemetry: canary infra HEALTHY (err~0.5%, p99~100ms) BUT spends 8000 tok/node");
        println!("           vs ~1000 baseline -> an 8x token-spend anomaly (bad route/loop/model).");

        let prior = cp.fleet.applied();
        let cp2 = cp.clone();
        let rollout =
            tokio::spawn(async move { cp2.roll_out_plan_canary("demo-B", &AutoApprove, Some(Duration::ZERO), Some(prior)).await });
        let (cv, ch) = canary.next_push(Duration::from_secs(3)).await.unwrap();
        println!("wave 1 (canary): pushed, node acks; opening analysis window...");
        canary.ack(cv, &ch).await;
        canary.auto_ack(Duration::from_secs(3)); // ack the rollback re-push
        eu.auto_ack(Duration::from_secs(3));
        us.auto_ack(Duration::from_secs(3));
        let out = rollout.await.unwrap();
        report(&out);
        assert!(out.rolled_back());
        let rb = out.rollback.as_ref().unwrap();
        println!(
            "\n>>> AUTO-ROLLBACK: wave {:?} tripped [{}]; reverted, later waves FROZEN. Fleet did NOT advance.",
            rb.wave_name,
            rb.cause()
        );
    }

    // -------------------------------------------------------------------- (C)
    banner("(C) MANUAL JUDGMENT GATE: HELD ON A GIT ARTIFACT, RELEASED WHEN COMMITTED");
    {
        let policy = CanaryPolicy {
            enabled: true,
            window_secs: 0,
            manual_gate_after: vec!["canary".to_string()],
            ..CanaryPolicy::default()
        };
        let (addr, cp) = boot(policy.clone()).await;
        let mut canary = Node::join(&addr, "n-canary", "tok-canary").await;
        let mut eu = Node::join(&addr, "n-eu", "tok-eu").await;
        canary.bootstrap().await;
        eu.bootstrap().await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        apply_render(&cp, "canary", policy);
        println!("config changed; policy holds a MANUAL JUDGMENT GATE after the canary wave.");
        println!("the gate is Git-native: it is satisfied by committing approvals/canary.approved");
        println!("to the config repo (the wave-PR approval), NOT a pipeline click.");
        seed_healthy(&cp, "n-canary");
        seed_healthy(&cp, "n-eu");

        let gate = FlagGate::new();
        let gate2 = gate.clone();
        let prior = cp.fleet.applied();
        let cp2 = cp.clone();
        let rollout = tokio::spawn(async move {
            cp2.roll_out_plan_canary("demo-C", gate2.as_ref(), Some(Duration::ZERO), Some(prior)).await
        });
        let (cv, ch) = canary.next_push(Duration::from_secs(3)).await.unwrap();
        println!("wave 1 (canary): pushed, acks, PASSES analysis -> PAUSES at the judgment gate.");
        canary.ack(cv, &ch).await;
        let eu_early = eu.next_push(Duration::from_millis(800)).await;
        println!(
            "while unapproved, wave 2 (eu) is NOT pushed: {}",
            if eu_early.is_none() { "confirmed held" } else { "ERROR: released early" }
        );
        assert!(eu_early.is_none());
        println!("operator commits approvals/canary.approved to the config repo...");
        gate.approve("canary");
        let (ev, eh) = eu.next_push(Duration::from_secs(3)).await.expect("released");
        println!("gate RELEASED -> wave 2 (eu) pushed, node acks.");
        eu.ack(ev, &eh).await;
        let out = rollout.await.unwrap();
        report(&out);
        assert!(out.is_fully_applied());
        assert_eq!(out.gates_released, vec!["canary".to_string()]);
    }

    banner("PHASE 5 PROOF COMPLETE: analysis-passes-advances, spend-anomaly auto-rollback, Git-native gate.");
}
