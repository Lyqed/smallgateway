//! End-to-end reconciler tests (Phase 2, milestone 2): drift detection and
//! self-heal driven over the REAL gRPC `FleetService`, plus break-glass TTL.
//!
//! A test node joins, acks its bootstrap, then reports (via a `Status`
//! heartbeat) an `observed_render_hash` that DIFFERS from desired — simulating a
//! node that restarted on a stale local file, was break-glassed out of band, or
//! was tampered with. One `Reconciler::tick()` must:
//!   - classify the node as drifted (delivered = desired, observed ≠ desired),
//!   - re-push desired,
//!   - and the node swaps back and acks — healed within one tick.
//!
//! Break-glass: the same drift under an active break-glass window is TOLERATED
//! (no heal push); once the window lapses the next tick heals it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use gateway_proto::fleet::fleet_service_server::FleetServiceServer;
use gateway_proto::{server_message, Ack, ClientMessage, FleetServiceClient, Hello, Status};
use gatewayctl::fleet::{Fleet, WaveOutcome};
use gatewayctl::reconcile::{Reconciler, TickReport};
use gatewayctl::render::{render_repo, testrepo};
use gatewayctl::server::{ControlPlane, FleetSvc};
use gatewayctl::store::RuntimeStore;
use gatewayctl::token::JoinTokens;

struct Harness {
    addr: String,
    cp: Arc<ControlPlane>,
}

impl Harness {
    async fn start(env: &str) -> Harness {
        let rendered = render_repo(&testrepo::write(env)).unwrap();
        let fleet = Arc::new(Fleet::new(rendered));
        let store = Arc::new(RuntimeStore::new());
        let tokens = Arc::new(JoinTokens::new(300));
        tokens.mint("tok-a", BTreeMap::new());
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

    /// Boot a control plane whose fleet has an eu -> us TWO-wave plan, applied at
    /// `env`, minting a region token per wave. Used to prove the reconciler does
    /// not fight a node PENDING in a not-yet-applied later wave.
    async fn start_multiwave(env: &str) -> Harness {
        use gatewayctl::render::{read_gatewaysets, render_resolved};
        use gatewayctl::source::{ConfigSource, DirectorySource};
        use gatewayctl::waves::{Selector, SelectorTerm, UnmatchedPolicy, Wave, WavePlan};

        let root = testrepo::write(env);
        let resolved = DirectorySource::new(&root).resolve().unwrap();
        let applied = render_resolved(&resolved).unwrap();
        let gatewaysets = read_gatewaysets(&resolved).unwrap();
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
        let fleet = Arc::new(Fleet::from_source(applied, resolved, gatewaysets, plan));
        let store = Arc::new(RuntimeStore::new());
        let tokens = Arc::new(JoinTokens::new(300));
        let eu = BTreeMap::from([("region".to_string(), "eu".to_string())]);
        let us = BTreeMap::from([("region".to_string(), "us".to_string())]);
        tokens.mint("tok-eu", eu);
        tokens.mint("tok-us", us);
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

    fn reconciler(&self) -> Reconciler {
        // A long interval — the tests drive `tick()` directly, deterministically.
        Reconciler::new(self.cp.clone(), Duration::from_secs(3600))
    }
}

/// A test node whose observed-hash heartbeat and ack behavior the test controls.
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
        let response = client.session(ReceiverStream::new(out_rx)).await.expect("join");
        Node {
            out,
            inbound: response.into_inner(),
        }
    }

