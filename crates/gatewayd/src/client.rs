//! Control-plane client mode (docs/07-control-plane.md, "gatewayd as a
//! control-plane client").
//!
//! Instead of reading a local file, gatewayd dials a gatewayctl endpoint with a
//! join token, holds one long-lived bidirectional gRPC stream open, and binds
//! each pushed `RenderedSnapshot` through the EXISTING `SharedSnapshot` /
//! `Reloader` machinery. The hot-reload drain semantics carry over UNCHANGED:
//! in-flight streams finish on their bound version, a snapshot that fails local
//! validation is NACKed and the old one keeps serving, and an identical re-push
//! is a hash no-op — all of that lives in `Reloader::reload_from_text`, which
//! the control plane is simply one more trigger for.
//!
//! The stream is xDS-shaped and dial-OUT: the data plane dials the control
//! plane and holds the stream, so a DMZ box or edge node needs no inbound path
//! (docs/07, "The stream").
//!
//! ## Bootstrap ordering
//!
//! pingora needs a `SharedSnapshot` at `Gateway::new` before it starts serving,
//! but in control-plane mode the first snapshot arrives asynchronously over the
//! stream. So [`connect_and_bootstrap`] blocks until the first `Push` has been
//! received and bound (or the dial/first-push fails, which is fatal — a node
//! with no config never serves), then returns the `SharedSnapshot` and spawns
//! the background loop that binds every subsequent push and heartbeats
//! `Status`.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use log::{error, info, warn};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use gateway_proto::{server_message, Ack, ClientMessage, FleetServiceClient, Hello, Nack, RenderedSnapshot, Status};

use crate::reload::{ReloadOutcome, Reloader, SharedSnapshot};

/// How often the node heartbeats its observed reality to the control plane.
const STATUS_INTERVAL: Duration = Duration::from_secs(10);

/// A synthetic source label for pushed snapshots (the `source` field on the
/// node's `Snapshot`, analogous to a file path in file mode).
const PUSH_SOURCE: &str = "<control-plane>";

/// Reconnect backoff bounds. After the stream drops (or the control plane goes
/// away), the node keeps serving its last bound snapshot and re-dials with
/// exponential backoff between these bounds until the control plane returns.
const RECONNECT_BACKOFF_MIN: Duration = Duration::from_millis(500);
const RECONNECT_BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Dial the control plane, join, receive and bind the first snapshot, and spawn
/// the background stream loop. Blocks until the first push is bound or fails.
///
/// Returns the `SharedSnapshot` pingora binds per request. The background loop
/// runs on its own multi-threaded runtime in a dedicated thread, so it lives
/// alongside pingora's blocking `run_forever` without contending for it.
///
/// After the first bootstrap the loop SUPERVISES the stream: if it drops (a
/// transient network fault, or the control plane restarting), the already-bound
/// `SharedSnapshot` keeps serving traffic unchanged (control-plane downtime
/// freezes the fleet at its last-acked version; it does not stop traffic —
/// docs/07, "the control plane is not a SPOF for serving") and the node
/// re-dials with backoff, re-joining on its established identity and resuming
/// pushes. The node neither crashes nor goes permanently quiet on a stream
/// loss; it serves-last-AND-reconnects.
pub fn connect_and_bootstrap(
    endpoint: String,
    node_id: String,
    join_token: String,
) -> Result<SharedSnapshot, String> {
    // Channel to hand the bootstrapped SharedSnapshot back to the caller
    // (the main thread) synchronously.
    let (ready_tx, ready_rx) = std_mpsc::channel::<Result<SharedSnapshot, String>>();

    thread::Builder::new()
        .name("cp-client".to_string())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("client runtime: {e}")));
                    return;
                }
            };
            rt.block_on(async move {
                supervise(endpoint, node_id, join_token, ready_tx).await;
            });
        })
        .map_err(|e| format!("spawn control-plane client thread: {e}"))?;

    // Block until bootstrap succeeds or fails.
    ready_rx
        .recv()
        .map_err(|_| "control-plane client exited before first snapshot".to_string())?
}

