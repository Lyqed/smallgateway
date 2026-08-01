//! Hot reload (Phase 1, milestone 2): one reload path, two triggers.
//!
//! SIGHUP and the poll-based file watcher both funnel into
//! [`Reloader::reload`], which enforces the doc-03 semantics:
//!
//! - **No-op on identical content**: the file's hash matching the active
//!   snapshot's hash short-circuits at debug level — touching the file is
//!   not a config change.
//! - **NACK keeps old**: a file that fails validation is REJECTED loudly
//!   (the precise errors plus the still-active version) and the old
//!   snapshot keeps serving — divergence surfaced, never silent
//!   (docs/03-hot-swap.md, limitation 1).
//! - **Atomic swap, drain on the old**: a successful render replaces the
//!   shared `Arc<Snapshot>`; requests already bound to the old snapshot
//!   hold their own Arc until their stream finishes, so the old version
//!   stays resident until its last in-flight stream drops it — Rust
//!   refcounting made explicit and tested here, not accidental
//!   (docs/03-hot-swap.md, limitation 2).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use log::{debug, error, info};

use gateway_core::config::ConfigError;
use gateway_core::snapshot::{content_hash, Renderer, Snapshot};

/// The one mutable cell in the process: the currently-active snapshot.
/// `load()` is the per-request bind — one guarded Arc clone, so a request
/// sees exactly one version, ever. `store()` is the swap — new requests
/// bind the new snapshot from the next `load()` on, and nothing else moves.
#[derive(Clone)]
pub struct SharedSnapshot(Arc<RwLock<Arc<Snapshot>>>);

impl SharedSnapshot {
    fn new(initial: Snapshot) -> SharedSnapshot {
        SharedSnapshot(Arc::new(RwLock::new(Arc::new(initial))))
    }

    /// Bind the current snapshot. The returned Arc pins the snapshot for as
    /// long as the caller holds it (the whole request, streaming included).
    pub fn load(&self) -> Arc<Snapshot> {
        self.0.read().expect("snapshot lock").clone()
    }

    fn store(&self, next: Snapshot) {
        *self.0.write().expect("snapshot lock") = Arc::new(next);
    }
}

/// What one reload attempt did — the log lines carry the same facts; tests
/// assert on this.
#[derive(Debug, PartialEq, Eq)]
pub enum ReloadOutcome {
    Swapped { old: u64, new: u64 },
    /// Content hash unchanged from the active snapshot.
    NoOp { active: u64 },
    /// Unreadable or invalid file: the old snapshot stays active (NACK).
    Rejected { active: u64 },
}

/// Owns the config path, the version-stamping renderer, and the shared
/// snapshot cell. Every trigger — SIGHUP, poll watcher, future control
/// plane — calls the same `reload`.
pub struct Reloader {
    shared: SharedSnapshot,
    renderer: Renderer,
    path: PathBuf,
    /// Serializes concurrent triggers (SIGHUP racing the watcher): without
    /// it two renders could stamp v2 and v3 and store them in the wrong
    /// order — a version regression the lock makes impossible.
    reload_lock: Mutex<()>,
}

impl Reloader {
    /// Initial render at startup: fail-fast, exactly like milestone 1 —
    /// a bad file never serves a request.
    pub fn bootstrap(path: PathBuf) -> Result<Reloader, ConfigError> {
        let renderer = Renderer::new();
        let initial = renderer.render_file(&path)?;
        Ok(Reloader {
            shared: SharedSnapshot::new(initial),
            renderer,
            path,
            reload_lock: Mutex::new(()),
        })
    }

    pub fn shared(&self) -> SharedSnapshot {
        self.shared.clone()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The single reload path. Reads the file once, no-ops on an identical
    /// hash, NACKs (keeping the old snapshot) on any failure, and swaps
    /// atomically on success.
    pub fn reload(&self, trigger: &str) -> ReloadOutcome {
        let _serialized = self.reload_lock.lock().expect("reload lock");
        let active = self.shared.load();

        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) => {
                error!(
                    "[reload] REJECTED (NACK, trigger={trigger}): cannot read {}: {e}; \
                     still serving cfg=v{} hash={}",
                    self.path.display(),
                    active.version,
                    active.short_hash(),
                );
                return ReloadOutcome::Rejected { active: active.version };
            }
        };