    /// Await the next push and return (version, render_hash, config bytes).
    async fn next_push(&mut self) -> (u64, String, Vec<u8>) {
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

    /// Report an observed hash via a Status heartbeat — the drift signal.
    async fn report_observed(&self, observed_hash: &str) {
        self.out
            .send(ClientMessage::status(Status {
                observed_render_hash: observed_hash.to_string(),
                health: "ok".to_string(),
                in_flight_streams: 0,
            }))
            .await
            .unwrap();
    }

    /// The hash a real node ACKs with: SHA-256 of the bytes it bound.
    fn true_hash(bytes: &[u8]) -> String {
        gateway_core::snapshot::content_hash(&String::from_utf8_lossy(bytes))
    }
}

/// Bring a node to a healthy in-sync baseline: join, ack the bootstrap with the
/// TRUE hash, and report that same hash as observed. Returns the desired hash.
async fn bring_in_sync(node: &mut Node) -> String {
    let (v, _adv, bytes) = node.next_push().await;
    let true_hash = Node::true_hash(&bytes);
    node.ack(v, &true_hash).await;
    node.report_observed(&true_hash).await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    true_hash
}

#[tokio::test]
async fn an_in_sync_node_needs_no_action() {
    let h = Harness::start("prod").await;
    let mut a = Node::join(&h.addr, "node-a", "tok-a").await;
    bring_in_sync(&mut a).await;

    let report = h.reconciler().tick().await;
    assert_eq!(
        report,
        TickReport {
            in_sync: 1,
            ..Default::default()
        },
        "an in-sync node is a no-op"
    );
}

#[tokio::test]
async fn a_drifted_node_is_healed_within_one_tick() {
    let h = Harness::start("prod").await;
    let mut a = Node::join(&h.addr, "node-a", "tok-a").await;
    let desired = bring_in_sync(&mut a).await;

    // The node DRIFTS: it now reports running a DIFFERENT hash than desired
    // (a restart on a stale local file / a break-glass edit / tampering).
    a.report_observed("stale-drifted-hash-0000").await;
    tokio::time::sleep(Duration::from_millis(120)).await;

    // Sanity: the store now sees the drift (observed != desired).
    let before = h.cp.store.get("node-a").unwrap();
    assert_eq!(before.observed_hash.as_deref(), Some("stale-drifted-hash-0000"));
    assert_ne!(before.observed_hash.as_deref(), Some(desired.as_str()));

    // Drive one reconcile tick concurrently with the node servicing the heal
    // re-push (bind the pushed bytes, recompute the true hash, ack it).
    let cp = h.cp.clone();
    let tick = tokio::spawn(async move {
        Reconciler::new(cp, Duration::from_secs(3600)).tick().await
    });

    // The node receives the heal push and acks with the true hash — swapping
    // back to desired. Then it reports desired as observed (healed).
    let (v, _adv, bytes) = a.next_push().await;
    let true_hash = Node::true_hash(&bytes);
    assert_eq!(true_hash, desired, "the heal re-pushes DESIRED");
    a.ack(v, &true_hash).await;

    let report = tick.await.unwrap();
    assert_eq!(report.healed, 1, "the drifted node was healed this tick: {report:?}");

    // After acking, the node reports desired as observed → back in sync.
    a.report_observed(&desired).await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    let after = h.cp.store.get("node-a").unwrap();
    assert_eq!(after.observed_hash.as_deref(), Some(desired.as_str()));
    let confirm = h.reconciler().tick().await;
    assert_eq!(confirm.in_sync, 1, "healed node is now in sync: {confirm:?}");
}

#[tokio::test]
async fn break_glass_tolerates_drift_then_heals_after_the_ttl() {
    let h = Harness::start("prod").await;
    let mut a = Node::join(&h.addr, "node-a", "tok-a").await;
    let desired = bring_in_sync(&mut a).await;

    // The node drifts.
    a.report_observed("stale-drifted-hash-0000").await;
    tokio::time::sleep(Duration::from_millis(120)).await;

    // Operator break-glasses the node for a SHORT window (1s): the reconciler
    // must TOLERATE its drift and not fight it.
    h.cp.store.set_break_glass("node-a", 1);

    let report = h.reconciler().tick().await;
    assert_eq!(
        report.break_glass_tolerated, 1,
        "the drift is tolerated under break-glass: {report:?}"
    );
    assert_eq!(report.healed, 0, "no heal push while break-glass is active");

    // Let the break-glass window lapse.
    tokio::time::sleep(Duration::from_millis(1100)).await;

    // Now the reconciler resumes and heals the node back to desired.
    let cp = h.cp.clone();
    let tick = tokio::spawn(async move {
        Reconciler::new(cp, Duration::from_secs(3600)).tick().await
    });
    let (v, _adv, bytes) = a.next_push().await;
    let true_hash = Node::true_hash(&bytes);
    assert_eq!(true_hash, desired);
    a.ack(v, &true_hash).await;
    let report = tick.await.unwrap();
    assert_eq!(
        report.healed, 1,
        "after the TTL lapses the reconciler heals the node: {report:?}"
    );
}

/// Drive a node to service the next push by binding its bytes, recomputing the
/// true hash, and acking it. Returns the true hash it acked.
async fn service_push(node: &mut Node) -> String {
    let (v, _adv, bytes) = node.next_push().await;
    let true_hash = Node::true_hash(&bytes);
    node.ack(v, &true_hash).await;
    true_hash
}

/// Whether the node received a push within `window` (non-panicking, for the
/// negative assertion "this node was NOT pushed").
async fn pushed_within(node: &mut Node, window: Duration) -> bool {
    matches!(
        tokio::time::timeout(window, node.inbound.next()).await,
        Ok(Some(Ok(m))) if matches!(m.kind, Some(server_message::Kind::Push(_)))
    )
}

/// The reconciler must NOT fight a node PENDING in a not-yet-applied later wave
/// (docs/07). Setup: an eu -> us two-wave fleet. Both nodes are in sync on the
/// OLD commit. A NEW commit is published and the EU wave is recorded committed on
/// it (a partial application: eu advanced, us has not). One reconcile tick then:
///   - HEALS the eu node (its wave reached target; it is genuinely behind), and
///   - LEAVES the us node alone — its wave has not been reached, so it is on its
///     prior version legitimately (pending, not drifted) and is never pushed.
#[tokio::test]
async fn the_reconciler_does_not_fight_a_node_pending_in_a_later_wave() {
    use gatewayctl::render::{read_gatewaysets, render_resolved};
    use gatewayctl::source::{ConfigSource, DirectorySource};
    use gatewayctl::waves::{Selector, SelectorTerm, UnmatchedPolicy, Wave, WavePlan};

    let h = Harness::start_multiwave("prod").await;
    let mut eu = Node::join(&h.addr, "node-eu", "tok-eu").await;
    let mut us = Node::join(&h.addr, "node-us", "tok-us").await;
    let old_desired = bring_in_sync(&mut eu).await;
    bring_in_sync(&mut us).await;

    // Publish a NEW applied render (canary) with the same two-wave plan, exactly
    // as reload_and_roll does before walking the waves.
    let root = testrepo::write("canary");
    let resolved = DirectorySource::new(&root).resolve().unwrap();
    let new_applied = render_resolved(&resolved).unwrap();
    let new_desired = new_applied.render_hash.clone();
    let new_commit = new_applied.source_commit.clone();
    let gatewaysets = read_gatewaysets(&resolved).unwrap();
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
    assert_ne!(new_desired, old_desired, "canary is a real change");
    h.cp
        .fleet
        .set_applied_from_source(new_applied, resolved, gatewaysets, plan);
    // The EU wave advanced to the new commit; the US wave has NOT (partial apply).
    h.cp.fleet.set_wave_commit("eu", &new_commit);

    // Sanity on the pure guard: us is pending, eu is not.
    let us_labels = BTreeMap::from([("region".to_string(), "us".to_string())]);
    let eu_labels = BTreeMap::from([("region".to_string(), "eu".to_string())]);
    assert!(h.cp.fleet.node_pending_in_unapplied_wave(&us_labels));
    assert!(!h.cp.fleet.node_pending_in_unapplied_wave(&eu_labels));

    // One reconcile tick, concurrently with the eu node servicing its heal push.
    let cp = h.cp.clone();
    let tick = tokio::spawn(async move {
        Reconciler::new(cp, Duration::from_secs(3600)).tick().await
    });
    let eu_healed = service_push(&mut eu).await;
    assert_eq!(eu_healed, new_desired, "eu is healed toward the NEW desired");

    // The us node must NOT have been pushed — it is pending in its later wave.
    assert!(
        !pushed_within(&mut us, Duration::from_millis(600)).await,
        "the us node (pending in a not-yet-applied later wave) must NOT be healed/pushed"
    );

    let report = tick.await.unwrap();
    assert_eq!(report.healed, 1, "only the eu node healed: {report:?}");
    assert_eq!(
        report.pending_later_wave, 1,
        "the us node was left as pending in a later wave: {report:?}"
    );
}

/// REGRESSION (the HIGH defect): a reconcile tick that lands WHILE a wave is
/// rolling out a new desired render must NOT fight the mid-rollout node. Before
/// the fix, `set_applied(new)` publishes the new desired before the wave
/// finishes, a tick in that window classifies the still-old node DeliveryStale
/// and heals it — colliding on the shared pending slot, resolving only one
/// waiter, and falsely reporting the node Silent so the wave HALTS even though
/// the node acked. The fix has two independent guards: (1) the reconciler
/// defers to an in-flight wave, and (2) pending waiters are namespaced by a
/// unique push-id so even a collision could not clobber the wave's correlation.
#[tokio::test]
async fn a_reconcile_tick_during_a_wave_does_not_falsely_halt_it() {
    // Start applied at "prod"; bring the node in sync there.
    let h = Harness::start("prod").await;
    let mut a = Node::join(&h.addr, "node-a", "tok-a").await;
    bring_in_sync(&mut a).await;

    // A new commit renders a DIFFERENT desired ("canary"). Publish it as the new
    // applied render, exactly as reload_and_roll does before rolling the wave.
    let next = render_repo(&testrepo::write("canary")).unwrap();
    let new_desired = next.render_hash.clone();
    assert!(h.cp.fleet.set_applied(next), "canary is a real change");

    // Fire the wave. It publishes new_desired to the node and BLOCKS awaiting
    // the ack (the node has not acked yet), holding the wave-in-flight guard the
    // whole time.
    let cp_wave = h.cp.clone();
    let wave = tokio::spawn(async move { cp_wave.roll_out("test-reload").await });

    // Deterministically land the reconcile tick INSIDE the wave window: spin
    // until the wave is provably in flight (its guard is set), then run the tick
    // INLINE while the wave is still blocked on the ack. This removes the timing
    // race — the tick cannot observe a completed wave because the wave cannot
    // complete until we service the push below.
    for _ in 0..200 {
        if h.cp.fleet.wave_in_flight() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(h.cp.fleet.wave_in_flight(), "the wave is in flight");
    let report = Reconciler::new(h.cp.clone(), Duration::from_secs(3600))
        .tick()
        .await;
    // The tick, seeing a wave in flight, DEFERRED to it — never healed/fought.
    assert_eq!(
        report.healed, 0,
        "the reconciler did not fight the mid-rollout node: {report:?}"
    );
    assert_eq!(
        report.mid_rollout_deferred, 1,
        "the mid-rollout node was deferred to the wave: {report:?}"
    );

    // Now service the wave's push: bind the bytes and ack the true canary hash.
    let acked = service_push(&mut a).await;
    assert_eq!(acked, new_desired, "the wave pushed the new desired render");

    let outcome = wave.await.unwrap();
    match outcome {
        WaveOutcome::Committed { render_hash, node_count } => {
            assert_eq!(render_hash, new_desired);
            assert_eq!(node_count, 1);
        }
        other => panic!(
            "the wave must COMMIT (the node acked), not halt: got {other:?}"
        ),
    }
    assert_eq!(
        h.cp.fleet.committed_version(),
        2,
        "committed_version advanced to the canary wave version; no phantom halt"
    );
}

/// The correlation guard proven directly at the server layer: two concurrent
/// pushes to the SAME node at the SAME render (a wave and a self-heal both
/// pushing the identical applied render) each get their own pending slot, so the
/// node's single ack resolves BOTH — neither is clobbered into a false Silent.
/// This is the push-id namespacing (Fix A) in isolation, without relying on the
/// reconciler's deferral (Fix B), so the correlation core is covered even if a
/// heal and a wave ever did overlap.
#[tokio::test]
async fn concurrent_pushes_at_the_same_version_both_resolve_on_one_ack() {
    let h = Harness::start("prod").await;
    let mut a = Node::join(&h.addr, "node-a", "tok-a").await;
    let desired = bring_in_sync(&mut a).await;

    // Fire a self-heal AND a single-node wave for the SAME applied render
    // concurrently. next_version_for returns the SAME version for the same hash,
    // so both pushes correlate to the same fleet_version — the exact collision.
    let cp_heal = h.cp.clone();
    let heal = tokio::spawn(async move { cp_heal.heal_node("node-a").await });
    let cp_wave = h.cp.clone();
    let wave = tokio::spawn(async move { cp_wave.roll_out("concurrent").await });

    // The node services BOTH pushes (each arrives as its own Push message) and
    // acks each with the true (desired) hash. One ack per push message.
    let first = service_push(&mut a).await;
    let second = service_push(&mut a).await;
    assert_eq!(first, desired);
    assert_eq!(second, desired);

    // Both awaited operations resolve on the acks — neither is stranded Silent.
    let healed = heal.await.unwrap();
    let outcome = wave.await.unwrap();
    assert!(healed, "the self-heal saw its ack (not clobbered into Silent)");
    assert!(
        matches!(outcome, WaveOutcome::Committed { .. }),
        "the wave saw its ack and committed (not a phantom Silent halt): {outcome:?}"
    );
}
