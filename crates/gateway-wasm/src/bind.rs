//! GB-9 for modules: atomic bind, per-stream drain, stateful migration, and
//! break-glass-with-TTL — the full doc-03 semantics applied to the module
//! set (docs/04 Phase 4 item 2).
//!
//! [`BoundModules`] is the runtime handle a request holds: the module set
//! bound to its snapshot (pinned by the same `Arc` the snapshot is), plus
//! the out-of-snapshot [`ModuleState`] the counters live in (shared across
//! swaps, exactly like GB-5's `NodeBudgets`). The invoke helpers here are
//! the fail-closed call sites: they run a hook and turn any [`HostError`]
//! into the operator's [`Decision`] — a broken module NEVER returns
//! `Continue` and never hangs.
//!
//! [`ModulePlan::diff`] is the swap decision: given the OLD bound set and a
//! NEW module set, it decides — per module name — whether the swap inherits,
//! migrates, resets, or initializes the module's counters, and returns the
//! observable [`MigrationOutcome`]s. gatewayd runs this the moment a snapshot
//! carrying a new module set is stored, exactly where the config swap logs
//! its old->new line.

use std::sync::Arc;

use log::{error, info, warn};

use crate::abi::{Decision, EndView, EventView, Hook, RequestView};
use crate::module::{ModuleSet, SharedModuleSet};
use crate::state::{Migration, MigrationOutcome, ModuleState, SchemaVersion};

/// The default bounded reset window (seconds) a schema bump with no migration
/// falls back to — the control-plane share-rebalance interval, so a reset
/// module's counters are re-established within one telemetry cycle. Stated,
/// not hidden (docs/03 limitation 3).
pub const DEFAULT_RESET_WINDOW_SECS: u64 = 30;

/// The runtime module handle a request binds. Clone-cheap (both fields are
/// `Arc`). Held for the request's whole life alongside its `Arc<Snapshot>`,
/// so the module set drains per stream exactly as the config does.
#[derive(Clone)]
pub struct BoundModules {
    modules: SharedModuleSet,
    state: Arc<ModuleState>,
}

impl BoundModules {
    pub fn new(modules: SharedModuleSet, state: Arc<ModuleState>) -> BoundModules {
        BoundModules { modules, state }
    }

    /// An empty binding (no modules) sharing a state store — the default a
    /// snapshot with no WASM modules carries. Zero per-event cost: the hot
    /// loop finds no `on_response_event` module and does nothing.
    pub fn empty(state: Arc<ModuleState>) -> BoundModules {
        BoundModules {
            modules: Arc::new(ModuleSet::default()),
            state,
        }
    }

    pub fn set(&self) -> &ModuleSet {
        &self.modules
    }

    pub fn state(&self) -> &ModuleState {
        &self.state
    }

    /// Whether ANY bound module implements `hook`. The hot-path guard: the
    /// per-event tap checks `wants(OnResponseEvent)` once and skips ALL WASM
    /// work when false — a snapshot with only an `on_request` module pays
    /// nothing per event (docs/04: gate per-event hooks).
    pub fn wants(&self, hook: Hook) -> bool {
        self.modules.for_hook(hook).next().is_some()
    }

    /// Run the `on_request` chain. The first module to return a non-`Continue`
    /// decision wins (reject/mutate short-circuits the chain, like the policy
    /// chain composes). A module that faults fails CLOSED to a reject.
    pub fn on_request(&self, view: &RequestView) -> Decision {
        for module in self.modules.for_hook(Hook::OnRequest) {
            match module.policy().on_request(view) {
                Ok(Decision::Continue) => continue,
                Ok(decision) => return decision,
                Err(e) => return fail_closed(Hook::OnRequest, module.name(), e),
            }
        }
        Decision::Continue
    }

    /// Run the `on_response_event` chain — the HOT path. First non-`Continue`
    /// wins (a cut short-circuits). A faulting module cuts the stream closed.
    /// Callers gate this behind [`BoundModules::wants`] so a set with no
    /// event module is never entered.
    pub fn on_response_event(&self, view: &EventView) -> Decision {
        for module in self.modules.for_hook(Hook::OnResponseEvent) {
            match module.policy().on_response_event(view) {
                Ok(Decision::Continue) => continue,
                Ok(decision) => return decision,
                Err(e) => return fail_closed(Hook::OnResponseEvent, module.name(), e),
            }
        }
        Decision::Continue
    }

    /// Run the `on_response_end` chain. A faulting module here fails closed to
    /// a reject decision the caller logs (the stream has already delivered;
    /// end-hook faults are observability, not enforcement, but still never
    /// silently `Continue`).
    pub fn on_response_end(&self, view: &EndView) -> Decision {
        for module in self.modules.for_hook(Hook::OnResponseEnd) {
            match module.policy().on_response_end(view) {
                Ok(Decision::Continue) => continue,
                Ok(decision) => return decision,
                Err(e) => return fail_closed(Hook::OnResponseEnd, module.name(), e),
            }
        }
        Decision::Continue
    }
}

