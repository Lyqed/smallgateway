//! The data-plane WASM runtime: atomic module binding paired with the config
//! snapshot, migration on swap, and break-glass (GB-9, docs/03/04 Phase 4).
//!
//! The problem this solves is the ATOMIC module binding (docs/04): a request
//! must bind a config version AND its module set together — no torn read
//! where the config is vN but a module is vN-1. The config `Snapshot` lives
//! in `gateway-core` and cannot hold wasmtime handles, so this runtime keeps
//! the compiled module set in a cell KEYED BY SNAPSHOT VERSION, and binds
//! both in one guarded read ([`WasmRuntime::bind`]). Because the module set
//! for version N is stored BEFORE the snapshot cell advances to N (see
//! [`WasmRuntime::on_swap`]), a request that reads snapshot vN always finds
//! its matching module set — the atomicity the doc demands, on top of the
//! existing `Arc<Snapshot>` per-request pin.
//!
//! Drain falls out for free: a request holds its [`BoundModules`] (an `Arc`)
//! for its whole life, so an in-flight stream keeps its module version until
//! it finishes, exactly as it keeps its config version.
//!
//! Migration ([`gateway_wasm::ModulePlan`]) runs on every swap that changes a
//! module's schema; the counters live in a process-lifetime [`ModuleState`]
//! (outside snapshots, like GB-5's `NodeBudgets`), so they survive swaps and
//! migrate per the module's declared schema — or reset with a stated window.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use log::{info, warn};

use gateway_core::config::{Config, WasmHook, WasmModule};
use gateway_core::snapshot::Snapshot;
use gateway_wasm::{
    BoundModules, Hook, Limits, ModulePlan, ModuleManifest, ModuleSet, ModuleState,
    SharedModuleSet, Watchdog,
};
use gateway_wasm::bind::DEFAULT_RESET_WINDOW_SECS;

/// The operator signing key for module verification. In a real deployment this
/// comes from a secret manager / env; here it is read from
/// `GATEWAYD_WASM_SIGNING_KEY` (or a fixed dev key when absent, loudly logged),
/// mirroring how GB-2's HS256 secret is an operator-held value.
pub fn signing_key() -> Vec<u8> {
    match std::env::var("GATEWAYD_WASM_SIGNING_KEY") {
        Ok(k) if !k.is_empty() => k.into_bytes(),
        _ => {
            warn!(
                "[wasm] GATEWAYD_WASM_SIGNING_KEY unset — using the DEV signing key; \
                 set an operator key in production (unsigned/mis-signed modules fail closed)"
            );
            b"gatewayd-dev-signing-key".to_vec()
        }
    }
}

/// The paired binding for one snapshot version: its module set. Kept in a
/// small map so a swap that fails to compile modules NACKs (the snapshot is
/// rejected) without disturbing the currently-serving version.
struct Bindings {
    /// version -> its compiled module set. Old versions stay until their last
    /// in-flight stream drops the `Arc`; the runtime prunes on swap.
    by_version: BTreeMap<u64, SharedModuleSet>,
    /// Whether per-event streaming hooks are enabled (config `per_event_hooks`).
    per_event: bool,
}

/// The runtime the proxy consults. Clone-cheap (`Arc` inside).
#[derive(Clone)]
pub struct WasmRuntime {
    inner: Arc<Inner>,
}

struct Inner {
    bindings: RwLock<Bindings>,
    /// Process-lifetime counter state (outside snapshots; survives swaps).
    state: Arc<ModuleState>,
    /// Operator signing key.
    key: Vec<u8>,
    /// Per-invocation resource bounds every module runs under.
    limits: Limits,
    /// The config source directory, so a `WasmModule.source` path resolves.
    base_dir: Option<PathBuf>,
    /// Break-glass: an optional operator pin of a module set with a TTL.
    break_glass: RwLock<Option<gateway_wasm::bind::ModuleBreakGlass>>,
}