/// The supervisor loop: bootstrap once, then re-dial forever on stream loss.
///
/// The `Reloader` (and thus the `SharedSnapshot` pingora holds) is created ONCE
/// during the first successful connection and reused across every reconnect, so
/// pushes on a re-dialed stream bind into the SAME live snapshot cell that has
/// been serving traffic the whole time. `ready_tx` fires exactly once, with the
/// first bootstrap's result.
async fn supervise(
    endpoint: String,
    node_id: String,
    join_token: String,
    ready_tx: std_mpsc::Sender<Result<SharedSnapshot, String>>,
) {
    // The last fleet_version the control plane delivered and this node bound.
    // Advertised as `current_fleet_version` on every (re)connect so the control
    // plane can skip a redelivery the node already has. Shared across reconnects
    // so it survives a stream drop (the field's documented meaning: "the node's
    // last-known fleet_version"). 0 until the first push binds.
    let last_fleet_version = Arc::new(AtomicU64::new(0));

    // First connection: dial, join, bind the first push. A failure here is
    // fatal (a node with no config never serves) and reported to the caller.
    let reloader = match connect_once(&endpoint, &node_id, &join_token, None, &last_fleet_version)
        .await
    {
        Ok(reloader) => reloader,
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
        }
    };
    if ready_tx.send(Ok(reloader.shared())).is_err() {
        // The caller went away; nothing to serve.
        return;
    }

    // Supervised reconnect loop. The snapshot keeps serving throughout; we only
    // re-establish the control channel so future pushes are received again.
    let mut backoff = RECONNECT_BACKOFF_MIN;
    loop {
        tokio::time::sleep(backoff).await;
        info!(
            "[cp-client] node {node_id:?} re-dialing {endpoint} (still serving cfg=v{} \
             hash={} meanwhile)",
            reloader.shared().load().version,
            short(&reloader.shared().load().content_hash),
        );
        match connect_once(
            &endpoint,
            &node_id,
            &join_token,
            Some(reloader.clone()),
            &last_fleet_version,
        )
        .await
        {
            Ok(_) => {
                // The stream ran and then ended; reset backoff and retry.
                backoff = RECONNECT_BACKOFF_MIN;
            }
            Err(e) => {
                warn!(
                    "[cp-client] node {node_id:?} reconnect to {endpoint} failed: {e}; \
                     still serving last snapshot, retrying in {:?}",
                    backoff
                );
                backoff = (backoff * 2).min(RECONNECT_BACKOFF_MAX);
            }
        }
    }
}

/// Dial once, join, and run the stream until it ends.
///
/// `existing` is `None` on the very first connection (the `Reloader` is created
/// from the first push and returned) and `Some(reloader)` on every reconnect
/// (pushes bind into the existing snapshot cell; the returned reloader is the
/// same one). Returns `Err` if the dial or join fails, or if the first-ever
/// push fails to bind (fatal only on the first connection — the supervisor
/// treats a reconnect error as retryable).
async fn connect_once(
    endpoint: &str,
    node_id: &str,
    join_token: &str,
    existing: Option<Arc<Reloader>>,
    last_fleet_version: &Arc<AtomicU64>,
) -> Result<Arc<Reloader>, String> {
    let mut client = FleetServiceClient::connect(endpoint.to_string())
        .await
        .map_err(|e| format!("dial {endpoint}: {e}"))?;

    // Outbound channel: Hello, then Ack/Nack/Status. On reconnect the node
    // presents `current_fleet_version` — the last fleet_version it bound — so
    // the control plane can skip a redelivery it already has (an identical
    // re-push is a hash no-op anyway).
    let (out_tx, out_rx) = mpsc::channel::<ClientMessage>(16);
    let hello = ClientMessage::hello(Hello {
        node_id: node_id.to_string(),
        join_token: join_token.to_string(),
        labels: Default::default(),
        current_fleet_version: last_fleet_version.load(Ordering::Relaxed),
    });
    if out_tx.send(hello).await.is_err() {
        return Err("outbound channel closed before Hello".into());
    }

    let outbound = ReceiverStream::new(out_rx);
    let response = client
        .session(outbound)
        .await
        // A rejected join (bad/replayed token) surfaces here as an
        // Unauthenticated status — loud, and fatal on the first connection.
        .map_err(|e| format!("join rejected by control plane: {e}"))?;
    let mut inbound = response.into_inner();

    // Establish (or reuse) the Reloader. On the first connection we block on the
    // first push to build it; on reconnect the snapshot already exists.
    let reloader = match existing {
        Some(reloader) => {
            info!("[cp-client] node {node_id:?} rejoined {endpoint}; resuming pushes");
            reloader
        }
        None => {
            info!("[cp-client] node {node_id:?} joined {endpoint}; awaiting first snapshot");
            let (reloader, first_version) =
                wait_first_push(&mut inbound, &out_tx, node_id).await?;
            last_fleet_version.store(first_version, Ordering::Relaxed);
            Arc::new(reloader)
        }
    };

    // Heartbeat task for THIS stream: periodic Status carrying the observed
    // render hash. It stops when this stream's outbound channel closes.
    spawn_heartbeat(out_tx.clone(), reloader.clone(), node_id.to_string());

    // Steady state: bind every subsequent push through the unchanged reload
    // path and ack/nack it, until the stream ends (then the supervisor re-dials).
    while let Some(msg) = inbound.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                warn!("[cp-client] stream error: {e}; supervisor will re-dial");
                break;
            }
        };
        match msg.kind {
            Some(server_message::Kind::Push(snap)) => {
                // Record the delivered fleet_version so a later reconnect can
                // advertise it (skip a redelivery). We store it whether the bind
                // swaps or no-ops; a NACK leaves the last successfully-bound
                // version in place, which is the honest "what I'm running".
                let delivered = snap.fleet_version;
                let acked = bind_push(&reloader, &out_tx, node_id, snap, "push").await;
                if acked {
                    last_fleet_version.store(delivered, Ordering::Relaxed);
                }
            }
            Some(server_message::Kind::AckOfStatus(_)) => { /* liveness only */ }
            None => warn!("[cp-client] empty server message"),
        }
    }
    info!("[cp-client] stream to {endpoint} ended; node keeps serving its last snapshot");
    Ok(reloader)
}