/// Turn a host fault into the fail-closed decision, LOUDLY. The log names the
/// module and the exact wall it hit (fuel / epoch / trap / boundary) — the
/// operational story a platform team needs, and the evidence that the route
/// failed closed rather than hung.
fn fail_closed(hook: Hook, module: &str, err: crate::host::HostError) -> Decision {
    error!(
        "[wasm {module}] hook {} FAILED CLOSED: {err}; failing the route to the operator template",
        hook.export_name()
    );
    Decision::fail_closed(hook, format!("wasm-module-fault:{module}"))
}

/// The per-module migration decisions a swap makes, plus the resulting bound
/// set. Built by [`ModulePlan::diff`], applied by [`ModulePlan::apply`].
#[derive(Debug)]
pub struct ModulePlan {
    /// (module name, outcome) for every module in the NEW set.
    pub outcomes: Vec<(String, MigrationOutcome)>,
}

impl ModulePlan {
    /// Decide the migration for a swap from `old` (the previously bound set,
    /// or `None` at first bind) to `new`, running each changed module's
    /// counter migration against `state`. `migrations` maps a module name to
    /// its declared migration chain; a name with no entry falls back to
    /// reset-on-schema-bump with `reset_window_secs`.
    ///
    /// Applying the migration MUTATES `state` (the counters), so this is the
    /// swap's side effect — called once, under the reload lock, exactly where
    /// the config swap stores the new snapshot. Modules present in `old` but
    /// absent from `new` are left in `state` untouched: a re-added module
    /// inherits its counters, a permanently-removed one is inert (its
    /// counters are dead weight, swept on process restart — an in-memory
    /// bound, stated).
    pub fn diff(
        old: Option<&ModuleSet>,
        new: &ModuleSet,
        state: &ModuleState,
        migrations: &dyn Fn(&str) -> Vec<Migration>,
        reset_window_secs: u64,
    ) -> ModulePlan {
        let mut outcomes = Vec::with_capacity(new.len());
        for (name, new_schema) in new.schemas() {
            let old_schema = old.and_then(|o| o.get(name)).map(|m| m.schema());
            let outcome = plan_one(
                state,
                name,
                old_schema,
                new_schema,
                migrations,
                reset_window_secs,
            );
            log_outcome(name, &outcome);
            outcomes.push((name.to_string(), outcome));
        }
        ModulePlan { outcomes }
    }

    /// The names that RESET (lost their counters) in this plan — the swap's
    /// bounded-overspend surface, for the swap log line and tests.
    pub fn reset_modules(&self) -> Vec<&str> {
        self.outcomes
            .iter()
            .filter(|(_, o)| matches!(o, MigrationOutcome::Reset { .. }))
            .map(|(n, _)| n.as_str())
            .collect()
    }
}

/// Migrate one module's counters for the swap and return the outcome. A
/// changed schema (or a first bind) touches `state`; an unchanged schema is
/// an inherit that still runs through `rebind` so the code path is uniform.
fn plan_one(
    state: &ModuleState,
    name: &str,
    old_schema: Option<SchemaVersion>,
    new_schema: SchemaVersion,
    migrations: &dyn Fn(&str) -> Vec<Migration>,
    reset_window_secs: u64,
) -> MigrationOutcome {
    // A module truly new to this node (never bound) initializes; a module that
    // existed keeps its counters and either inherits or migrates. `rebind`
    // already distinguishes these by whether the counter map exists, so the
    // `old_schema` here is only used to keep the log precise.
    let _ = old_schema;
    let chain = migrations(name);
    state.rebind(name, new_schema, &chain, reset_window_secs)
}

fn log_outcome(name: &str, outcome: &MigrationOutcome) {
    match outcome {
        MigrationOutcome::Inherited { schema, keys } => info!(
            "[wasm-swap {name}] counters INHERITED at schema v{schema} ({keys} keys carried forward)"
        ),
        MigrationOutcome::Migrated { from, to, keys } => info!(
            "[wasm-swap {name}] counters MIGRATED schema v{from} -> v{to} ({keys} keys transformed)"
        ),
        MigrationOutcome::Reset { from, to, window_secs } => warn!(
            "[wasm-swap {name}] counters RESET schema v{from} -> v{to} (no migration declared); \
             bounded-overspend window {window_secs}s until telemetry re-establishes counts \
             (docs/03 limitation 3, stated not hidden)"
        ),
        MigrationOutcome::Initialized { schema } => info!(
            "[wasm-swap {name}] counters INITIALIZED at schema v{schema} (first bind on this node)"
        ),
    }
}