impl WasmRuntime {
    /// Build the runtime from the bootstrap snapshot. Loads its module set
    /// (verifying signatures), starts ONE epoch watchdog for the shared
    /// engine bounds, and pairs the set with v-the-bootstrap-version. A module
    /// that fails to load fails BOOTSTRAP — a node never serves with a module
    /// set the operator's config named but could not verify/compile.
    pub fn bootstrap(
        snap: &Snapshot,
        base_dir: Option<PathBuf>,
    ) -> Result<WasmRuntime, String> {
        let key = signing_key();
        let limits = Limits::default();
        let state = Arc::new(ModuleState::new());
        let set = load_set(&snap.config, base_dir.as_deref(), &key, limits)?;

        // Migration genesis for the bootstrap set (Initialized outcomes).
        if !set.is_empty() {
            let plan = ModulePlan::diff(
                None,
                &set,
                &state,
                &migrations_for,
                DEFAULT_RESET_WINDOW_SECS,
            );
            info!(
                "[wasm] bootstrap loaded {} module(s) at cfg=v{}; {}",
                set.len(),
                snap.version,
                summarize_plan(&plan),
            );
            // Start the epoch watchdog on this engine so the deadline fires.
            if let Some(module) = set.for_hook(Hook::OnRequest).next() {
                Watchdog::spawn(module.policy().engine().clone(), watchdog_tick());
            } else if let Some(module) = set.schemas().next().and_then(|(n, _)| set.get(n)) {
                Watchdog::spawn(module.policy().engine().clone(), watchdog_tick());
            }
        }

        let mut by_version = BTreeMap::new();
        by_version.insert(snap.version, Arc::new(set));
        Ok(WasmRuntime {
            inner: Arc::new(Inner {
                bindings: RwLock::new(Bindings {
                    by_version,
                    per_event: snap.config.wasm.per_event_hooks,
                }),
                state,
                key,
                limits,
                base_dir,
                break_glass: RwLock::new(None),
            }),
        })
    }

    /// Bind the module set for `version` — the per-request pairing. Returns a
    /// [`BoundModules`] the request holds for its whole life (drain). A
    /// break-glass pin, if active at `now`, overrides the desired set
    /// (visible, temporary, auto-reverting). An unknown version (should never
    /// happen — the swap stores the set before advancing the snapshot) binds
    /// EMPTY, fail-safe: no module runs rather than the wrong version's.
    pub fn bind(&self, version: u64, now: u64) -> BoundModules {
        let bindings = self.inner.bindings.read().expect("bindings lock");
        let desired = bindings
            .by_version
            .get(&version)
            .cloned()
            .unwrap_or_else(|| Arc::new(ModuleSet::default()));
        drop(bindings);

        // Break-glass: pin overrides desired for its window.
        let set = {
            let bg = self.inner.break_glass.read().expect("break-glass lock");
            match bg.as_ref() {
                Some(bg) if bg.active(now) => bg.resolve(&desired, now),
                _ => desired,
            }
        };
        BoundModules::new(set, self.inner.state.clone())
    }

    /// Whether per-event streaming hooks are enabled on this node. The proxy
    /// checks this AND `BoundModules::wants(OnResponseEvent)` before touching
    /// wasm per event — a double gate: config-off OR no event module -> zero
    /// per-event cost (the measured-budget gate, docs/04).
    pub fn per_event_enabled(&self) -> bool {
        self.inner.bindings.read().expect("bindings lock").per_event
    }

    /// Apply a config swap: load the NEW snapshot's module set, run migration
    /// against the OLD set, and store the new set KEYED BY the new version
    /// BEFORE the caller advances the snapshot cell — so a request that later
    /// reads vN always finds vN's modules (atomic bind). Returns the reset
    /// module names (the bounded-overspend surface) for the swap log, or an
    /// error that makes the caller NACK the whole snapshot (modules and config
    /// bind together — a module that will not load rejects the config too).
    pub fn on_swap(&self, old_version: u64, new_snap: &Snapshot) -> Result<Vec<String>, String> {
        let new_set = load_set(
            &new_snap.config,
            self.inner.base_dir.as_deref(),
            &self.inner.key,
            self.inner.limits,
        )?;

        let mut bindings = self.inner.bindings.write().expect("bindings lock");
        let old_set = bindings.by_version.get(&old_version).cloned();

        let plan = ModulePlan::diff(
            old_set.as_deref(),
            &new_set,
            &self.inner.state,
            &migrations_for,
            DEFAULT_RESET_WINDOW_SECS,
        );
        let resets: Vec<String> = plan.reset_modules().iter().map(|s| s.to_string()).collect();
        info!(
            "[wasm] swap cfg=v{old_version} -> v{}: {}",
            new_snap.version,
            summarize_plan(&plan),
        );

        // Store the new set BEFORE the snapshot cell advances (the caller does
        // that after this returns Ok) — the store ordering that guarantees a
        // vN reader finds vN modules.
        bindings.by_version.insert(new_snap.version, Arc::new(new_set));
        bindings.per_event = new_snap.config.wasm.per_event_hooks;

        // Prune versions with no remaining Arc holder (drained streams). We
        // keep the new version and any still-referenced older ones; a version
        // whose only reference is the map itself (strong_count == 1) and which
        // is older than the new version is safe to drop.
        let new_v = new_snap.version;
        bindings.by_version.retain(|&v, set| {
            v >= new_v || Arc::strong_count(set) > 1
        });
        Ok(resets)
    }

