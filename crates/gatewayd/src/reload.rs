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

/// Owns the config source, the version-stamping renderer, and the shared
/// snapshot cell. Every trigger — SIGHUP, poll watcher, control plane — calls
/// the same reload logic (`reload` reads the file; `reload_from_text` binds
/// bytes handed in over the wire), so the doc-03 semantics (no-op on identical
/// hash, NACK-keeps-old, atomic swap, drain-on-old) are identical whether the
/// new config came from disk or from a control-plane `Push`.
pub struct Reloader {
    shared: SharedSnapshot,
    renderer: Renderer,
    /// The file source, when in file mode. `None` in control-plane mode: there
    /// is no local file, only pushed snapshots.
    path: Option<PathBuf>,
    /// Serializes concurrent triggers (SIGHUP racing the watcher, or a push
    /// racing either): without it two renders could stamp v2 and v3 and store
    /// them in the wrong order — a version regression the lock makes
    /// impossible.
    reload_lock: Mutex<()>,
    /// Phase 4: an optional swap observer run UNDER the reload lock, AFTER a
    /// successful render but BEFORE the snapshot cell advances. It binds the
    /// new snapshot's WASM module set (paired atomically with the version) and
    /// may FAIL — a module that will not verify/compile NACKs the whole
    /// snapshot, so config and modules bind together or not at all (docs/04
    /// atomic module binding). `None` -> no wasm runtime.
    #[allow(clippy::type_complexity)]
    swap_hook: Mutex<Option<Box<dyn Fn(u64, &Snapshot) -> Result<(), String> + Send + Sync>>>,
}

impl Reloader {
    /// Initial render at startup from a file: fail-fast, exactly like
    /// milestone 1 — a bad file never serves a request.
    pub fn bootstrap(path: PathBuf) -> Result<Reloader, ConfigError> {
        let renderer = Renderer::new();
        let initial = renderer.render_file(&path)?;
        Ok(Reloader {
            shared: SharedSnapshot::new(initial),
            renderer,
            path: Some(path),
            reload_lock: Mutex::new(()),
            swap_hook: Mutex::new(None),
        })
    }

    /// Initial render at startup from bytes handed in (control-plane mode: the
    /// first `RenderedSnapshot` a joining node receives). Same fail-fast gate —
    /// a first push that does not validate never serves a request, and the node
    /// NACKs it upstream.
    pub fn bootstrap_from_text(text: &str, source: &Path) -> Result<Reloader, ConfigError> {
        let renderer = Renderer::new();
        let initial = renderer.render_text(text, source)?;
        Ok(Reloader {
            shared: SharedSnapshot::new(initial),
            renderer,
            path: None,
            reload_lock: Mutex::new(()),
            swap_hook: Mutex::new(None),
        })
    }

    pub fn shared(&self) -> SharedSnapshot {
        self.shared.clone()
    }

    /// Install the Phase 4 swap observer (the WASM module-set binder). Called
    /// once at startup after the runtime is built; runs under the reload lock
    /// on every subsequent swap, before the snapshot cell advances.
    #[allow(clippy::type_complexity)]
    pub fn set_swap_hook(
        &self,
        hook: Box<dyn Fn(u64, &Snapshot) -> Result<(), String> + Send + Sync>,
    ) {
        *self.swap_hook.lock().expect("swap hook lock") = Some(hook);
    }