/// Break-glass for module binding (docs/04 Phase 4 item 3; reusing the
/// reconciler break-glass shape from Phase 2). A break-glass window lets an
/// operator PIN a module set temporarily — e.g. force-disable a
/// misbehaving module at 3am — visibly, with a TTL, auto-reverting when the
/// window lapses. Same "visible, temporary, auto-reverting" contract as the
/// config break-glass: the pinned set serves until `until`, then the next
/// bind after expiry falls back to the desired (snapshot-carried) set.
///
/// This mirrors `gatewayctl`'s `NodeState::break_glass_active`/`until` field
/// exactly (a unix expiry, checked against an injected `now`), so the data
/// plane and control plane reason about break-glass the same way.
#[derive(Clone)]
pub struct ModuleBreakGlass {
    pinned: SharedModuleSet,
    /// Unix seconds the pin expires at.
    until: u64,
    reason: String,
}

impl ModuleBreakGlass {
    /// Pin `set` until `until` (unix seconds), for `reason`.
    pub fn pin(set: SharedModuleSet, until: u64, reason: &str) -> ModuleBreakGlass {
        info!(
            "[wasm-break-glass] module set PINNED until unix={until} (reason: {reason}); \
             visible, temporary, auto-reverting (docs/04)"
        );
        ModuleBreakGlass {
            pinned: set,
            until,
            reason: reason.to_string(),
        }
    }

    /// Whether the pin is still active at `now`.
    pub fn active(&self, now: u64) -> bool {
        now < self.until
    }

