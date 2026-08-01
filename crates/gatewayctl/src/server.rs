//! The gRPC `FleetService` server (docs/07, "The stream").
//!
//! One long-lived bidirectional stream per data plane. The control plane is the
//! server; the data plane dials out. This module:
//!
//! 1. authenticates each `Hello` via its join token (bad token -> the stream is
//!    refused, loudly),
//! 2. pushes the current rendered snapshot to a freshly-joined node,
//! 3. on a local reload (SIGHUP or a rendered-config change), runs one
//!    all-or-nothing wave: pushes the new versioned snapshot to every connected
//!    node and collects each node's Ack/Nack; on any Nack the divergence is
//!    logged loudly and the fleet's committed version does not advance,
//! 4. records every Ack/Nack/Status into the in-memory runtime store.
//!
//! Concurrency shape: each session owns an outbound `mpsc` sender registered in
//! a shared [`Sessions`] table by node_id. A push writes a `ServerMessage` to a
//! session's sender; the session's inbound loop routes the node's Ack/Nack back
//! to the waiting wave via a per-`(node,version)` oneshot. This keeps the wave
//! sequencing entirely in the control plane and leaves the node code unchanged
//! (docs/07: "It reuses the node code unchanged").

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use log::{error, info, warn};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tonic::{Request, Response, Status as GrpcStatus, Streaming};

use gateway_proto::{
    client_message, Ack, ClientMessage, FleetService, Nack, RenderedSnapshot, ServerMessage,
};

use crate::fleet::{AckResult, Divergence, DivergenceKind, Fleet, WaveOutcome};
use crate::store::RuntimeStore;
use crate::token::{Admission, JoinTokens};

/// How long a wave waits for each node to answer before treating it as silent
/// (docs/07: the unknown-node timeout; "unknown halts"). A conservative,
/// short M1 default; a real deployment tunes it per fleet.
const WAVE_ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// A globally-unique id for one push-and-await, so two concurrent pushes to the
/// SAME node at the SAME fleet_version (e.g. a wave and a self-heal both pushing
/// the identical applied render — `next_version_for` returns the same version
/// for the same hash) each get their OWN pending slot and neither clobbers the
/// other. Keying `pending` by fleet_version alone silently overwrote one
/// waiter's oneshot sender, stranding the wave's correlation and falsely
/// reporting the node Silent even though it acked (the HIGH defect).
static PUSH_SEQ: AtomicU64 = AtomicU64::new(1);

fn next_push_id() -> u64 {
    PUSH_SEQ.fetch_add(1, Ordering::Relaxed)
}

/// One pending push awaiting the node's answer: which `fleet_version` its ACK
/// will carry, and the oneshot the inbound loop fires when it arrives.
struct Pending {
    awaited_version: u64,
    waiter: oneshot::Sender<AckResult>,
}

/// A registered push awaiting an ack: its unique push-id (to clear exactly this
/// slot on timeout) and the receiver its ack resolves. `None` when the node's
/// stream was already gone at push time (resolves as `Silent`).
type PendingHandle = Option<(u64, oneshot::Receiver<AckResult>)>;

/// One connected node's outbound channel + the pending-ack correlations for the
/// pushes currently in flight to it. Keyed by a unique push-id (NOT by version)
/// so concurrent pushes at the same version cannot clobber each other.
struct Session {
    tx: mpsc::Sender<Result<ServerMessage, GrpcStatus>>,
    /// push_id -> the pending correlation for that in-flight push.
    pending: BTreeMap<u64, Pending>,
}

/// The registry of live sessions, keyed by node_id. Shared across every stream
/// handler and the reload path.
#[derive(Clone, Default)]
pub struct Sessions(Arc<Mutex<BTreeMap<String, Session>>>);

impl Sessions {
    async fn insert(&self, node_id: &str, tx: mpsc::Sender<Result<ServerMessage, GrpcStatus>>) {
        self.0.lock().await.insert(
            node_id.to_string(),
            Session {
                tx,
                pending: BTreeMap::new(),
            },
        );
    }

