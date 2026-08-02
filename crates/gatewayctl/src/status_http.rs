//! The status query surface: a read-only HTTP/JSON endpoint exposing the
//! fleet's REAL per-node state — who is connected, what version/hash each
//! node ACKed, what it reports actually running — plus the currently
//! applied render identity.
//!
//! This is the read path the Kubernetes operator (and any dashboardless
//! operator with `curl`) uses to make `Ready` mean "the fleet COMMITTED
//! this config", not "the pods look healthy": deploy/README.md documents
//! the rollout window where the CR status shows a new input hash while the
//! fleet still serves the old config; gating on these acks closes it.
//!
//! Deliberately minimal (defer, no bloat): a std TcpListener thread and
//! hand-assembled JSON via serde_json — the same idiom as the mock
//! servers, no web framework. GET /status is the only route. Read-only:
//! nothing here mutates fleet state, so exposing it adds no control
//! surface. Binds loopback by default; the k8s Deployment overrides to
//! 0.0.0.0 inside the pod network.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use log::{error, info};
use serde_json::json;

use crate::fleet::Fleet;
use crate::store::RuntimeStore;

/// Spawn the status listener on `addr`. Returns after binding (so a bind
/// failure is loud at startup, not on first query); serving runs on a
/// detached thread for the process's life.
pub fn spawn(addr: &str, fleet: Arc<Fleet>, store: Arc<RuntimeStore>) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    info!("[status] listening on {addr} (GET /status)");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    if let Err(e) = handle(s, &fleet, &store) {
                        error!("[status] connection error: {e}");
                    }
                }
                Err(e) => error!("[status] accept error: {e}"),
            }
        }
    });
    Ok(())
}

fn handle(stream: TcpStream, fleet: &Fleet, store: &RuntimeStore) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(());
    }
    // Drain headers; the route is all we need.
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim_end().is_empty() {
            break;
        }
    }
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if method != "GET" || target != "/status" {
        return respond(stream, 404, "{\"error\":\"GET /status is the only route\"}");
    }
    let applied = fleet.applied();
    let body =
        status_json(&applied.render_hash, &applied.source_commit, fleet.committed_version(), store)
            .to_string();
    respond(stream, 200, &body)
}

/// The status document. Every field is REAL observed state: `applied` is
/// the render the control plane currently distributes; `nodes` is what
/// each node last said, verbatim from the runtime store. Takes the raw
/// pieces (not the Fleet) so it is trivially unit-testable.
pub fn status_json(
    render_hash: &str,
    source_commit: &str,
    committed_version: u64,
    store: &RuntimeStore,
) -> serde_json::Value {
    let nodes: Vec<serde_json::Value> = store
        .all()
        .into_iter()
        .map(|n| {
            json!({
                "node_id": n.node_id,
                "labels": n.labels,
                "connected": n.connected,
                "acked_version": n.last_acked_version,
                "acked_hash": n.last_acked_hash,
                "observed_hash": n.observed_hash,
                "health": n.health,
                "last_seen": n.last_seen,
                "consecutive_nacks": n.consecutive_nacks,
                "last_nack": n.last_nack.map(|(v, reason)| json!({
                    "version": v,
                    "reason": reason,
                })),
            })
        })
        .collect();
    json!({
        "applied": {
            "render_hash": render_hash,
            "source_commit": source_commit,
        },
        "committed_version": committed_version,
        "nodes": nodes,
    })
}

fn respond(mut w: TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = if status == 200 { "OK" } else { "Not Found" };
    write!(
        w,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    )?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn status_json_reports_real_node_state() {
        let store = RuntimeStore::new();
        let mut labels = BTreeMap::new();
        labels.insert("region".to_string(), "eu".to_string());
        store.connect("n1", labels);
        store.record_ack("n1", 3, "hash-3");
        store.record_status("n1", "hash-3", "serving");
        store.connect("n2", BTreeMap::new());
        store.record_nack("n2", 3, "validation failed");

        let v = status_json("rh-abc", "commit-1", 3, &store);
        let nodes = v["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 2);
        let n1 = nodes.iter().find(|n| n["node_id"] == "n1").unwrap();
        assert_eq!(n1["acked_version"], 3);
        assert_eq!(n1["acked_hash"], "hash-3");
        assert_eq!(n1["labels"]["region"], "eu");
        assert_eq!(n1["connected"], true);
        let n2 = nodes.iter().find(|n| n["node_id"] == "n2").unwrap();
        assert_eq!(n2["last_nack"]["reason"], "validation failed");
        assert!(v["applied"]["render_hash"].is_string());
    }
}