    /// Install a break-glass pin (docs/04 item 3): force a specific module set
    /// (e.g. the empty set, to disable a misbehaving module) until `until`,
    /// visibly and temporarily. Reuses the reconciler break-glass shape.
    pub fn break_glass_pin(&self, set: SharedModuleSet, until: u64, reason: &str) {
        let bg = gateway_wasm::bind::ModuleBreakGlass::pin(set, until, reason);
        *self.inner.break_glass.write().expect("break-glass lock") = Some(bg);
    }

    /// The empty module set — what break-glass pins to disable all modules.
    pub fn empty_set(&self) -> SharedModuleSet {
        Arc::new(ModuleSet::default())
    }

    /// Spawn the SIGUSR1 break-glass listener: on SIGUSR1, pin the EMPTY
    /// module set for `ttl_secs` (the 3am "force-disable a misbehaving
    /// module" story, docs/00/04). Visible (logged), temporary (the TTL), and
    /// auto-reverting (the next bind after expiry falls back to desired). One
    /// signal, one bounded window — the honest middle between forbidding
    /// emergency edits and losing Git as truth.
    pub fn spawn_break_glass_listener(self, ttl_secs: u64) {
        std::thread::Builder::new()
            .name("wasm-break-glass".to_string())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        warn!("[wasm] break-glass listener runtime failed: {e}");
                        return;
                    }
                };
                rt.block_on(async {
                    let mut usr1 = match tokio::signal::unix::signal(
                        tokio::signal::unix::SignalKind::user_defined1(),
                    ) {
                        Ok(s) => s,
                        Err(e) => {
                            warn!("[wasm] cannot install SIGUSR1 handler: {e}");
                            return;
                        }
                    };
                    while usr1.recv().await.is_some() {
                        let until = now_unix() + ttl_secs;
                        self.break_glass_pin(
                            self.empty_set(),
                            until,
                            "SIGUSR1: emergency disable of all WASM modules",
                        );
                        warn!(
                            "[wasm-break-glass] SIGUSR1: ALL wasm modules disabled for {ttl_secs}s \
                             (until unix={until}); auto-reverts to the desired set on expiry"
                        );
                    }
                });
            })
            .map(|_| ())
            .unwrap_or_else(|e| warn!("[wasm] failed to spawn break-glass listener: {e}"));
    }
}

/// Unix seconds — the break-glass TTL clock (injected in tests, wall clock
/// here). Shares the reconciler's break-glass time model.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The epoch watchdog tick. Paired with the default `epoch_deadline` (10
/// ticks), a stuck guest is preempted within ~10ms of blowing its deadline
/// while a legitimate microsecond hook — which cannot straddle 10 ticks — is
/// never spuriously preempted. 1ms keeps the ceiling tight without spinning a
/// core; the headroom lives in the deadline, not the tick.
fn watchdog_tick() -> std::time::Duration {
    std::time::Duration::from_millis(1)
}

/// Declared migrations for a module by name. Phase 4 ships the MECHANISM
/// ([`ModulePlan`] + [`ModuleState`]) and this seam; a production build wires
/// a real migration catalog here (or from the manifest). Today no migrations
/// are declared, so a schema bump resets with the stated bounded window — the
/// honest default, documented in the README.
fn migrations_for(_name: &str) -> Vec<gateway_wasm::Migration> {
    Vec::new()
}

/// Load and verify the module set declared in `cfg.wasm`. Reads each module's
/// bytes from `base_dir/source`, verifies its signature, and compiles it. Any
/// failure is a `String` error that becomes a snapshot NACK.
fn load_set(
    cfg: &Config,
    base_dir: Option<&Path>,
    key: &[u8],
    limits: Limits,
) -> Result<ModuleSet, String> {
    if cfg.wasm.modules.is_empty() {
        return Ok(ModuleSet::default());
    }
    let mut manifests = Vec::with_capacity(cfg.wasm.modules.len());
    for module in &cfg.wasm.modules {
        manifests.push(to_manifest(module, base_dir)?);
    }
    ModuleSet::load(&manifests, key, limits).map_err(|e| e.to_string())
}

