//! Integration tests for the control plane's fleet distribution (Phase 2, M1).
//!
//! These drive the REAL gRPC `FleetService` over a loopback socket with tonic
//! clients that stand in for gatewayd nodes, so the wave sequencing, join-token
//! auth, and Ack/Nack routing are exercised end to end — not mocked.
//!
//! Covered (the task's proof list, server side):
//! - join-token auth: a bad token is rejected, a good one joins;
//! - two-node fan-out: both nodes receive the initial snapshot;
//! - a fully-acked wave commits and advances the fleet version;
//! - Nack-keeps-old: a node that NACKs the pushed render halts the wave and the
//!   fleet's committed version does not advance.
//!
//! The snapshot-rendering-determinism and proto-round-trip proofs live as unit
//! tests in `render.rs` and `gateway-proto` respectively.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use gateway_proto::fleet::fleet_service_server::FleetServiceServer;
use gateway_proto::{server_message, Ack, ClientMessage, FleetServiceClient, Hello, Nack};
use gatewayctl::fleet::Fleet;
use gatewayctl::render::{render_repo, testrepo};
use gatewayctl::server::{ControlPlane, FleetSvc};
use gatewayctl::store::RuntimeStore;
use gatewayctl::token::JoinTokens;

/// A running control plane on a loopback port, with the tokens it minted.
struct Harness {
    addr: String,
    cp: Arc<ControlPlane>,
}