        if content_hash(&text) == active.content_hash {
            debug!(
                "[reload] no-op (trigger={trigger}): content hash {} unchanged; \
                 still cfg=v{}",
                active.short_hash(),
                active.version,
            );
            return ReloadOutcome::NoOp { active: active.version };
        }

        match self.renderer.render_text(&text, &self.path) {
            Ok(next) => {
                let (old, new) = (active.version, next.version);
                info!(
                    "[reload] swapped cfg=v{old} -> v{new} hash={} at_unix={} \
                     trigger={trigger}; in-flight streams drain on v{old}",
                    next.short_hash(),
                    now_unix(),
                );
                self.shared.store(next);
                ReloadOutcome::Swapped { old, new }
            }
            Err(e) => {
                // Loud by design: the precise validation errors AND the
                // version that keeps serving, in one place.
                error!(
                    "[reload] REJECTED (NACK, trigger={trigger}): new config invalid; \
                     still serving cfg=v{} hash={}\n{e}",
                    active.version,
                    active.short_hash(),
                );
                ReloadOutcome::Rejected { active: active.version }
            }
        }
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Trigger 1: SIGHUP. A dedicated thread runs a minimal current-thread
/// tokio runtime (tokio is already in the tree via pingora; pingora itself
/// listens for SIGINT/SIGTERM/SIGQUIT, never SIGHUP) and funnels every
/// hangup into the one reload path.
pub fn spawn_sighup_listener(reloader: Arc<Reloader>) {
    thread::Builder::new()
        .name("cfg-sighup".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("sighup runtime");
            rt.block_on(async {
                let mut hangup =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
                        .expect("install SIGHUP handler");
                while hangup.recv().await.is_some() {
                    reloader.reload("sighup");
                }
            });
        })
        .expect("spawn sighup listener");
}

/// Trigger 2: poll-based watcher. Cheap mtime stat every `interval`; an
/// observed change funnels into the same reload path, whose hash check
/// downgrades touch-without-change to a debug no-op. The last-seen mtime
/// advances even when the reload NACKs, so a bad file is rejected loudly
/// once per change, not once per tick.
pub fn spawn_poll_watcher(reloader: Arc<Reloader>, interval: Duration) {
    thread::Builder::new()
        .name("cfg-watch".to_string())
        .spawn(move || {
            let mut last_mtime = mtime_of(reloader.path());
            loop {
                thread::sleep(interval);
                let mtime = mtime_of(reloader.path());
                if mtime != last_mtime {
                    last_mtime = mtime;
                    reloader.reload("poll");
                }
            }
        })
        .expect("spawn poll watcher");
}

fn mtime_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Shared by the reload tests here and the per-request pinning test in
/// `proxy.rs`.
#[cfg(test)]
pub(crate) mod testutil {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A minimal valid config whose only variable is the pinned `env`
    /// value — the visible difference between "versions" in tests.
    pub fn valid_yaml(env: &str) -> String {
        format!(
            r#"
providers:
  openai-main:
    kind: openai
    upstream: {{ host: 127.0.0.1, port: 6190 }}
routes:
  - prefix: /openai
    provider: openai-main
    attribution:
      pinned: {{ env: {env} }}
rejections:
  missing_attribution:
    status: 428
    content_type: application/json
    body: '{{"error":"missing {{{{key}}}} on {{{{route}}}}"}}'
  unknown_route:
    status: 404
    content_type: application/json
    body: '{{"error":"no route for {{{{route}}}}"}}'
"#
        )
    }

    /// Config that fails validation with a precise, named error.
    pub fn invalid_yaml() -> String {
        valid_yaml("prod").replace("provider: openai-main", "provider: does-not-exist")
    }

    /// Unique temp config file per call; tests rewrite it to simulate
    /// operator edits.
    pub fn temp_cfg(text: &str) -> PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "gatewayd-test-{}-{}.yaml",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::write(&path, text).expect("write temp config");
        path
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::{invalid_yaml, temp_cfg, valid_yaml};
    use super::*;

