//! The gRPC `FleetService` server (docs/07, "The stream").
//!
//! One long-lived bidirectional stream per data plane. The control plane is the
//! server; the data plane dials out. This module:
//!
//! 1. authenticates each `Hello` via its join token (bad token -> the stream is
//!    refused, loudly),
//! 2. pushes the current rendered snapshot to a freshly-joined node,
//! 3. owns the session table and the push/ack CORRELATION machinery that the
//!    wave rollout builds on (the rollout walk itself lives in [`crate::rollout`]),
//! 4. records every Ack/Nack/Status into the in-memory runtime store.
//!
//! Concurrency shape: each session owns an outbound `mpsc` sender registered in
//! a shared [`Sessions`] table by node_id. A push writes a `ServerMessage` to a
//! session's sender; the session's inbound loop routes the node's Ack/Nack back
//! to the waiting rollout via a per-push-id oneshot keyed on the awaited
//! `fleet_version`. This keeps the wave sequencing entirely in the control plane
//! and leaves the node code unchanged (docs/07: "It reuses the node code
//! unchanged"). The rollout methods ([`crate::rollout`]) are a second
//! `impl ControlPlane` block that calls the `pub(crate)` push/await helpers here.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use log::{info, warn};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tonic::{Request, Response, Status as GrpcStatus, Streaming};

use gateway_core::budget::{AlertSink, CapId, LogAlertSink};
use gateway_proto::{
    client_message, Ack, BudgetShare, ClientMessage, FleetService, Nack, RenderedSnapshot,
    ServerMessage, ShareGrant, UsageReport,
};

use crate::budget::FleetBudgets;
use crate::fleet::{AckResult, Fleet};
use crate::store::RuntimeStore;
use crate::token::{Admission, JoinTokens};

/// How long a wave waits for each node to answer before treating it as silent
/// (docs/07: the unknown-node timeout; "unknown halts"). A conservative,
/// short M1 default; a real deployment tunes it per fleet.
pub(crate) const WAVE_ACK_TIMEOUT: Duration = Duration::from_secs(5);

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
pub(crate) type PendingHandle = Option<(u64, oneshot::Receiver<AckResult>)>;

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

    pub(crate) async fn connected_ids(&self) -> Vec<String> {
        self.0.lock().await.keys().cloned().collect()
    }
}

/// The control-plane state shared by every stream and the reload path.
pub struct ControlPlane {
    pub fleet: Arc<Fleet>,
    pub store: Arc<RuntimeStore>,
    pub tokens: Arc<JoinTokens>,
    pub sessions: Sessions,
    /// GB-5: the fleet-wide budget ledger — observed per-node spend telemetry,
    /// the continuous share allocation, and the fleet-wide GB-6 alert latches.
    pub budgets: Arc<FleetBudgets>,
    /// GB-6: where alerts raised at the control plane's enforcement point go.
    /// Pluggable; defaults to the structured log sink.
    pub alerts: Arc<dyn AlertSink>,
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
        Self::with_alert_sink(fleet, store, tokens, Arc::new(LogAlertSink))
    }

    /// Build a control plane with a custom GB-6 alert sink (the demo/tests wire
    /// a webhook-shaped or capturing sink; production wires a pager/bus).
    pub fn with_alert_sink(
        fleet: Arc<Fleet>,
        store: Arc<RuntimeStore>,
        tokens: Arc<JoinTokens>,
        alerts: Arc<dyn AlertSink>,
    ) -> Arc<ControlPlane> {
        Arc::new(ControlPlane {
            fleet,
            store,
            tokens,
            sessions: Sessions::default(),
            budgets: Arc::new(FleetBudgets::new()),
            alerts,
        })
    }

    /// Build the GB-5 `ShareGrant` this node currently holds (its rebalanced
    /// slices of every capped value), or `None` when there is nothing to grant.
    pub(crate) fn share_grant_for(&self, node_id: &str) -> Option<ShareGrant> {
        let allocations = self.budgets.shares_for(node_id);
        if allocations.is_empty() {
            return None;
        }
        let shares = allocations
            .into_iter()
            .map(|a| BudgetShare {
                attribution_key: a.id.key,
                attribution_value: a.id.value,
                cap_tokens: a.cap,
                tokens: a.share,
            })
            .collect();
        Some(ShareGrant { shares })
    }

    /// Ingest a node's `UsageReport` into the fleet ledger and emit every GB-6
    /// alert the fleet-wide crossing newly triggers, at the point of ingestion.
    fn ingest_usage(&self, node_id: &str, usage: &UsageReport) {
        for s in &usage.spenders {
            let id = CapId::new(&s.attribution_key, &s.attribution_value);
            for alert in self
                .budgets
                .report_spend(node_id, &id, s.cap_tokens, s.tokens)
            {
                info!(
                    "[gb6] control-plane fleet-wide alert from usage telemetry: {alert}"
                );
                self.alerts.emit(&alert);
            }
        }
    }

    /// Push a snapshot to one node and register a pending-ack waiter under a
    /// UNIQUE push-id, returning that id plus the receiver the caller awaits.
    /// Returns `None` if the node's stream is gone (its result becomes
    /// `Silent`). The push-id lets a caller clear exactly its own pending slot
    /// on timeout, without disturbing a concurrent push to the same node.
    pub(crate) async fn push_and_await(
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


    pub(crate) async fn clear_pending(&self, node_id: &str, push_id: u64) {
        if let Some(session) = self.sessions.0.lock().await.get_mut(node_id) {
            session.pending.remove(&push_id);
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


pub(crate) fn short(hash: &str) -> &str {
    &hash[..12.min(hash.len())]
}

pub(crate) fn now_unix() -> i64 {
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
        this.store.connect(&node_id, labels.clone());
        this.sessions.insert(&node_id, tx.clone()).await;

        // Push the node its first snapshot immediately (bootstrap). It is the
        // node's OWN per-node desired render (GatewaySet-stamped when its labels
        // match one), so a freshly-joined matching node picks up the stamped
        // config on its very first render — never the unstamped fleet-wide bytes
        // it would then have to be healed off of (docs/02 GatewaySets).
        let rendered = this.fleet.desired_for(&labels);
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
                    Some(client_message::Kind::Usage(usage)) => {
                        // GB-5: fold this node's observed spend into the fleet
                        // ledger (fires GB-6 alerts on a fleet-wide crossing),
                        // then push back its freshly-rebalanced shares so a hot
                        // node's slice grows continuously from telemetry.
                        cp.ingest_usage(&node_for_loop, &usage);
                        if let Some(grant) = cp.share_grant_for(&node_for_loop) {
                            let _ = tx.send(Ok(ServerMessage::share_grant(grant))).await;
                        }
                    }
                    Some(client_message::Kind::SyncCheck(check)) => {
                        // GB-5: the synchronous ~90% escalation. Record the
                        // near-limit spend the node reports (so the rebalance
                        // sees it), then reply with a fresh grant — the node is
                        // asking whether it may spend past its current share.
                        cp.ingest_usage(
                            &node_for_loop,
                            &UsageReport { spenders: check.spenders.clone() },
                        );
                        let grant = cp
                            .share_grant_for(&node_for_loop)
                            .unwrap_or(ShareGrant { shares: Vec::new() });
                        info!(
                            "[gb5] node {:?} escalated (>=90% share); regranting {} share(s)",
                            node_for_loop,
                            grant.shares.len()
                        );
                        let _ = tx.send(Ok(ServerMessage::share_grant(grant))).await;
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
