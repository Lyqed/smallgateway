//! End-to-end GB-5 budget-share tests (Phase 3) driven over the REAL gRPC
//! `FleetService`: a node reports observed spend up the stream, the control
//! plane rebalances shares from that telemetry and grants them back, a hot node
//! gets a bigger slice than a cold one, and the ~90% escalation (`SyncCheck`)
//! gets a fresh grant synchronously. Fleet-wide GB-6 alerts raised from the
//! ingest are captured through a test alert sink.
//!
//! The allocation math itself is unit-tested in `gatewayctl::budget`; these
//! tests prove the wire path: UsageReport -> ingest -> rebalance -> ShareGrant,
//! and SyncCheck -> regrant, against the actual server.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use gateway_core::budget::{Alert, AlertKind, AlertSink};
use gateway_proto::fleet::fleet_service_server::FleetServiceServer;
use gateway_proto::{
    server_message, BudgetShare, ClientMessage, FleetServiceClient, Hello, ShareGrant, SyncCheck,
    UsageReport,
};
use gatewayctl::fleet::Fleet;
use gatewayctl::render::{render_repo, testrepo};
use gatewayctl::server::{ControlPlane, FleetSvc};
use gatewayctl::store::RuntimeStore;
use gatewayctl::token::JoinTokens;

/// A capturing GB-6 alert sink so a test asserts on what the enforcement layer
/// raised fleet-wide.
#[derive(Default)]
struct CaptureSink(Mutex<Vec<Alert>>);

impl CaptureSink {
    fn alerts(&self) -> Vec<Alert> {
        self.0.lock().unwrap().clone()
    }
}

impl AlertSink for CaptureSink {
    fn emit(&self, alert: &Alert) {
        self.0.lock().unwrap().push(alert.clone());
    }
}

struct Harness {
    addr: String,
    sink: Arc<CaptureSink>,
}

impl Harness {
    async fn start() -> Harness {
        let rendered = render_repo(&testrepo::write("prod")).unwrap();
        let fleet = Arc::new(Fleet::new(rendered));
        let store = Arc::new(RuntimeStore::new());
        let tokens = Arc::new(JoinTokens::new(300));
        tokens.mint("tok-a", BTreeMap::new());
        tokens.mint("tok-b", BTreeMap::new());
        let sink = Arc::new(CaptureSink::default());
        let cp = ControlPlane::with_alert_sink(fleet, store, tokens, sink.clone());

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
        Harness { addr: endpoint, sink }
    }
}

/// A test node that joins, drains its bootstrap push, and can send usage /
/// sync-check messages and await the share grants that come back.
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
        let mut node = Node {
            out,
            inbound: response.into_inner(),
        };
        node.drain_bootstrap_push().await;
        node
    }

    /// Drain the initial bootstrap Push so the next awaited server message is a
    /// share grant, not the config push.
    async fn drain_bootstrap_push(&mut self) {
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(3), self.inbound.next())
                .await
                .expect("bootstrap within timeout")
                .expect("stream open")
                .expect("no stream error");
            if let Some(server_message::Kind::Push(_)) = msg.kind {
                return;
            }
        }
    }

    async fn report_usage(&self, key: &str, value: &str, cap: u64, spent: u64) {
        self.out
            .send(ClientMessage::usage(UsageReport {
                spenders: vec![BudgetShare {
                    attribution_key: key.to_string(),
                    attribution_value: value.to_string(),
                    cap_tokens: cap,
                    tokens: spent,
                }],
            }))
            .await
            .unwrap();
    }

    async fn sync_check(&self, key: &str, value: &str, cap: u64, spent: u64) {
        self.out
            .send(ClientMessage::sync_check(SyncCheck {
                spenders: vec![BudgetShare {
                    attribution_key: key.to_string(),
                    attribution_value: value.to_string(),
                    cap_tokens: cap,
                    tokens: spent,
                }],
            }))
            .await
            .unwrap();
    }

    /// Await the next `ShareGrant` from the control plane (skipping liveness /
    /// pushes), or panic on timeout.
    async fn next_grant(&mut self) -> ShareGrant {
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(3), self.inbound.next())
                .await
                .expect("grant within timeout")
                .expect("stream open")
                .expect("no stream error");
            if let Some(server_message::Kind::ShareGrant(g)) = msg.kind {
                return g;
            }
        }
    }
}

