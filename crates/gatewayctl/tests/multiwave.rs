//! Integration tests for MULTI-WAVE rollout grouped by failure domain (Phase 2,
//! milestone 3 — docs/07-control-plane.md, "Partial application: all-or-nothing
//! waves, chosen").
//!
//! These drive the REAL gRPC `FleetService` over a loopback socket with tonic
//! clients standing in for gatewayd nodes, each labeled by region (canary/eu/us)
//! via the join token it presents. A three-wave plan (`waves.yaml`) orders the
//! rollout canary -> eu -> us. The tests prove the load-bearing wave semantics:
//!
//! - **Ordering**: a later wave is not pushed until every node in the earlier
//!   wave has acked. We gate wave 1's ack behind a barrier and assert wave 2's
//!   node has NOT been pushed yet.
//! - **Halt freezes later, keeps earlier**: a NACK in wave 2 halts wave 2 and
//!   FREEZES wave 3 (never pushed), while wave 1 stays advanced — the surfaced,
//!   per-wave committed state ("waves [canary] advanced; halted at eu; [us]
//!   frozen"), never "some on new, some on old, shrug".
//! - **Assignment by label**: a node lands in the first wave its region matches.
//!
//! The node-to-wave assignment PURE logic, selector matching, GatewaySet stamp +
//! determinism, and GatewaySet admission live as unit tests in `waves.rs`,
//! `gatewayset.rs`, `render.rs`, and `admission.rs`. The reconciler-does-not-
//! fight-a-pending-later-wave proof lives in `tests/reconcile.rs`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use gateway_proto::fleet::fleet_service_server::FleetServiceServer;
use gateway_proto::{server_message, Ack, ClientMessage, FleetServiceClient, Hello, Nack};
use gatewayctl::fleet::Fleet;
use gatewayctl::rollout::{MultiWaveOutcome, WaveStepState};
use gatewayctl::render::{read_gatewaysets, read_wave_plan, render_resolved};
use gatewayctl::server::{ControlPlane, FleetSvc};
use gatewayctl::source::{ConfigSource, DirectorySource};
use gatewayctl::store::RuntimeStore;
use gatewayctl::token::JoinTokens;

/// A running control plane whose fleet was built from a source with a 3-wave
/// plan, plus a token per region so joining nodes carry region labels.
struct Harness {
    addr: String,
    cp: Arc<ControlPlane>,
}

impl Harness {
    /// Like [`start`], but the served repo ALSO carries a `gatewaysets.yaml` that
    /// stamps `tier: gold` onto every `region: eu` node — used to prove a joining
    /// eu node gets the STAMPED render on its very first (bootstrap) push.
    async fn start_with_gatewayset(env: &str) -> Harness {
        let root = gatewayctl::render::testrepo::write(env);
        std::fs::write(
            root.join("waves.yaml"),
            "waves:\n  - name: eu\n    selector: { region: eu }\n  - name: us\n    selector: { region: us }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("gatewaysets.yaml"),
            "\
gatewaysets:
  - name: eu-gold-tier
    selector: { region: eu }
    overlay:
      fleet:
        attribution:
          pinned: { tier: gold }
",
        )
        .unwrap();
        Self::boot(&root).await
    }

    /// Boot a control plane serving `env`, with a `waves.yaml` ordering
    /// canary -> eu -> us, and mint one token per region carrying that region as
    /// the node's label (labels flow from the token at join, per `server.rs`).
    async fn start(env: &str) -> Harness {
        let root = gatewayctl::render::testrepo::write(env);
        std::fs::write(
            root.join("waves.yaml"),
            "\
waves:
  - name: canary
    selector: { region: canary }
  - name: eu
    selector: { region: eu }
  - name: us
    selector: { region: us }
",
        )
        .unwrap();
        Self::boot(&root).await
    }