    async fn remove(&self, node_id: &str) {
        self.0.lock().await.remove(node_id);
    }

    /// Route an inbound Ack/Nack to every pending push awaiting this version.
    ///
    /// A node sends ONE answer per fleet_version. Any push in flight to this node
    /// that awaits that version is satisfied by it — so we fan the result out to
    /// ALL such waiters (there can legitimately be more than one when a wave and
    /// a self-heal both push the identical applied render concurrently). Each is
    /// keyed by its unique push-id, so removing them cannot disturb a waiter that
    /// awaits a different version.
    async fn deliver(&self, node_id: &str, version: u64, result: AckResult) {
        let mut guard = self.0.lock().await;
        let Some(session) = guard.get_mut(node_id) else {
            return;
        };
        let matched: Vec<u64> = session
            .pending
            .iter()
            .filter(|(_, p)| p.awaited_version == version)
            .map(|(id, _)| *id)
            .collect();
        for id in matched {
            if let Some(pending) = session.pending.remove(&id) {
                let _ = pending.waiter.send(result.clone());
            }
        }
    }

    async fn connected_ids(&self) -> Vec<String> {
        self.0.lock().await.keys().cloned().collect()
    }
}

/// The control-plane state shared by every stream and the reload path.
pub struct ControlPlane {
    pub fleet: Arc<Fleet>,
    pub store: Arc<RuntimeStore>,
    pub tokens: Arc<JoinTokens>,
    pub sessions: Sessions,
}

/// A cheaply-cloneable service handle. tonic clones the service per connection;
/// the orphan rule also forbids `impl FleetService for Arc<ControlPlane>`
/// (foreign trait, foreign type), so this local newtype carries the impl.
#[derive(Clone)]
pub struct FleetSvc(pub Arc<ControlPlane>);

impl FleetSvc {
    /// Build a service handle wrapping the shared control-plane state.
    pub fn new(cp: Arc<ControlPlane>) -> FleetSvc {
        FleetSvc(cp)
    }
}

impl ControlPlane {
    pub fn new(fleet: Arc<Fleet>, store: Arc<RuntimeStore>, tokens: Arc<JoinTokens>) -> Arc<ControlPlane> {
        Arc::new(ControlPlane {
            fleet,
            store,
            tokens,
            sessions: Sessions::default(),
        })
    }

    /// Push a snapshot to one node and register a pending-ack waiter under a
    /// UNIQUE push-id, returning that id plus the receiver the caller awaits.
    /// Returns `None` if the node's stream is gone (its result becomes
    /// `Silent`). The push-id lets a caller clear exactly its own pending slot
    /// on timeout, without disturbing a concurrent push to the same node.
    async fn push_and_await(
        &self,
        node_id: &str,
        snapshot: RenderedSnapshot,
    ) -> PendingHandle {
        let awaited_version = snapshot.fleet_version;
        let push_id = next_push_id();
        let mut guard = self.sessions.0.lock().await;
        let session = guard.get_mut(node_id)?;
        let (waiter_tx, waiter_rx) = oneshot::channel();
        session.pending.insert(
            push_id,
            Pending {
                awaited_version,
                waiter: waiter_tx,
            },
        );
        // If the send fails the stream is dead; drop the waiter so it resolves
        // as Silent via the timeout path.
        if session
            .tx
            .send(Ok(ServerMessage::push(snapshot)))
            .await
            .is_err()
        {
            session.pending.remove(&push_id);
            return None;
        }
        Some((push_id, waiter_rx))
    }

    /// Run one all-or-nothing wave for the currently-applied render across every
    /// connected node. Single wave in M1. Returns the adjudicated outcome and
    /// logs the loud divergence line on a halt.
    pub async fn roll_out(&self, trigger: &str) -> WaveOutcome {
        let rendered = self.fleet.applied();
        self.run_wave(trigger, &rendered.render_hash, &rendered.source_commit, &rendered.config_bytes)
            .await
    }