    #[test]
    fn bootstrap_renders_v1_from_the_file() {
        let path = temp_cfg(&valid_yaml("prod"));
        let reloader = Reloader::bootstrap(path.clone()).unwrap();
        let snap = reloader.shared().load();
        assert_eq!(snap.version, 1);
        assert_eq!(snap.source, path);
        assert_eq!(snap.config.routes[0].attribution.pinned["env"], "prod");
    }

    #[test]
    fn bootstrap_fails_fast_on_an_invalid_file() {
        let path = temp_cfg(&invalid_yaml());
        assert!(matches!(
            Reloader::bootstrap(path),
            Err(ConfigError::Invalid(_))
        ));
    }

    #[test]
    fn changed_content_swaps_and_new_binds_see_the_new_version() {
        let path = temp_cfg(&valid_yaml("prod"));
        let reloader = Reloader::bootstrap(path.clone()).unwrap();
        std::fs::write(&path, valid_yaml("canary")).unwrap();

        assert_eq!(reloader.reload("test"), ReloadOutcome::Swapped { old: 1, new: 2 });

        let snap = reloader.shared().load();
        assert_eq!(snap.version, 2);
        assert_eq!(snap.config.routes[0].attribution.pinned["env"], "canary");
    }

    #[test]
    fn drain_semantics_old_snapshot_lives_until_its_last_holder_drops() {
        let path = temp_cfg(&valid_yaml("prod"));
        let reloader = Reloader::bootstrap(path.clone()).unwrap();

        // An in-flight request binds v1 at request start...
        let bound = reloader.shared().load();
        let old_alive = Arc::downgrade(&bound);
        assert_eq!(bound.version, 1);

        // ...then the operator swaps to v2 mid-stream.
        std::fs::write(&path, valid_yaml("canary")).unwrap();
        assert_eq!(reloader.reload("test"), ReloadOutcome::Swapped { old: 1, new: 2 });

        // The stream still sees v1, torn-read-free, while v2 serves new
        // binds — two versions live simultaneously (doc 03, limitation 2).
        assert_eq!(bound.version, 1);
        assert_eq!(bound.config.routes[0].attribution.pinned["env"], "prod");
        assert_eq!(reloader.shared().load().version, 2);
        assert!(old_alive.upgrade().is_some(), "old snapshot pinned by the stream");

        // The stream finishes: the last holder drops, and only then does
        // the old snapshot die. Explicit, not accidental.
        drop(bound);
        assert!(old_alive.upgrade().is_none(), "old snapshot freed on last drop");
    }

    #[test]
    fn invalid_file_is_nacked_and_the_old_snapshot_keeps_serving() {
        let path = temp_cfg(&valid_yaml("prod"));
        let reloader = Reloader::bootstrap(path.clone()).unwrap();

        std::fs::write(&path, invalid_yaml()).unwrap();
        assert_eq!(reloader.reload("test"), ReloadOutcome::Rejected { active: 1 });
        assert_eq!(reloader.shared().load().version, 1, "old snapshot still active");

        // Fixing the file swaps to v2, not v3: the NACK consumed no version.
        std::fs::write(&path, valid_yaml("canary")).unwrap();
        assert_eq!(reloader.reload("test"), ReloadOutcome::Swapped { old: 1, new: 2 });
    }

    #[test]
    fn unreadable_file_is_nacked_and_the_old_snapshot_keeps_serving() {
        let path = temp_cfg(&valid_yaml("prod"));
        let reloader = Reloader::bootstrap(path.clone()).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(reloader.reload("test"), ReloadOutcome::Rejected { active: 1 });
        assert_eq!(reloader.shared().load().version, 1);
    }

    #[test]
    fn identical_content_is_a_no_op() {
        let path = temp_cfg(&valid_yaml("prod"));
        let reloader = Reloader::bootstrap(path.clone()).unwrap();

        // Rewrite the same bytes (fresh mtime, same hash): no new version.
        std::fs::write(&path, valid_yaml("prod")).unwrap();
        assert_eq!(reloader.reload("test"), ReloadOutcome::NoOp { active: 1 });
        assert_eq!(reloader.shared().load().version, 1);

        // And a real change afterwards still gets the next version, v2.
        std::fs::write(&path, valid_yaml("canary")).unwrap();
        assert_eq!(reloader.reload("test"), ReloadOutcome::Swapped { old: 1, new: 2 });
    }
}