    /// Resolve + render the repo at `root`, build a `from_source` fleet from its
    /// waves + GatewaySets, mint a region token per region, and serve it.
    async fn boot(root: &std::path::Path) -> Harness {
        let resolved = DirectorySource::new(root).resolve().unwrap();
        let rendered = render_resolved(&resolved).unwrap();
        let gatewaysets = read_gatewaysets(&resolved).unwrap();
        let plan = read_wave_plan(&resolved).unwrap();

        let fleet = Arc::new(Fleet::from_source(rendered, resolved, gatewaysets, plan));
        let store = Arc::new(RuntimeStore::new());
        let tokens = Arc::new(JoinTokens::new(300));
        tokens.mint("tok-canary", region_label("canary"));
        tokens.mint("tok-eu", region_label("eu"));
        tokens.mint("tok-eu-2", region_label("eu")); // a 2nd eu node (single-use tokens)
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
}

fn region_label(region: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("region".to_string(), region.to_string());
    m
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

    /// Await the next push within `timeout`, returning (fleet_version, hash).
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

    async fn nack(&self, version: u64, hash: &str, reason: &str) {
        self.out
            .send(ClientMessage::nack(Nack {
                fleet_version: version,
                render_hash: hash.to_string(),
                reason: reason.to_string(),
            }))
            .await
            .unwrap();
    }

    /// Drain and ack the bootstrap push so the node is registered.
    async fn bootstrap(&mut self) {
        let (v, h) = self
            .next_push(Duration::from_secs(3))
            .await
            .expect("bootstrap push");
        self.ack(v, &h).await;
    }
}

/// A later wave is NOT pushed until every node in the earlier wave has acked.
/// We hold wave 1's (canary) ack, then assert the wave-2 (eu) node receives no
/// push while wave 1 is still outstanding; once canary acks, eu is pushed.
#[tokio::test]
async fn a_later_wave_is_not_pushed_until_the_earlier_wave_fully_acks() {
    let h = Harness::start("prod").await;
    let mut canary = Node::join(&h.addr, "n-canary", "tok-canary").await;
    let mut eu = Node::join(&h.addr, "n-eu", "tok-eu").await;
    canary.bootstrap().await;
    eu.bootstrap().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Apply a new render and start the multi-wave rollout.
    let root2 = gatewayctl::render::testrepo::write("canary");
    std::fs::write(
        root2.join("waves.yaml"),
        "waves:\n  - name: canary\n    selector: { region: canary }\n  - name: eu\n    selector: { region: eu }\n  - name: us\n    selector: { region: us }\n",
    )
    .unwrap();
    let resolved2 = DirectorySource::new(&root2).resolve().unwrap();
    let rendered2 = render_resolved(&resolved2).unwrap();
    let gs2 = read_gatewaysets(&resolved2).unwrap();
    let plan2 = read_wave_plan(&resolved2).unwrap();
    h.cp
        .fleet
        .set_applied_from_source(rendered2, resolved2, gs2, plan2);

    let cp = h.cp.clone();
    let rollout = tokio::spawn(async move { cp.roll_out_plan("test").await });

    // Wave 1 (canary) is pushed. Do NOT ack yet.
    let (cv, ch) = canary
        .next_push(Duration::from_secs(3))
        .await
        .expect("canary pushed in wave 1");

    // Wave 2 (eu) must NOT have been pushed while canary is unacked.
    let eu_early = eu.next_push(Duration::from_millis(600)).await;
    assert!(
        eu_early.is_none(),
        "eu (wave 2) must not be pushed until canary (wave 1) acks"
    );

    // Now canary acks -> wave 1 commits -> wave 2 proceeds and eu IS pushed.
    canary.ack(cv, &ch).await;
    let (ev, eh) = eu
        .next_push(Duration::from_secs(3))
        .await
        .expect("eu pushed once canary acked");
    eu.ack(ev, &eh).await;

    let outcome: MultiWaveOutcome = rollout.await.unwrap();
    assert!(outcome.is_fully_applied(), "{}", outcome.summary());
    // canary and eu advanced; us had no connected node (Empty), not a halt.
    assert!(matches!(
        outcome.steps[0].state,
        WaveStepState::Advanced { .. }
    ));
    assert!(matches!(
        outcome.steps[1].state,
        WaveStepState::Advanced { .. }
    ));
}

/// A NACK in wave 2 halts wave 2 and FREEZES wave 3, while wave 1 stays
/// advanced — the per-wave committed state is surfaced, never silent.
#[tokio::test]
async fn a_nack_in_wave_2_halts_it_and_freezes_wave_3_while_wave_1_stays_advanced() {
    let h = Harness::start("prod").await;
    let mut canary = Node::join(&h.addr, "n-canary", "tok-canary").await;
    let mut eu = Node::join(&h.addr, "n-eu", "tok-eu").await;
    let mut us = Node::join(&h.addr, "n-us", "tok-us").await;
    canary.bootstrap().await;
    eu.bootstrap().await;
    us.bootstrap().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Apply a new render.
    let root2 = gatewayctl::render::testrepo::write("canary");
    std::fs::write(
        root2.join("waves.yaml"),
        "waves:\n  - name: canary\n    selector: { region: canary }\n  - name: eu\n    selector: { region: eu }\n  - name: us\n    selector: { region: us }\n",
    )
    .unwrap();
    let resolved2 = DirectorySource::new(&root2).resolve().unwrap();
    let rendered2 = render_resolved(&resolved2).unwrap();
    let gs2 = read_gatewaysets(&resolved2).unwrap();
    let plan2 = read_wave_plan(&resolved2).unwrap();
    h.cp
        .fleet
        .set_applied_from_source(rendered2, resolved2, gs2, plan2);

    let cp = h.cp.clone();
    let rollout = tokio::spawn(async move { cp.roll_out_plan("test").await });

    // Wave 1 (canary) acks -> advances.
    let (cv, ch) = canary.next_push(Duration::from_secs(3)).await.unwrap();
    canary.ack(cv, &ch).await;

    // Wave 2 (eu) NACKs -> the wave halts.
    let (ev, eh) = eu.next_push(Duration::from_secs(3)).await.unwrap();
    eu.nack(ev, &eh, "unknown provider foo").await;

    // Wave 3 (us) must NEVER be pushed — it is frozen behind the halt.
    let us_push = us.next_push(Duration::from_millis(800)).await;
    assert!(us_push.is_none(), "us (wave 3) must be frozen, never pushed");

    let outcome: MultiWaveOutcome = rollout.await.unwrap();
    assert!(!outcome.is_fully_applied(), "the rollout halted");
    assert_eq!(outcome.halted_at, Some(1), "halted at wave index 1 (eu)");

    // Per-wave committed state, surfaced: canary advanced, eu halted, us frozen.
    assert!(
        matches!(outcome.steps[0].state, WaveStepState::Advanced { .. }),
        "canary stays advanced: {:?}",
        outcome.steps[0].state
    );
    assert!(
        matches!(outcome.steps[1].state, WaveStepState::Halted { .. }),
        "eu halted: {:?}",
        outcome.steps[1].state
    );
    assert!(
        matches!(outcome.steps[2].state, WaveStepState::Frozen { .. }),
        "us frozen behind the halt: {:?}",
        outcome.steps[2].state
    );

    // The surfaced summary names the mixed state (never "shrug").
    let summary = outcome.summary();
    assert!(summary.contains("canary"), "{summary}");
    assert!(summary.contains("halted at"), "{summary}");
}

/// A node lands in the FIRST wave its region matches: the wave that pushes a
/// given node is the one named for its region.
#[tokio::test]
async fn a_node_is_pushed_by_the_wave_its_region_selects() {
    let h = Harness::start("prod").await;
    let mut eu = Node::join(&h.addr, "n-eu", "tok-eu").await;
    let mut us = Node::join(&h.addr, "n-us", "tok-us").await;
    eu.bootstrap().await;
    us.bootstrap().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // No canary node is connected, so wave 1 (canary) is Empty and the rollout
    // proceeds straight to eu then us. Both should advance.
    let root2 = gatewayctl::render::testrepo::write("canary");
    std::fs::write(
        root2.join("waves.yaml"),
        "waves:\n  - name: canary\n    selector: { region: canary }\n  - name: eu\n    selector: { region: eu }\n  - name: us\n    selector: { region: us }\n",
    )
    .unwrap();
    let resolved2 = DirectorySource::new(&root2).resolve().unwrap();
    let rendered2 = render_resolved(&resolved2).unwrap();
    let gs2 = read_gatewaysets(&resolved2).unwrap();
    let plan2 = read_wave_plan(&resolved2).unwrap();
    h.cp
        .fleet
        .set_applied_from_source(rendered2, resolved2, gs2, plan2);

    let cp = h.cp.clone();
    let rollout = tokio::spawn(async move { cp.roll_out_plan("test").await });

    // eu is pushed in wave 2 (canary is empty), us in wave 3.
    let (ev, eh) = eu.next_push(Duration::from_secs(3)).await.expect("eu push");
    eu.ack(ev, &eh).await;
    let (uv, uh) = us.next_push(Duration::from_secs(3)).await.expect("us push");
    us.ack(uv, &uh).await;

    let outcome: MultiWaveOutcome = rollout.await.unwrap();
    assert!(outcome.is_fully_applied(), "{}", outcome.summary());
    assert!(
        matches!(outcome.steps[0].state, WaveStepState::Empty),
        "canary wave had no connected node"
    );
    assert!(matches!(
        outcome.steps[1].state,
        WaveStepState::Advanced { .. }
    ));
    assert!(matches!(
        outcome.steps[2].state,
        WaveStepState::Advanced { .. }
    ));
}

/// A GatewaySet-matching node gets the STAMPED render on its very first
/// (bootstrap) push — a newly-joined eu node picks up `tier: gold` on render,
/// and a non-matching us node does not, so they bind DIFFERENT hashes from the
/// same repo purely by their region label (docs/02 GatewaySets).
#[tokio::test]
async fn a_joining_matching_node_gets_the_stamped_render_on_bootstrap() {
    let h = Harness::start_with_gatewayset("prod").await;
    let mut eu = Node::join(&h.addr, "n-eu", "tok-eu").await;
    let mut us = Node::join(&h.addr, "n-us", "tok-us").await;

    let (_ev, eu_hash) = eu
        .next_push(Duration::from_secs(3))
        .await
        .expect("eu bootstrap push");
    let (_uv, us_hash) = us
        .next_push(Duration::from_secs(3))
        .await
        .expect("us bootstrap push");

    // The eu node's FIRST render already carries the stamp, so its hash differs
    // from the unstamped us node's — no per-node file, no post-join heal.
    assert_ne!(
        eu_hash, us_hash,
        "the eu node's bootstrap render is GatewaySet-stamped (distinct hash)"
    );

    // A SECOND eu node joining later gets the identical eu-stamped render.
    let mut eu2 = Node::join(&h.addr, "n-eu-2", "tok-eu-2").await;
    let (_e2v, eu2_hash) = eu2
        .next_push(Duration::from_secs(3))
        .await
        .expect("eu-2 bootstrap push");
    assert_eq!(
        eu2_hash, eu_hash,
        "a newly-joined matching node picks up the same stamped render"
    );
}

/// A GatewaySet-ONLY edit — no change to any base fragment — moves the
/// per-node stamped render WITHOUT moving the fleet-wide (unstamped) render_hash.
/// The reload gate must therefore key on `source_commit` (a content id over ALL
/// files), not the base render_hash; otherwise the ordered multi-wave rollout is
/// silently skipped and propagation falls back to the unordered reconciler,
/// bypassing this milestone's ordered-wave substrate. This drives the exact gate
/// (`set_applied_from_source`) that `reload_and_roll` checks and proves the
/// stamp reaches the matching node through the WAVE substrate.
#[tokio::test]
async fn a_gatewayset_only_edit_triggers_the_ordered_wave_rollout() {
    // Boot a plain repo (no gatewaysets.yaml), waves canary -> eu -> us.
    let h = Harness::start("prod").await;
    let mut eu = Node::join(&h.addr, "n-eu", "tok-eu").await;
    let mut us = Node::join(&h.addr, "n-us", "tok-us").await;
    let (_ev0, eu_hash0) = eu.next_push(Duration::from_secs(3)).await.expect("eu boot");
    let (_uv0, us_hash0) = us.next_push(Duration::from_secs(3)).await.expect("us boot");
    // No stamp yet: eu and us bind the same unstamped render.
    assert_eq!(eu_hash0, us_hash0, "no gatewayset yet => identical renders");
    tokio::time::sleep(Duration::from_millis(100)).await;

    let base_hash_before = h.cp.fleet.applied().render_hash.clone();

    // GatewaySet-ONLY edit: same env=prod (base fragments byte-identical, so the
    // fleet-wide render_hash does NOT move), but ADD a gatewaysets.yaml stamping
    // eu. The directory source_commit (content id over all files) DOES move.
    let root2 = gatewayctl::render::testrepo::write("prod");
    std::fs::write(
        root2.join("waves.yaml"),
        "waves:\n  - name: canary\n    selector: { region: canary }\n  - name: eu\n    selector: { region: eu }\n  - name: us\n    selector: { region: us }\n",
    )
    .unwrap();
    std::fs::write(
        root2.join("gatewaysets.yaml"),
        "\
gatewaysets:
  - name: eu-gold-tier
    selector: { region: eu }
    overlay:
      fleet:
        attribution:
          pinned: { tier: gold }
",
    )
    .unwrap();
    let resolved2 = DirectorySource::new(&root2).resolve().unwrap();
    let rendered2 = render_resolved(&resolved2).unwrap();
    let gs2 = read_gatewaysets(&resolved2).unwrap();
    let plan2 = read_wave_plan(&resolved2).unwrap();

    // The base (unstamped) render_hash is UNCHANGED by a GatewaySet-only edit...
    assert_eq!(
        rendered2.render_hash, base_hash_before,
        "GatewaySet-only edit must not move the fleet-wide unstamped render_hash"
    );
    // ...yet `set_applied_from_source` must still report changed=true, because the
    // resolved config (source_commit) moved. This is the gate reload_and_roll uses
    // to decide whether to run roll_out_plan.
    let changed = h
        .cp
        .fleet
        .set_applied_from_source(rendered2, resolved2, gs2, plan2);
    assert!(
        changed,
        "a GatewaySet-only edit must trigger the ordered rollout (source_commit moved)"
    );

    // Drive the ordered multi-wave rollout the gate authorizes. The eu node is in
    // wave 2; canary wave is empty. eu must be re-pushed with the STAMPED hash
    // through the wave substrate (not left to an unordered reconciler heal).
    let cp = h.cp.clone();
    let rollout = tokio::spawn(async move { cp.roll_out_plan("gwset-edit").await });

    let (ev, eh) = eu
        .next_push(Duration::from_secs(3))
        .await
        .expect("eu re-pushed by the wave rollout after the gatewayset edit");
    assert_ne!(
        eh, eu_hash0,
        "eu's post-edit render carries the tier: gold stamp (distinct hash)"
    );
    eu.ack(ev, &eh).await;

    let (uv, uh) = us
        .next_push(Duration::from_secs(3))
        .await
        .expect("us re-pushed in wave 3");
    // us does not match the selector, so its render is unchanged; it re-acks anyway.
    us.ack(uv, &uh).await;

    let outcome: MultiWaveOutcome = rollout.await.unwrap();
    assert!(outcome.is_fully_applied(), "{}", outcome.summary());
    assert!(
        matches!(outcome.steps[0].state, WaveStepState::Empty),
        "canary wave has no connected node"
    );
    assert!(
        matches!(outcome.steps[1].state, WaveStepState::Advanced { .. }),
        "eu wave advanced through the ordered substrate"
    );
    assert!(matches!(
        outcome.steps[2].state,
        WaveStepState::Advanced { .. }
    ));
}