    /// Self-heal one drifted node: re-push the CURRENT applied render to just
    /// that node and await its ack (docs/07: "Self-heal is re-push"). Unlike a
    /// wave, this touches ONE node and never advances the fleet's committed
    /// version — it converges a node that fell behind desired, it does not roll
    /// out a new desired. Returns `true` if the node acked the desired
    /// `render_hash` within the timeout, `false` otherwise (a NACK or silence,
    /// which the reconciler surfaces and retries next tick).
    pub async fn heal_node(&self, node_id: &str) -> bool {
        let rendered = self.fleet.applied();
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

    async fn clear_pending(&self, node_id: &str, push_id: u64) {
        if let Some(session) = self.sessions.0.lock().await.get_mut(node_id) {
            session.pending.remove(&push_id);
        }
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

/// A short label for a node's bootstrap answer, for the join log line.
fn describe_ack(result: &AckResult) -> String {
    match result {
        AckResult::Acked { hash } => format!("ACK (hash={})", short(hash)),
        AckResult::Nacked { reason } => format!("NACK ({reason})"),
        AckResult::Silent => "SILENT".to_string(),
    }
}

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

fn short(hash: &str) -> &str {
    &hash[..12.min(hash.len())]
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

type ServerStream =
    Pin<Box<dyn Stream<Item = Result<ServerMessage, GrpcStatus>> + Send + 'static>>;

#[tonic::async_trait]
impl FleetService for FleetSvc {
    type SessionStream = ServerStream;

    async fn session(
        &self,
        request: Request<Streaming<ClientMessage>>,
    ) -> Result<Response<Self::SessionStream>, GrpcStatus> {
        let this = &self.0;
        let mut inbound = request.into_inner();

        // The stream MUST open with a Hello. Authenticate it before anything.
        let first = inbound
            .next()
            .await
            .ok_or_else(|| GrpcStatus::invalid_argument("stream closed before Hello"))?
            .map_err(|e| GrpcStatus::internal(format!("stream error: {e}")))?;

        let hello = match first.kind {
            Some(client_message::Kind::Hello(h)) => h,
            _ => {
                return Err(GrpcStatus::invalid_argument(
                    "first message must be Hello",
                ))
            }
        };

        // Join auth (docs/07: authenticate the join; the node identity
        // authenticates every subsequent stream). A first join burns the token
        // and binds it to this node_id; a reconnect is that same node
        // re-presenting its (now-burned) token, admitted as an established
        // identity. A bad token — unknown, expired-unused, or burned-by-another
        // — is refused loudly and the stream never opens.
        let node_id = hello.node_id.clone();
        let (labels, admission) = match this.tokens.authorize(&node_id, &hello.join_token) {
            Ok(ok) => ok,
            Err(e) => {
                warn!(
                    "[join] REJECTED node {:?}: {e}",
                    hello.node_id
                );
                return Err(GrpcStatus::unauthenticated(format!("join rejected: {e}")));
            }
        };
        info!(
            "[join] node {:?} {}; labels={:?} current_fleet_version=v{}",
            node_id,
            match admission {
                Admission::FreshJoin => "joined (fresh)",
                Admission::Reconnect => "reconnected (established identity)",
            },
            labels,
            hello.current_fleet_version
        );

        // Register the outbound channel.
        let (tx, rx) = mpsc::channel::<Result<ServerMessage, GrpcStatus>>(16);
        this.store.connect(&node_id, labels);
        this.sessions.insert(&node_id, tx.clone()).await;

        // Push the node its first snapshot immediately (bootstrap), unless it
        // reconnected already at the current render (skip the redelivery).
        let rendered = this.fleet.applied();
        let cp = self.0.clone();
        let node_for_task = node_id.clone();
        tokio::spawn(async move {
            let version = cp.fleet.next_version_for(&node_for_task, &rendered.render_hash);
            let snapshot = rendered.to_snapshot(&node_for_task, version, now_unix());
            // Register a pending waiter so the node's Ack for this bootstrap
            // push is recorded (and not treated as an orphan).
            let pushed = cp.push_and_await(&node_for_task, snapshot).await;
            info!(
                "[join] pushed initial render_hash={} v={version} to {:?}",
                short(&rendered.render_hash),
                node_for_task
            );
            // A bootstrap push is NOT a wave: it delivers to ONE joining node,
            // and the all-or-nothing invariant (docs/07: "commit only on a full
            // sweep") requires EVERY connected node to ack before the fleet's
            // committed version advances. So we deliberately do NOT call
            // conclude_wave here — that would advance committed_version off a
            // single node's ack, bypassing wave adjudication and diverging from
            // the invariant the moment a second node exists. The ack is recorded
            // in the runtime store by the inbound loop (record_ack) and surfaced
            // there; committed_version only moves through run_wave over the full
            // connected set. We still drain the waiter so the oneshot resolves
            // and the pending entry is cleaned up.
            if let Some((push_id, rx)) = pushed {
                match tokio::time::timeout(WAVE_ACK_TIMEOUT, rx).await {
                    Ok(Ok(result)) => {
                        info!(
                            "[join] node {:?} answered bootstrap v{version}: {}; \
                             recorded (fleet committed_version NOT advanced by a \
                             single-node bootstrap — that needs a full wave)",
                            node_for_task,
                            describe_ack(&result),
                        );
                    }
                    _ => {
                        // A bootstrap NACK/silence is surfaced via the store; it
                        // does not by itself advance the fleet.
                        cp.clear_pending(&node_for_task, push_id).await;
                    }
                }
            }
        });

        // The inbound loop: route Ack/Nack/Status, update the store, and detect
        // disconnect. Runs as a task so the outbound stream returns now.
        let cp = self.0.clone();
        let node_for_loop = node_id.clone();
        tokio::spawn(async move {
            while let Some(msg) = inbound.next().await {
                let msg = match msg {
                    Ok(m) => m,
                    Err(e) => {
                        warn!("[stream] node {:?} stream error: {e}", node_for_loop);
                        break;
                    }
                };
                match msg.kind {
                    Some(client_message::Kind::Ack(Ack {
                        fleet_version,
                        render_hash,
                    })) => {
                        info!(
                            "[ack] node {:?} acked v{fleet_version} hash={}",
                            node_for_loop,
                            short(&render_hash)
                        );
                        cp.store.record_ack(&node_for_loop, fleet_version, &render_hash);
                        cp.sessions
                            .deliver(&node_for_loop, fleet_version, AckResult::Acked { hash: render_hash })
                            .await;
                    }
                    Some(client_message::Kind::Nack(Nack {
                        fleet_version,
                        render_hash: _,
                        reason,
                    })) => {
                        warn!(
                            "[nack] node {:?} NACKed v{fleet_version}: {reason} \
                             (node keeps its prior version)",
                            node_for_loop
                        );
                        cp.store.record_nack(&node_for_loop, fleet_version, &reason);
                        cp.sessions
                            .deliver(&node_for_loop, fleet_version, AckResult::Nacked { reason })
                            .await;
                    }
                    Some(client_message::Kind::Status(status)) => {
                        cp.store.record_status(
                            &node_for_loop,
                            &status.observed_render_hash,
                            &status.health,
                        );
                        // Liveness reply, nothing more.
                        let _ = tx.send(Ok(ServerMessage::ack_of_status())).await;
                    }
                    Some(client_message::Kind::Hello(_)) | None => {
                        warn!("[stream] node {:?} sent an unexpected message", node_for_loop);
                    }
                }
            }
            // Stream ended: mark the node gone and drop its session.
            info!("[stream] node {:?} disconnected", node_for_loop);
            cp.store.disconnect(&node_for_loop);
            cp.sessions.remove(&node_for_loop).await;
        });

        let out: ServerStream = Box::pin(ReceiverStream::new(rx));
        Ok(Response::new(out))
    }
}
