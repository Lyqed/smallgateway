//! Runtime state: connected nodes, their acked versions, and health.
//!
//! docs/07-control-plane.md, "Postgres for runtime state only, never truth":
//! the control plane's runtime state holds *observed reality* and nothing
//! *desired*. Desired state is always Git (the config repo); this store records
//! only what happened — which nodes are connected, what version each last
//! acked/nacked, and their last-seen health.
//!
//! **M1 uses an in-memory store; Postgres replaces it later and is NEVER
//! truth.** Every field here is derivable or re-derivable from Git plus the
//! stream: wipe it and the fleet keeps serving (nodes run their local
//! snapshots), and the next round of `Status` heartbeats rebuilds it. There is
//! deliberately no column — and here, no field — for desired state: the schema
//! is designed so there is nowhere to write "this node SHOULD run version N".
//! That is recomputed from the current applied render every time, never stored.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// What the control plane last heard from one node. Purely observed — no
/// desired-state field exists by construction.
#[derive(Debug, Clone)]
pub struct NodeState {
    pub node_id: String,
    pub labels: BTreeMap<String, String>,
    /// The last version the node ACKed (accepted + swapped, or no-opped).
    pub last_acked_version: Option<u64>,
    /// The render_hash echoed on that last ack — lets the control plane detect
    /// a node that acked the wrong bytes, not just the wrong number.
    pub last_acked_hash: Option<String>,
    /// The most recent NACK, if any (version, reason). Surfaced, never hidden.
    pub last_nack: Option<(u64, String)>,
    /// The render_hash the node reports it is ACTUALLY running (from `Status`).
    pub observed_hash: Option<String>,
    pub health: Option<String>,
    /// Unix seconds of the last message of any kind from this node.
    pub last_seen: u64,
    pub connected: bool,
}

impl NodeState {
    fn new(node_id: String, labels: BTreeMap<String, String>) -> NodeState {
        NodeState {
            node_id,
            labels,
            last_acked_version: None,
            last_acked_hash: None,
            last_nack: None,
            observed_hash: None,
            health: None,
            last_seen: now_unix(),
            connected: true,
        }
    }
}

/// The in-memory runtime-state store. `Mutex<BTreeMap>` is plenty for M1's
/// scale; the interface is what Postgres will implement later, so callers do
/// not encode the storage choice.
#[derive(Default)]
pub struct RuntimeStore {
    nodes: Mutex<BTreeMap<String, NodeState>>,
}

impl RuntimeStore {
    pub fn new() -> RuntimeStore {
        RuntimeStore::default()
    }

    /// Register (or re-register on reconnect) a node from its `Hello`.
    pub fn connect(&self, node_id: &str, labels: BTreeMap<String, String>) {
        let mut nodes = self.lock();
        nodes
            .entry(node_id.to_string())
            .and_modify(|n| {
                n.labels = labels.clone();
                n.connected = true;
                n.last_seen = now_unix();
            })
            .or_insert_with(|| NodeState::new(node_id.to_string(), labels));
    }

    /// Mark a node's stream as gone. Its last-acked version is retained: a
    /// disconnected node is *unknown*, not reset — history is not truth but it
    /// is still worth keeping (docs/07: unknown is a third state).
    pub fn disconnect(&self, node_id: &str) {
        if let Some(n) = self.lock().get_mut(node_id) {
            n.connected = false;
            n.last_seen = now_unix();
        }
    }

    /// Record an ACK: the node accepted and is at `version`/`hash`.
    pub fn record_ack(&self, node_id: &str, version: u64, hash: &str) {
        if let Some(n) = self.lock().get_mut(node_id) {
            n.last_acked_version = Some(version);
            n.last_acked_hash = Some(hash.to_string());
            n.last_nack = None;
            n.last_seen = now_unix();
        }
    }

    /// Record a NACK: the node rejected `version`; it keeps serving its prior
    /// version. Loud by design — the reason is retained for surfacing.
    pub fn record_nack(&self, node_id: &str, version: u64, reason: &str) {
        if let Some(n) = self.lock().get_mut(node_id) {
            n.last_nack = Some((version, reason.to_string()));
            n.last_seen = now_unix();
        }
    }

    /// Record a `Status` heartbeat's observed reality.
    pub fn record_status(&self, node_id: &str, observed_hash: &str, health: &str) {
        if let Some(n) = self.lock().get_mut(node_id) {
            n.observed_hash = Some(observed_hash.to_string());
            n.health = Some(health.to_string());
            n.last_seen = now_unix();
        }
    }

    /// Snapshot of one node's state.
    pub fn get(&self, node_id: &str) -> Option<NodeState> {
        self.lock().get(node_id).cloned()
    }

    /// Snapshot of every node's state, sorted by id — the queryable,
    /// alertable fleet view (docs/07: divergence is queryable, never "shrug").
    pub fn all(&self) -> Vec<NodeState> {
        self.lock().values().cloned().collect()
    }

    /// The ids of currently-connected nodes.
    pub fn connected_ids(&self) -> Vec<String> {
        self.lock()
            .values()
            .filter(|n| n.connected)
            .map(|n| n.node_id.clone())
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, NodeState>> {
        self.nodes.lock().expect("runtime store lock")
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> BTreeMap<String, String> {
        BTreeMap::from([("region".to_string(), "fra".to_string())])
    }

    #[test]
    fn connect_then_ack_records_observed_version() {
        let store = RuntimeStore::new();
        store.connect("n1", labels());
        store.record_ack("n1", 2, "hash2");
        let n = store.get("n1").unwrap();
        assert_eq!(n.last_acked_version, Some(2));
        assert_eq!(n.last_acked_hash.as_deref(), Some("hash2"));
        assert!(n.connected);
        assert_eq!(n.labels["region"], "fra");
    }

    #[test]
    fn nack_is_retained_and_surfaced() {
        let store = RuntimeStore::new();
        store.connect("n1", labels());
        store.record_nack("n1", 3, "unknown provider foo");
        let n = store.get("n1").unwrap();
        assert_eq!(n.last_nack, Some((3, "unknown provider foo".to_string())));
    }

    #[test]
    fn a_later_ack_clears_a_prior_nack() {
        let store = RuntimeStore::new();
        store.connect("n1", labels());
        store.record_nack("n1", 3, "boom");
        store.record_ack("n1", 3, "hash3");
        assert!(store.get("n1").unwrap().last_nack.is_none());
    }

    #[test]
    fn disconnect_keeps_last_acked_but_flips_connected() {
        let store = RuntimeStore::new();
        store.connect("n1", labels());
        store.record_ack("n1", 1, "h1");
        store.disconnect("n1");
        let n = store.get("n1").unwrap();
        assert!(!n.connected);
        assert_eq!(n.last_acked_version, Some(1), "history retained");
        assert!(store.connected_ids().is_empty());
    }

    #[test]
    fn all_is_sorted_and_status_records_observed_reality() {
        let store = RuntimeStore::new();
        store.connect("z", labels());
        store.connect("a", labels());
        store.record_status("a", "obs-a", "ok");
        let all = store.all();
        assert_eq!(all[0].node_id, "a");
        assert_eq!(all[1].node_id, "z");
        assert_eq!(all[0].observed_hash.as_deref(), Some("obs-a"));
    }
}