fn share_of<'a>(grant: &'a ShareGrant, value: &str) -> Option<&'a BudgetShare> {
    grant
        .shares
        .iter()
        .find(|s| s.attribution_value == value)
}

#[tokio::test]
async fn a_node_reporting_spend_gets_a_share_grant_back_over_the_wire() {
    let h = Harness::start().await;
    let mut node = Node::join(&h.addr, "n1", "tok-a").await;

    node.report_usage("team", "ml-research", 100_000, 30_000).await;
    let grant = node.next_grant().await;

    let share = share_of(&grant, "ml-research").expect("share for the reported value");
    assert_eq!(share.cap_tokens, 100_000);
    // Single node holding the whole cap: its share is the cap (30k spent + all
    // 70k headroom).
    assert_eq!(share.tokens, 100_000);
}

#[tokio::test]
async fn the_hot_node_gets_a_bigger_slice_than_the_cold_one_over_the_wire() {
    let h = Harness::start().await;
    let mut hot = Node::join(&h.addr, "hot", "tok-a").await;
    let mut cold = Node::join(&h.addr, "cold", "tok-b").await;

    // Both nodes report; hot has spent far more of the same capped value.
    hot.report_usage("team", "ml-research", 100_000, 60_000).await;
    let _ = hot.next_grant().await;
    cold.report_usage("team", "ml-research", 100_000, 5_000).await;
    let cold_grant = cold.next_grant().await;

    // Re-report hot so it gets a fresh grant reflecting the full fleet picture.
    hot.report_usage("team", "ml-research", 100_000, 60_000).await;
    let hot_grant = hot.next_grant().await;

    let hot_share = share_of(&hot_grant, "ml-research").unwrap().tokens;
    let cold_share = share_of(&cold_grant, "ml-research").unwrap().tokens;
    assert!(
        hot_share > cold_share,
        "hot node's slice ({hot_share}) must exceed the cold node's ({cold_share})"
    );
    // The shares never exceed the cap.
    assert!(hot_share <= 100_000 && cold_share <= 100_000);
}

#[tokio::test]
async fn a_sync_check_at_the_escalation_boundary_gets_a_fresh_grant_synchronously() {
    let h = Harness::start().await;
    let mut node = Node::join(&h.addr, "n1", "tok-a").await;

    // The node is at ~92% of its share and escalates synchronously for more.
    node.sync_check("team", "ml-research", 100_000, 92_000).await;
    let grant = node.next_grant().await;
    let share = share_of(&grant, "ml-research").expect("regrant for the escalating value");
    // The regrant confirms/raises the ceiling — a single node holds the whole
    // cap, so it is granted the full 100k it may still spend up to.
    assert_eq!(share.tokens, 100_000);
}

#[tokio::test]
async fn a_fleet_wide_spend_crossing_fires_a_gb6_alert_from_the_ingest() {
    let h = Harness::start().await;
    let mut a = Node::join(&h.addr, "a", "tok-a").await;
    let mut b = Node::join(&h.addr, "b", "tok-b").await;

    // Each node alone is below 80% of the cap, but together they cross it —
    // the control plane raises the soft GB-6 alert from the enforcement
    // telemetry, at the point of ingest, not reconstructed from logs.
    a.report_usage("team", "ml-research", 100_000, 45_000).await;
    let _ = a.next_grant().await;
    b.report_usage("team", "ml-research", 100_000, 40_000).await; // total 85k
    let _ = b.next_grant().await;

    // Give the server task a beat to record the alert.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let alerts = h.sink.alerts();
    assert!(
        alerts
            .iter()
            .any(|al| matches!(al.kind, AlertKind::SoftThreshold { .. })
                && al.spend == 85_000),
        "expected a fleet-wide soft alert at 85k, got {alerts:?}"
    );
}