    /// The file path in file mode; `None` in control-plane mode.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The single file-mode reload path. Reads the file once and delegates to
    /// [`Reloader::reload_from_text`] — file mode only differs in WHERE the
    /// bytes come from; the no-op / NACK / swap / drain semantics are shared.
    pub fn reload(&self, trigger: &str) -> ReloadOutcome {
        let Some(path) = self.path.clone() else {
            // A file reload was triggered on a control-plane-mode node: there is
            // no file. Treat as a loud no-op rather than a panic.
            error!("[reload] file reload triggered but this node has no config file (control-plane mode)");
            return ReloadOutcome::NoOp {
                active: self.shared.load().version,
            };
        };
        let _serialized = self.reload_lock.lock().expect("reload lock");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                let active = self.shared.load();
                error!(
                    "[reload] REJECTED (NACK, trigger={trigger}): cannot read {}: {e}; \
                     still serving cfg=v{} hash={}",
                    path.display(),
                    active.version,
                    active.short_hash(),
                );
                return ReloadOutcome::Rejected { active: active.version };
            }
        };
        self.reload_from_text_locked(&text, &path, trigger)
    }

    /// Bind config bytes handed in over the wire (a control-plane `Push`) or
    /// from any other in-memory source. Same no-op / NACK-keeps-old / atomic-
    /// swap / drain-on-old semantics as the file path — the control plane is
    /// just one more trigger funneling into the identical logic
    /// (docs/03-hot-swap.md; docs/07: "the node already knows how to accept,
    /// reject, and drain a version").
    ///
    /// The returned [`ReloadOutcome`] maps straight onto the wire Ack/Nack the
    /// client sends back (docs/07, "ACK/NACK semantics, extended").
    pub fn reload_from_text(&self, text: &str, source: &Path, trigger: &str) -> ReloadOutcome {
        let _serialized = self.reload_lock.lock().expect("reload lock");
        self.reload_from_text_locked(text, source, trigger)
    }

    /// The shared body, run under the already-held reload lock.
    fn reload_from_text_locked(&self, text: &str, source: &Path, trigger: &str) -> ReloadOutcome {
        let active = self.shared.load();

        if content_hash(text) == active.content_hash {
            debug!(
                "[reload] no-op (trigger={trigger}): content hash {} unchanged; \
                 still cfg=v{}",
                active.short_hash(),
                active.version,
            );
            return ReloadOutcome::NoOp { active: active.version };
        }

        match self.renderer.render_text(text, source) {
            Ok(next) => {
                let (old, new) = (active.version, next.version);
                // Phase 4: bind the new snapshot's WASM module set BEFORE the
                // snapshot cell advances (the store below). The hook stores
                // vN's modules keyed by version so a later vN reader finds
                // them — atomic config+module binding. A module that will not
                // verify/compile makes the hook FAIL, and the whole snapshot
                // NACKs: config and modules bind together or not at all.
                if let Some(hook) = self.swap_hook.lock().expect("swap hook lock").as_ref() {
                    if let Err(e) = hook(old, &next) {
                        error!(
                            "[reload] REJECTED (NACK, trigger={trigger}): new config's WASM \
                             module set failed to bind ({e}); still serving cfg=v{} hash={}",
                            active.version,
                            active.short_hash(),
                        );
                        return ReloadOutcome::Rejected { active: active.version };
                    }
                }
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
    let Some(path) = reloader.path().map(Path::to_path_buf) else {
        // No file to watch in control-plane mode; the stream is the trigger.
        return;
    };
    thread::Builder::new()
        .name("cfg-watch".to_string())
        .spawn(move || {
            let mut last_mtime = mtime_of(&path);
            loop {
                thread::sleep(interval);
                let mtime = mtime_of(&path);
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
  default_response:
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

    // ---- control-plane-mode paths (bytes pushed over the wire) -------------

    #[test]
    fn bootstrap_from_text_binds_v1_and_fails_fast_on_invalid() {
        let src = Path::new("<control-plane>");
        let reloader = Reloader::bootstrap_from_text(&valid_yaml("prod"), src).unwrap();
        assert_eq!(reloader.shared().load().version, 1);
        assert!(reloader.path().is_none(), "no file in control-plane mode");

        assert!(matches!(
            Reloader::bootstrap_from_text(&invalid_yaml(), src),
            Err(ConfigError::Invalid(_))
        ));
    }

    #[test]
    fn pushed_bytes_swap_noop_and_nack_exactly_like_the_file_path() {
        let src = Path::new("<control-plane>");
        let reloader = Reloader::bootstrap_from_text(&valid_yaml("prod"), src).unwrap();

        // A new valid push swaps to v2.
        assert_eq!(
            reloader.reload_from_text(&valid_yaml("canary"), src, "push"),
            ReloadOutcome::Swapped { old: 1, new: 2 }
        );
        assert_eq!(reloader.shared().load().version, 2);

        // Re-pushing identical bytes is a hash no-op — no new version.
        assert_eq!(
            reloader.reload_from_text(&valid_yaml("canary"), src, "push"),
            ReloadOutcome::NoOp { active: 2 }
        );

        // An invalid push is NACKed and v2 keeps serving (never silent).
        assert_eq!(
            reloader.reload_from_text(&invalid_yaml(), src, "push"),
            ReloadOutcome::Rejected { active: 2 }
        );
        assert_eq!(reloader.shared().load().version, 2, "old snapshot still active");

        // A later good push resumes at v3 (the NACK consumed no version).
        assert_eq!(
            reloader.reload_from_text(&valid_yaml("prod"), src, "push"),
            ReloadOutcome::Swapped { old: 2, new: 3 }
        );
    }

    #[test]
    fn pushed_swap_drains_the_old_version_on_its_last_holder() {
        let src = Path::new("<control-plane>");
        let reloader = Reloader::bootstrap_from_text(&valid_yaml("prod"), src).unwrap();

        // An in-flight request bound v1; a push swaps to v2 mid-stream.
        let bound = reloader.shared().load();
        let old_alive = Arc::downgrade(&bound);
        assert_eq!(
            reloader.reload_from_text(&valid_yaml("canary"), src, "push"),
            ReloadOutcome::Swapped { old: 1, new: 2 }
        );

        // The in-flight stream still sees v1 while v2 serves new binds — the
        // drain semantics carry over UNCHANGED from the file path.
        assert_eq!(bound.version, 1);
        assert_eq!(reloader.shared().load().version, 2);
        assert!(old_alive.upgrade().is_some(), "old snapshot pinned by the stream");
        drop(bound);
        assert!(old_alive.upgrade().is_none(), "old snapshot freed on last drop");
    }
}