/// Turn a config `WasmModule` into a loadable manifest: read the bytes from
/// disk (relative to the config dir), carry the declared signature and hooks.
fn to_manifest(module: &WasmModule, base_dir: Option<&Path>) -> Result<ModuleManifest, String> {
    let path = match base_dir {
        Some(dir) => dir.join(&module.source),
        None => PathBuf::from(&module.source),
    };
    let bytes = std::fs::read(&path)
        .map_err(|e| format!("wasm module {:?}: cannot read {}: {e}", module.name, path.display()))?;
    Ok(ModuleManifest {
        name: module.name.clone(),
        bytes,
        signature: module.signature.clone(),
        hooks: module.hooks.iter().map(to_hook).collect(),
        schema: module.schema,
    })
}

fn to_hook(h: &WasmHook) -> Hook {
    match h {
        WasmHook::OnRequest => Hook::OnRequest,
        WasmHook::OnResponseEvent => Hook::OnResponseEvent,
        WasmHook::OnResponseEnd => Hook::OnResponseEnd,
    }
}

/// One-line summary of a migration plan for the swap log.
fn summarize_plan(plan: &ModulePlan) -> String {
    if plan.outcomes.is_empty() {
        return "no modules".to_string();
    }
    plan.outcomes
        .iter()
        .map(|(name, outcome)| format!("{name}:{outcome:?}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gateway_core::snapshot::Renderer;
    use std::path::Path;

    const KEY_ENV: &str = "GATEWAYD_WASM_SIGNING_KEY";
    const CONTINUE_WAT: &str = include_str!("../../gateway-wasm/fixtures/continue.wat");
    const MUTATE_WAT: &str = include_str!("../../gateway-wasm/fixtures/mutate_headers.wat");

    /// Write a module file + return a config referencing it, signed with the
    /// env key. Returns (config_yaml, base_dir).
    fn repo_with_module(
        wat: &str,
        hooks: &str,
        schema: u32,
        per_event: bool,
    ) -> (String, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "gatewayd-wasm-test-{}-{}",
            std::process::id(),
            fastrand_like(),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let module_path = dir.join("mod.wat");
        std::fs::write(&module_path, wat).unwrap();

        let key = std::env::var(KEY_ENV).unwrap();
        let sig = gateway_wasm::sign(key.as_bytes(), wat.as_bytes());
        let yaml = format!(
            r#"
providers:
  openai-main:
    kind: openai
    upstream: {{ host: 127.0.0.1, port: 6190 }}
wasm:
  per_event_hooks: {per_event}
  modules:
    - name: policy
      source: mod.wat
      signature: "{sig}"
      hooks: {hooks}
      schema: {schema}
routes:
  - prefix: /openai
    provider: openai-main
    attribution:
      pinned: {{ env: prod }}
rejections:
  missing_attribution:
    status: 428
    content_type: application/json
    body: '{{"error":"missing {{{{key}}}}"}}'
  unknown_route:
    status: 404
    content_type: application/json
    body: '{{"error":"no route"}}'
"#
        );
        (yaml, dir)
    }

    fn fastrand_like() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ SEQ.fetch_add(1, Ordering::Relaxed)
    }

    #[test]
    fn bootstrap_binds_a_signed_module_and_the_request_hook_runs() {
        std::env::set_var(KEY_ENV, "test-key");
        let (yaml, dir) = repo_with_module(MUTATE_WAT, "[on_request]", 1, false);
        let renderer = Renderer::new();
        let snap = renderer.render_text(&yaml, Path::new("g.yaml")).unwrap();
        let rt = WasmRuntime::bootstrap(&snap, Some(dir.clone())).unwrap();

        let bound = rt.bind(snap.version, 0);
        assert!(bound.wants(Hook::OnRequest));
        // The mutate module enforces a header transform on_request.
        match bound.on_request(&gateway_wasm::RequestView::default()) {
            gateway_wasm::Decision::MutateHeaders { set, .. } => {
                assert_eq!(set.get("x-policy").map(String::as_str), Some("enforced"));
            }
            other => panic!("expected MutateHeaders, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unsigned_module_fails_bootstrap() {
        std::env::set_var(KEY_ENV, "test-key");
        let (yaml, dir) = repo_with_module(CONTINUE_WAT, "[on_request]", 1, false);
        // Strip the signature line -> gateway-core parses (presence not a
        // structural error) but the loader's verify rejects it.
        let unsigned = yaml
            .lines()
            .filter(|l| !l.trim_start().starts_with("signature:"))
            .collect::<Vec<_>>()
            .join("\n");
        let renderer = Renderer::new();
        let snap = renderer.render_text(&unsigned, Path::new("g.yaml")).unwrap();
        let result = WasmRuntime::bootstrap(&snap, Some(dir.clone()));
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("an unsigned module must fail bootstrap"),
        };
        assert!(err.contains("unsigned") || err.contains("signature"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn swap_pairs_the_new_set_with_the_new_version_and_old_binds_still_see_old() {
        std::env::set_var(KEY_ENV, "test-key");
        // v1: a continue module. v2: a mutate module. An in-flight bind to v1
        // must keep seeing continue while a new bind to v2 sees mutate.
        let (yaml_v1, dir) = repo_with_module(CONTINUE_WAT, "[on_request]", 1, false);
        let renderer = Renderer::new();
        let snap1 = renderer.render_text(&yaml_v1, Path::new("g.yaml")).unwrap();
        let rt = WasmRuntime::bootstrap(&snap1, Some(dir.clone())).unwrap();

        // An in-flight request binds v1's module set and HOLDS it.
        let bound_v1 = rt.bind(snap1.version, 0);
        assert_eq!(
            bound_v1.on_request(&gateway_wasm::RequestView::default()),
            gateway_wasm::Decision::Continue
        );

        // Swap to v2 (mutate). Write the new module file into the same dir.
        std::fs::write(dir.join("mod.wat"), MUTATE_WAT).unwrap();
        let key = std::env::var(KEY_ENV).unwrap();
        let sig2 = gateway_wasm::sign(key.as_bytes(), MUTATE_WAT.as_bytes());
        let yaml_v2 = yaml_v1
            .replace(&sig_of(&yaml_v1), &sig2)
            .replace("env: prod", "env: canary");
        let snap2 = renderer.render_text(&yaml_v2, Path::new("g.yaml")).unwrap();
        rt.on_swap(snap1.version, &snap2).unwrap();

        // The in-flight v1 bind STILL returns Continue (its module set is
        // pinned by the Arc it holds) while a fresh v2 bind returns Mutate.
        assert_eq!(
            bound_v1.on_request(&gateway_wasm::RequestView::default()),
            gateway_wasm::Decision::Continue,
            "in-flight stream keeps its old module version (drain)"
        );
        let bound_v2 = rt.bind(snap2.version, 0);
        assert!(matches!(
            bound_v2.on_request(&gateway_wasm::RequestView::default()),
            gateway_wasm::Decision::MutateHeaders { .. }
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Extract the signature value from a generated config (for the swap test).
    fn sig_of(yaml: &str) -> String {
        yaml.lines()
            .find_map(|l| l.trim_start().strip_prefix("signature: \""))
            .map(|s| s.trim_end_matches('"').to_string())
            .expect("signature line")
    }

    #[test]
    fn break_glass_pins_the_empty_set_then_reverts() {
        std::env::set_var(KEY_ENV, "test-key");
        let (yaml, dir) = repo_with_module(MUTATE_WAT, "[on_request]", 1, false);
        let renderer = Renderer::new();
        let snap = renderer.render_text(&yaml, Path::new("g.yaml")).unwrap();
        let rt = WasmRuntime::bootstrap(&snap, Some(dir.clone())).unwrap();

        // Pin the EMPTY set until unix=200 — disable the misbehaving module.
        rt.break_glass_pin(rt.empty_set(), 200, "disable policy at 3am");

        // Within the window: no module runs (empty set) -> Continue.
        let bound = rt.bind(snap.version, 100);
        assert_eq!(
            bound.on_request(&gateway_wasm::RequestView::default()),
            gateway_wasm::Decision::Continue,
            "break-glass empty set: the module is disabled"
        );

        // After expiry: auto-revert to the desired (mutate) set.
        let bound = rt.bind(snap.version, 250);
        assert!(matches!(
            bound.on_request(&gateway_wasm::RequestView::default()),
            gateway_wasm::Decision::MutateHeaders { .. }
        ));
        std::fs::remove_dir_all(&dir).ok();
    }
}