impl Harness {
    /// Boot a control plane serving the given repo `env`, minting two join
    /// tokens ("tok-a", "tok-b") for two nodes.
    async fn start(env: &str) -> Harness {
        let rendered = render_repo(&testrepo::write(env)).unwrap();
        let fleet = Arc::new(Fleet::new(rendered));
        let store = Arc::new(RuntimeStore::new());
        let tokens = Arc::new(JoinTokens::new(300));
        tokens.mint("tok-a", BTreeMap::new());
        tokens.mint("tok-b", BTreeMap::new());
        let cp = ControlPlane::new(fleet, store, tokens);

        // Bind an ephemeral port, then serve on it.
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

        // Wait until the port accepts.
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

/// One test node: an outbound channel + the inbound stream. Sends its own
/// Ack/Nack in response to pushes according to `nack`.
struct Node {
    out: mpsc::Sender<ClientMessage>,
    inbound: tonic::Streaming<gateway_proto::ServerMessage>,
}

impl Node {
    async fn join(addr: &str, node_id: &str, token: &str) -> Result<Node, tonic::Status> {
        Self::join_at_version(addr, node_id, token, 0).await
    }

    /// Join (or REJOIN, on the established identity) presenting
    /// `current_fleet_version` — a reconnecting node advertises the version it
    /// last bound so the control plane can skip a redelivery.
    async fn join_at_version(
        addr: &str,
        node_id: &str,
        token: &str,
        current_fleet_version: u64,
    ) -> Result<Node, tonic::Status> {
        let mut client = FleetServiceClient::connect(addr.to_string())
            .await
            .expect("dial");
        let (out, out_rx) = mpsc::channel::<ClientMessage>(16);
        out.send(ClientMessage::hello(Hello {
            node_id: node_id.to_string(),
            join_token: token.to_string(),
            labels: Default::default(),
            current_fleet_version,
        }))
        .await
        .unwrap();
        let response = client.session(ReceiverStream::new(out_rx)).await?;
        Ok(Node {
            out,
            inbound: response.into_inner(),
        })
    }

    /// Await the next push and return (fleet_version, render_hash).
    async fn next_push(&mut self) -> (u64, String) {
        let (v, hash, _bytes) = self.next_push_full().await;
        (v, hash)
    }

    /// Await the next push and return (fleet_version, advertised render_hash,
    /// config bytes) — the bytes let a test compute the render's TRUE hash and
    /// mimic the real node's recompute-and-echo behavior.
    async fn next_push_full(&mut self) -> (u64, String, Vec<u8>) {
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(3), self.inbound.next())
                .await
                .expect("push within timeout")
                .expect("stream open")
                .expect("no stream error");
            if let Some(server_message::Kind::Push(snap)) = msg.kind {
                return (snap.fleet_version, snap.render_hash, snap.config);
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
}

#[tokio::test]
async fn bad_join_token_is_rejected_and_good_one_joins() {
    let h = Harness::start("prod").await;

    // A wrong token: the stream is refused with Unauthenticated.
    let err = Node::join(&h.addr, "n-bad", "not-a-real-token").await;
    match err {
        Err(status) => assert_eq!(status.code(), tonic::Code::Unauthenticated, "{status:?}"),
        Ok(_) => panic!("expected the bad-token join to be rejected"),
    }

    // A valid token joins and receives its first snapshot.
    let mut good = Node::join(&h.addr, "n-good", "tok-a").await.expect("join ok");
    let (version, hash) = good.next_push().await;
    assert_eq!(version, 1, "first snapshot is v1 for this node");
    assert_eq!(hash.len(), 64);
}

#[tokio::test]
async fn two_nodes_join_and_both_receive_the_initial_snapshot() {
    let h = Harness::start("prod").await;
    let mut a = Node::join(&h.addr, "node-a", "tok-a").await.unwrap();
    let mut b = Node::join(&h.addr, "node-b", "tok-b").await.unwrap();

    let (va, ha) = a.next_push().await;
    let (vb, hb) = b.next_push().await;
    // Same rendered config (M1: no selectors), so identical render_hash to both.
    assert_eq!(ha, hb, "both nodes get the same render");
    assert_eq!(va, 1);
    assert_eq!(vb, 1);

    // Both ack their bootstrap push.
    a.ack(va, &ha).await;
    b.ack(vb, &hb).await;

    // The store observes both connected nodes.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let ids = h.cp.store.connected_ids();
    assert!(ids.contains(&"node-a".to_string()));
    assert!(ids.contains(&"node-b".to_string()));
}

#[tokio::test]
async fn a_fully_acked_wave_commits_and_advances_the_fleet_version() {
    let h = Harness::start("prod").await;
    let mut a = Node::join(&h.addr, "node-a", "tok-a").await.unwrap();
    let mut b = Node::join(&h.addr, "node-b", "tok-b").await.unwrap();

    // Drain + ack the bootstrap pushes so both nodes are registered.
    let (va, ha) = a.next_push().await;
    a.ack(va, &ha).await;
    let (vb, hb) = b.next_push().await;
    b.ack(vb, &hb).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Apply a new render (v2 config) and roll it out: both nodes must ack.
    let v2 = render_repo(&testrepo::write("canary")).unwrap();
    let v2_hash = v2.render_hash.clone();
    assert!(h.cp.fleet.set_applied(v2), "a real change applies");

    // Spawn the rollout; concurrently, the nodes receive the push and ack it.
    let cp = h.cp.clone();
    let rollout = tokio::spawn(async move { cp.roll_out("test").await });

    let (va2, ha2) = a.next_push().await;
    assert_eq!(ha2, v2_hash);
    a.ack(va2, &ha2).await;
    let (vb2, hb2) = b.next_push().await;
    b.ack(vb2, &hb2).await;

    let outcome = rollout.await.unwrap();
    match outcome {
        gatewayctl::fleet::WaveOutcome::Committed { node_count, .. } => {
            assert_eq!(node_count, 2)
        }
        other => panic!("expected Committed, got {other:?}"),
    }
    assert!(h.cp.fleet.committed_version() >= 2, "fleet version advanced");
}

#[tokio::test]
async fn a_nack_halts_the_wave_and_the_fleet_version_does_not_advance() {
    let h = Harness::start("prod").await;
    let mut a = Node::join(&h.addr, "node-a", "tok-a").await.unwrap();
    let mut b = Node::join(&h.addr, "node-b", "tok-b").await.unwrap();

    // Bootstrap: both ack v1 so the fleet is at a known-good baseline.
    let (va, ha) = a.next_push().await;
    a.ack(va, &ha).await;
    let (vb, hb) = b.next_push().await;
    b.ack(vb, &hb).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let committed_before = h.cp.fleet.committed_version();

    // Apply a new render and roll out; node-b NACKs it (simulating a local
    // validation failure), node-a acks.
    let v2 = render_repo(&testrepo::write("canary")).unwrap();
    assert!(h.cp.fleet.set_applied(v2));

    let cp = h.cp.clone();
    let rollout = tokio::spawn(async move { cp.roll_out("test").await });

    let (va2, ha2) = a.next_push().await;
    a.ack(va2, &ha2).await;
    let (vb2, hb2) = b.next_push().await;
    b.nack(vb2, &hb2, "unknown provider foo").await;

    let outcome = rollout.await.unwrap();
    match outcome {
        gatewayctl::fleet::WaveOutcome::Halted { divergences, .. } => {
            assert_eq!(divergences.len(), 1);
            assert_eq!(divergences[0].node_id, "node-b");
        }
        other => panic!("expected Halted, got {other:?}"),
    }
    // The fleet's committed version did NOT advance past the pre-wave baseline.
    assert_eq!(
        h.cp.fleet.committed_version(),
        committed_before,
        "a NACK in the wave freezes the committed version"
    );
    // node-b's NACK is surfaced in the runtime store, never silent.
    let nb = h.cp.store.get("node-b").unwrap();
    assert!(nb.last_nack.is_some());
}

/// Finding 1 (fixed): the node ACKs with the hash it RECOMPUTES of the bytes it
/// bound, not the server's advertised `render_hash`. So a control-plane bug (or
/// tampering) that advertises a `render_hash` inconsistent with the shipped
/// config bytes is caught at the wave as a `WrongHash` divergence — an
/// INDEPENDENT hash verification, not a parroted echo. This is the whole point
/// of docs/07 line 71 ("hashes the incoming config bytes ... before anything
/// else"): if the node echoed the advertised hash, this bug would commit
/// silently with the fleet recorded at the wrong hash.
#[tokio::test]
async fn an_inconsistent_advertised_hash_is_caught_because_the_node_acks_its_own_hash() {
    let h = Harness::start("prod").await;
    let mut a = Node::join(&h.addr, "node-a", "tok-a").await.unwrap();

    // Bootstrap: bind and ack the honest first push (ack the TRUE hash of the
    // bytes, exactly as the real gatewayd client does).
    let (v1, _adv1, bytes1) = a.next_push_full().await;
    let true1 = gateway_core::snapshot::content_hash(&String::from_utf8_lossy(&bytes1));
    a.ack(v1, &true1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The control plane pushes VALID bytes but ADVERTISES a mismatched hash —
    // the simulated inconsistency. The honest node recomputes the true hash and
    // ACKs with it.
    let good = render_repo(&testrepo::write("canary")).unwrap();
    let wrong_hash = "0".repeat(64); // deliberately not the bytes' true hash
    let cp = h.cp.clone();
    let bytes = good.config_bytes.clone();
    let wrong = wrong_hash.clone();
    let rollout =
        tokio::spawn(async move { cp.roll_out_tampered("test", &wrong, bytes).await });

    let (v2, adv2, bytes2) = a.next_push_full().await;
    assert_eq!(adv2, wrong_hash, "the control plane advertised the wrong hash");
    // The node binds and recomputes the TRUE hash of the bytes it received.
    let true2 = gateway_core::snapshot::content_hash(&String::from_utf8_lossy(&bytes2));
    assert_ne!(true2, wrong_hash, "the true hash differs from the advertised one");
    a.ack(v2, &true2).await; // ACK the recomputed hash, as the real node does

    let outcome = rollout.await.unwrap();
    match outcome {
        gatewayctl::fleet::WaveOutcome::Halted { divergences, .. } => {
            assert_eq!(divergences.len(), 1);
            assert!(
                matches!(
                    divergences[0].kind,
                    gatewayctl::fleet::DivergenceKind::WrongHash { .. }
                ),
                "an inconsistent advertised hash is a WrongHash divergence, got {:?}",
                divergences[0].kind
            );
        }
        other => panic!("expected Halted on the hash mismatch, got {other:?}"),
    }
    // Crucially, the fleet did NOT commit at the wrong hash.
    assert_eq!(
        h.cp.fleet.committed_version(),
        0,
        "the wave halted; committed version did not advance at the wrong hash"
    );
}

/// Finding 2 (fixed): a node reconnects on its ESTABLISHED IDENTITY (the same
/// node_id re-presenting its now-burned token) after a stream drop, and resumes
/// receiving pushes. The control plane never took the data plane down — it kept
/// serving its last snapshot the whole time — and a fresh token is NOT required
/// to rejoin. A DIFFERENT node replaying that burned token is still refused.
#[tokio::test]
async fn a_node_reconnects_on_its_identity_and_resumes_pushes() {
    let h = Harness::start("prod").await;

    // First join: burns tok-a, binds it to node-a, receives v1.
    let mut a = Node::join(&h.addr, "node-a", "tok-a").await.unwrap();
    let (v1, h1) = a.next_push().await;
    assert_eq!(v1, 1);
    a.ack(v1, &h1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The stream drops: dropping the node's outbound sender ends the session.
    drop(a);
    tokio::time::sleep(Duration::from_millis(150)).await;

    // A DIFFERENT node replaying node-a's burned token is refused — the
    // reconnect path is identity-scoped, no bypass.
    let replay = Node::join(&h.addr, "node-evil", "tok-a").await;
    match replay {
        Err(status) => assert_eq!(status.code(), tonic::Code::Unauthenticated, "{status:?}"),
        Ok(_) => panic!("a burned token replayed by a different node must be refused"),
    }

    // node-a RECONNECTS on its established identity (same node_id + same burned
    // token), advertising the version it last bound. It is admitted and receives
    // the current render again (a re-push it can no-op).
    let mut a2 = Node::join_at_version(&h.addr, "node-a", "tok-a", v1)
        .await
        .expect("node-a may reconnect on its identity");
    let (vr, hr) = a2.next_push().await;
    assert_eq!(hr, h1, "the reconnected node receives the same current render");
    // It can ack again; the fleet keeps functioning across the reconnect.
    a2.ack(vr, &hr).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        h.cp.store.connected_ids().contains(&"node-a".to_string()),
        "node-a is connected again after the reconnect"
    );
}