/// Await the first push and bootstrap the Reloader from it. A first push that
/// fails validation is NACKed and is fatal for bootstrap (a node with no valid
/// config cannot serve) — the operator sees the NACK on the control plane.
async fn wait_first_push(
    inbound: &mut tonic::Streaming<gateway_proto::ServerMessage>,
    out_tx: &mpsc::Sender<ClientMessage>,
    node_id: &str,
) -> Result<(Reloader, u64), String> {
    loop {
        let msg = inbound
            .next()
            .await
            .ok_or_else(|| "stream closed before first snapshot".to_string())?
            .map_err(|e| format!("stream error before first snapshot: {e}"))?;
        match msg.kind {
            Some(server_message::Kind::Push(snap)) => {
                let text = match std::str::from_utf8(&snap.config) {
                    Ok(t) => t.to_owned(),
                    Err(e) => {
                        let reason = format!("pushed config is not utf-8: {e}");
                        nack(out_tx, snap.fleet_version, &snap.render_hash, &reason).await;
                        return Err(reason);
                    }
                };
                let source = Path::new(PUSH_SOURCE);
                match Reloader::bootstrap_from_text(&text, source) {
                    Ok(reloader) => {
                        let snap_v = reloader.shared().load();
                        // The ACK echoes the hash WE recomputed of the bytes we
                        // actually bound (`Snapshot.content_hash`, SHA-256 of the
                        // pushed `config`), NOT the server's advertised
                        // `render_hash`. This makes the control plane's
                        // WrongHash check an INDEPENDENT verification: if it ever
                        // advertised a render_hash inconsistent with the config
                        // bytes it sent (a bug or tampering), the hash we echo
                        // will not match and the wave diverges — docs/07 line 71,
                        // "hashes the incoming config bytes ... before anything
                        // else".
                        let bound_hash = snap_v.content_hash.clone();
                        let hash_note = hash_mismatch_note(&snap.render_hash, &bound_hash);
                        info!(
                            "[cp-client] node {node_id:?} bound first snapshot fleet_version=v{} \
                             advertised_render_hash={} bound_hash={} -> local cfg=v{}{hash_note}",
                            snap.fleet_version,
                            short(&snap.render_hash),
                            short(&bound_hash),
                            snap_v.version,
                        );
                        ack(out_tx, snap.fleet_version, &bound_hash).await;
                        return Ok((reloader, snap.fleet_version));
                    }
                    Err(e) => {
                        let reason = format!("{e}");
                        warn!(
                            "[cp-client] node {node_id:?} NACKed first snapshot v{}: {reason}",
                            snap.fleet_version
                        );
                        nack(out_tx, snap.fleet_version, &snap.render_hash, &reason).await;
                        return Err(format!("first snapshot rejected: {reason}"));
                    }
                }
            }
            // Ignore any liveness frame that precedes the first push.
            Some(server_message::Kind::AckOfStatus(_)) => continue,
            None => continue,
        }
    }
}