    /// The set to serve at `now`: the pinned set while the window is open,
    /// else `desired` (the snapshot-carried set) — auto-revert on expiry.
    pub fn resolve(&self, desired: &SharedModuleSet, now: u64) -> SharedModuleSet {
        if self.active(now) {
            self.pinned.clone()
        } else {
            info!(
                "[wasm-break-glass] window lapsed (reason: {}); reverting to the desired module set",
                self.reason
            );
            desired.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Limits;
    use crate::module::ModuleManifest;
    use crate::sig;

    const KEY: &[u8] = b"operator-key";
    const CONTINUE_WAT: &str = include_str!("../fixtures/continue.wat");
    const BURN_WAT: &str = include_str!("../fixtures/burn_fuel.wat");
    const MUTATE_WAT: &str = include_str!("../fixtures/mutate_headers.wat");

    fn manifest(name: &str, wat: &str, hooks: Vec<Hook>, schema: SchemaVersion) -> ModuleManifest {
        let bytes = wat.as_bytes().to_vec();
        let signature = Some(sig::sign(KEY, &bytes));
        ModuleManifest { name: name.to_string(), bytes, signature, hooks, schema }
    }

    fn set(manifests: &[ModuleManifest]) -> SharedModuleSet {
        Arc::new(ModuleSet::load(manifests, KEY, Limits::default()).unwrap())
    }

    #[test]
    fn a_continue_module_lets_the_request_through() {
        let bound = BoundModules::new(
            set(&[manifest("noop", CONTINUE_WAT, vec![Hook::OnRequest], 1)]),
            Arc::new(ModuleState::new()),
        );
        assert_eq!(bound.on_request(&RequestView::default()), Decision::Continue);
    }

    #[test]
    fn a_mutate_module_returns_its_header_transform() {
        let bound = BoundModules::new(
            set(&[manifest("hdr", MUTATE_WAT, vec![Hook::OnRequest], 1)]),
            Arc::new(ModuleState::new()),
        );
        match bound.on_request(&RequestView::default()) {
            Decision::MutateHeaders { set, .. } => {
                assert_eq!(set.get("x-policy").map(String::as_str), Some("enforced"));
            }
            other => panic!("expected MutateHeaders, got {other:?}"),
        }
    }

    #[test]
    fn a_fuel_burning_module_fails_closed_to_reject_not_continue() {
        // Tight fuel so the loop trips OutOfFuel fast and deterministically.
        let limits = Limits { fuel: 100_000, ..Limits::default() };
        let modules = Arc::new(
            ModuleSet::load(
                &[manifest("burner", BURN_WAT, vec![Hook::OnRequest], 1)],
                KEY,
                limits,
            )
            .unwrap(),
        );
        let bound = BoundModules::new(modules, Arc::new(ModuleState::new()));
        // The looping module must NOT return Continue — it fails closed.
        assert!(matches!(
            bound.on_request(&RequestView::default()),
            Decision::Reject { .. }
        ));
    }

    #[test]
    fn wants_gates_the_hot_path() {
        let state = Arc::new(ModuleState::new());
        // A request-only module: wants(OnResponseEvent) is false, so the tap
        // skips ALL wasm work per event.
        let bound = BoundModules::new(
            set(&[manifest("req", CONTINUE_WAT, vec![Hook::OnRequest], 1)]),
            state.clone(),
        );
        assert!(bound.wants(Hook::OnRequest));
        assert!(!bound.wants(Hook::OnResponseEvent));

        // An empty binding wants nothing — zero per-event cost.
        let empty = BoundModules::empty(state);
        assert!(!empty.wants(Hook::OnResponseEvent));
    }

    #[test]
    fn diff_initializes_then_inherits_then_migrates() {
        let state = ModuleState::new();
        let no_migrations = |_: &str| Vec::new();

        // First bind: a v1 module -> Initialized.
        let s1 = ModuleSet::load(
            &[manifest("quota", CONTINUE_WAT, vec![Hook::OnResponseEvent], 1)],
            KEY,
            Limits::default(),
        )
        .unwrap();
        let plan = ModulePlan::diff(None, &s1, &state, &no_migrations, DEFAULT_RESET_WINDOW_SECS);
        assert!(matches!(plan.outcomes[0].1, MigrationOutcome::Initialized { schema: 1 }));
        // Accrue a counter under v1.
        state.bump("quota", "t", 7);

        // Same schema swap -> Inherited, counter preserved.
        let s1b = ModuleSet::load(
            &[manifest("quota", CONTINUE_WAT, vec![Hook::OnResponseEvent], 1)],
            KEY,
            Limits::default(),
        )
        .unwrap();
        let plan = ModulePlan::diff(Some(&s1), &s1b, &state, &no_migrations, DEFAULT_RESET_WINDOW_SECS);
        assert!(matches!(plan.outcomes[0].1, MigrationOutcome::Inherited { schema: 1, keys: 1 }));
        assert_eq!(state.counters("quota").unwrap().values["t"], 7);

        // Schema bump WITH a declared migration (x10) -> Migrated, 7 -> 70.
        let migrations = |name: &str| {
            if name == "quota" {
                vec![Migration {
                    from: 1,
                    to: 2,
                    map: Box::new(|old| old.iter().map(|(k, v)| (k.clone(), v * 10)).collect()),
                }]
            } else {
                Vec::new()
            }
        };
        let s2 = ModuleSet::load(
            &[manifest("quota", CONTINUE_WAT, vec![Hook::OnResponseEvent], 2)],
            KEY,
            Limits::default(),
        )
        .unwrap();
        let plan = ModulePlan::diff(Some(&s1b), &s2, &state, &migrations, DEFAULT_RESET_WINDOW_SECS);
        assert!(matches!(plan.outcomes[0].1, MigrationOutcome::Migrated { from: 1, to: 2, keys: 1 }));
        assert_eq!(state.counters("quota").unwrap().values["t"], 70);
        assert!(plan.reset_modules().is_empty());
    }

    #[test]
    fn diff_resets_a_schema_bump_with_no_migration_and_reports_it() {
        let state = ModuleState::new();
        let no_migrations = |_: &str| Vec::new();
        let s1 = ModuleSet::load(
            &[manifest("q", CONTINUE_WAT, vec![Hook::OnResponseEvent], 1)],
            KEY,
            Limits::default(),
        )
        .unwrap();
        ModulePlan::diff(None, &s1, &state, &no_migrations, DEFAULT_RESET_WINDOW_SECS);
        state.bump("q", "t", 500);

        let s2 = ModuleSet::load(
            &[manifest("q", CONTINUE_WAT, vec![Hook::OnResponseEvent], 2)],
            KEY,
            Limits::default(),
        )
        .unwrap();
        let plan = ModulePlan::diff(Some(&s1), &s2, &state, &no_migrations, 30);
        assert_eq!(plan.reset_modules(), vec!["q"]);
        assert!(matches!(
            plan.outcomes[0].1,
            MigrationOutcome::Reset { from: 1, to: 2, window_secs: 30 }
        ));
        // Counter zeroed after the reset.
        assert!(state.counters("q").unwrap().values.is_empty());
    }

    #[test]
    fn break_glass_pins_then_auto_reverts() {
        let desired = set(&[manifest("desired", CONTINUE_WAT, vec![Hook::OnRequest], 1)]);
        let pinned = set(&[manifest("pinned", CONTINUE_WAT, vec![Hook::OnRequest], 1)]);
        let bg = ModuleBreakGlass::pin(pinned.clone(), 200, "force-disable misbehaving module");

        // Within the window: the pinned set serves.
        let now = 100;
        assert!(bg.active(now));
        assert!(Arc::ptr_eq(&bg.resolve(&desired, now), &pinned));

        // After expiry: auto-revert to desired.
        let later = 250;
        assert!(!bg.active(later));
        assert!(Arc::ptr_eq(&bg.resolve(&desired, later), &desired));
    }
}