/// Bind one pushed snapshot through `Reloader::reload_from_text` and ack/nack
/// per the outcome (docs/07, "ACK/NACK semantics, extended"). Returns `true` if
/// the node ACKed (swapped or no-op) and `false` on a NACK — the caller uses
/// this to advance its last-known fleet_version only on a successful bind.
async fn bind_push(
    reloader: &Arc<Reloader>,
    out_tx: &mpsc::Sender<ClientMessage>,
    node_id: &str,
    snap: RenderedSnapshot,
    trigger: &str,
) -> bool {
    let text = match std::str::from_utf8(&snap.config) {
        Ok(t) => t,
        Err(e) => {
            let reason = format!("pushed config is not utf-8: {e}");
            warn!("[cp-client] node {node_id:?} NACKed v{}: {reason}", snap.fleet_version);
            nack(out_tx, snap.fleet_version, &snap.render_hash, &reason).await;
            return false;
        }
    };
    let outcome = reloader.reload_from_text(text, Path::new(PUSH_SOURCE), trigger);
    // The hash the node ACKs with is the SHA-256 of the bytes it actually bound
    // (its active `Snapshot.content_hash`), recomputed locally — never the
    // server's advertised `render_hash`. On a mismatch the control plane's
    // WrongHash divergence fires, catching an inconsistent advertisement (bug or
    // tampering) that echoing the advertised value could never detect.
    let bound_hash = reloader.shared().load().content_hash.clone();
    let hash_note = hash_mismatch_note(&snap.render_hash, &bound_hash);
    match outcome {
        ReloadOutcome::Swapped { old, new } => {
            info!(
                "[cp-client] node {node_id:?} ACK v{} (swapped local cfg v{old}->v{new}, \
                 bound_hash={}){hash_note}",
                snap.fleet_version,
                short(&bound_hash)
            );
            ack(out_tx, snap.fleet_version, &bound_hash).await;
            true
        }
        ReloadOutcome::NoOp { active } => {
            // A no-op is an ACK: the node confirms it is already at this render.
            info!(
                "[cp-client] node {node_id:?} ACK v{} (no-op; already at bound_hash={}, \
                 local cfg=v{active}){hash_note}",
                snap.fleet_version,
                short(&bound_hash)
            );
            ack(out_tx, snap.fleet_version, &bound_hash).await;
            true
        }
        ReloadOutcome::Rejected { active } => {
            let reason = format!(
                "pushed config failed local validation; still serving local cfg=v{active}"
            );
            warn!(
                "[cp-client] node {node_id:?} NACK v{}: {reason}",
                snap.fleet_version
            );
            nack(out_tx, snap.fleet_version, &snap.render_hash, &reason).await;
            false
        }
    }
}

fn spawn_heartbeat(out_tx: mpsc::Sender<ClientMessage>, reloader: Arc<Reloader>, node_id: String) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(STATUS_INTERVAL);
        loop {
            ticker.tick().await;
            let active = reloader.shared().load();
            let status = ClientMessage::status(Status {
                observed_render_hash: active.content_hash.clone(),
                health: "ok".to_string(),
                in_flight_streams: 0,
            });
            if out_tx.send(status).await.is_err() {
                info!("[cp-client] node {node_id:?} heartbeat stopped (stream gone)");
                break;
            }
        }
    });
}

async fn ack(out_tx: &mpsc::Sender<ClientMessage>, fleet_version: u64, render_hash: &str) {
    let _ = out_tx
        .send(ClientMessage::ack(Ack {
            fleet_version,
            render_hash: render_hash.to_string(),
        }))
        .await;
}

async fn nack(out_tx: &mpsc::Sender<ClientMessage>, fleet_version: u64, render_hash: &str, reason: &str) {
    let _ = out_tx
        .send(ClientMessage::nack(Nack {
            fleet_version,
            render_hash: render_hash.to_string(),
            reason: reason.to_string(),
        }))
        .await;
    let _ = error_once(reason);
}

/// A tiny helper so a NACK reason is always visible even at info log levels.
fn error_once(reason: &str) -> bool {
    error!("[cp-client] NACK reason: {reason}");
    true
}

fn short(hash: &str) -> &str {
    &hash[..12.min(hash.len())]
}

/// A loud, non-fatal note when the control plane's advertised `render_hash`
/// disagrees with the hash the node recomputed of the bytes it bound. The node
/// still binds and ACKs (its validation passed), but it ACKs with its OWN hash,
/// so the control plane's WrongHash divergence catches the inconsistency. This
/// surfaces it locally too, never silent (docs/07: divergence is never silent).
fn hash_mismatch_note(advertised: &str, bound: &str) -> String {
    if advertised == bound {
        String::new()
    } else {
        warn!(
            "[cp-client] HASH MISMATCH: control plane advertised render_hash={} but the bytes \
             it sent hash to {}; ACKing with the bytes' true hash (control plane will diverge)",
            short(advertised),
            short(bound),
        );
        format!(
            " [!] advertised_render_hash={} != bound_hash={}",
            short(advertised),
            short(bound)
        )
    }
}
