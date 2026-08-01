//! Stateful-module migration across a hot-swap (docs/03, limitation 3 — the
//! one limitation the phase must not leave unaddressed).
//!
//! A WASM policy module may own counters (a per-tenant request counter, a
//! sliding quota, a rolling token tally). Swapping a stateless transform is
//! trivial; swapping a module that owns counters is a state-migration
//! problem: the incoming version either INHERITS the old counters (the
//! schema is versioned and a migration hook maps old->new) or RESETS them (a
//! bounded window while every counter reads zero). This module implements
//! the mechanism and makes the bound explicit — it does not hand-wave the
//! limitation away.
//!
//! The design mirrors how GB-5 budget counters already survive a config
//! swap (they live OUTSIDE the snapshot, in `NodeBudgets`): a module's
//! counters live in [`ModuleState`], keyed by the module NAME, OUTSIDE the
//! snapshot too. When a snapshot swap binds a new VERSION of a module with
//! the same name, [`migrate`] runs:
//!
//! - **Same schema version** -> the counters carry forward untouched
//!   (inherit). No window, no reset.
//! - **Newer schema version, migratable** -> the declared [`Migration`]
//!   maps the old counter map to the new one (inherit, transformed).
//! - **Newer schema version, no migration declared** -> RESET, and the
//!   bounded-overspend window is STATED in the return value and logged, not
//!   silent. This is the "reset with a stated bounded window" branch of the
//!   doc-03 trade-off, made observable.
//!
//! Counters are a simple `BTreeMap<String, i64>` — enough to prove the
//! mechanism (a real module's richer state serializes to the same shape).
//! The point of the phase is that the mechanism EXISTS and the reset window
//! is declared; a production migration catalog is a later elaboration noted
//! in the README.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// A module's counter schema version. Bumped by the module author when the
/// counter LAYOUT changes (a renamed key, a resized bucket). Two module
/// versions with the same schema version share a counter layout and inherit
/// freely; a bump signals "the old counters do not fit as-is".
pub type SchemaVersion = u32;

/// One module's live counters plus the schema version they are laid out for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counters {
    pub schema: SchemaVersion,
    pub values: BTreeMap<String, i64>,
}

impl Counters {
    pub fn new(schema: SchemaVersion) -> Counters {
        Counters {
            schema,
            values: BTreeMap::new(),
        }
    }
}

/// A counter map: key -> value. The unit a module owns and a migration maps.
pub type CounterMap = BTreeMap<String, i64>;

/// A pure counter-map transform — the body of a [`Migration`].
pub type MigrationFn = dyn Fn(&CounterMap) -> CounterMap + Send + Sync;

/// A declared migration from one schema version to the next: a pure function
/// over the old counter map. Declared alongside the module (in its manifest,
/// conceptually) so a swap that bumps the schema can carry state forward
/// instead of dropping it. Boxed so a manifest can carry an ordered chain.
pub struct Migration {
    pub from: SchemaVersion,
    pub to: SchemaVersion,
    /// Maps the old values to the new layout. Pure; runs under the swap lock.
    pub map: Box<MigrationFn>,
}

impl std::fmt::Debug for Migration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Migration")
            .field("from", &self.from)
            .field("to", &self.to)
            .finish()
    }
}

/// What a migration attempt did — the observable, logged outcome of a
/// stateful-module swap. Every branch is a STATED edge (docs/03).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// Schema unchanged: counters carried forward untouched.
    Inherited { schema: SchemaVersion, keys: usize },
    /// Schema bumped and a migration mapped the old counters to the new
    /// layout: counters carried forward, transformed.
    Migrated {
        from: SchemaVersion,
        to: SchemaVersion,
        keys: usize,
    },
    /// Schema bumped with NO migration declared: counters RESET to zero. The
    /// bounded-overspend window is the time until the next telemetry cycle
    /// re-establishes the counts — stated here, never hidden.
    Reset {
        from: SchemaVersion,
        to: SchemaVersion,
        /// The stated bounded window (seconds) during which every counter
        /// reads zero. The operator declares this per module; the default is
        /// the control-plane share-rebalance interval.
        window_secs: u64,
    },
    /// The module is new to this node (first bind): counters initialized
    /// empty at the new schema. Not a migration — a genesis.
    Initialized { schema: SchemaVersion },
}

/// Compute the migration of `old` counters to a module at `new_schema`,
/// given the declared `migrations`. Pure — the actual store mutation is
/// [`ModuleState::rebind`]; this is the decision, unit-testable in isolation.
///
/// `reset_window_secs` is the operator-declared bound applied when a schema
/// bump has no migration path: the counters reset and this window is the
/// published staleness edge.
pub fn plan(
    old: Option<&Counters>,
    new_schema: SchemaVersion,
    migrations: &[Migration],
    reset_window_secs: u64,
) -> (Counters, MigrationOutcome) {
    let Some(old) = old else {
        // First bind on this node: genesis, empty counters at the new schema.
        return (
            Counters::new(new_schema),
            MigrationOutcome::Initialized { schema: new_schema },
        );
    };

    if old.schema == new_schema {
        // Same layout: inherit untouched. The common hot-swap (a code change
        // that does not touch the counter layout).
        return (
            old.clone(),
            MigrationOutcome::Inherited {
                schema: new_schema,
                keys: old.values.len(),
            },
        );
    }

    // Schema bumped: look for a migration chain old.schema -> new_schema.
    if let Some(values) = try_migrate(&old.values, old.schema, new_schema, migrations) {
        let keys = values.len();
        return (
            Counters {
                schema: new_schema,
                values,
            },
            MigrationOutcome::Migrated {
                from: old.schema,
                to: new_schema,
                keys,
            },
        );
    }

    // No migration path: RESET with the stated bounded window.
    (
        Counters::new(new_schema),
        MigrationOutcome::Reset {
            from: old.schema,
            to: new_schema,
            window_secs: reset_window_secs,
        },
    )
}

/// Follow the declared migrations from `from` to `to`, applying each in
/// order. Returns `None` if no complete chain exists (-> reset). Chains are
/// followed greedily by `from == current`; a gap breaks the chain.
fn try_migrate(
    values: &BTreeMap<String, i64>,
    from: SchemaVersion,
    to: SchemaVersion,
    migrations: &[Migration],
) -> Option<BTreeMap<String, i64>> {
    if from > to {
        // Downgrade: never auto-migrated (a rollback resets, bounded window).
        return None;
    }
    let mut current = from;
    let mut acc = values.clone();
    while current < to {
        let step = migrations.iter().find(|m| m.from == current)?;
        acc = (step.map)(&acc);
        current = step.to;
    }
    (current == to).then_some(acc)
}

/// The out-of-snapshot store of every module's counters, keyed by module
/// name — the exact pattern GB-5's `NodeBudgets` uses so counters survive a
/// config swap. `rebind` is called by the snapshot swap path when a module's
/// version changes; it runs [`plan`] and applies the result atomically.
#[derive(Default)]
pub struct ModuleState {
    inner: Mutex<BTreeMap<String, Counters>>,
}

impl ModuleState {
    pub fn new() -> ModuleState {
        ModuleState::default()
    }

    /// Rebind `module` to `new_schema` under the declared `migrations`,
    /// returning the observable outcome. Called on every snapshot swap that
    /// changes a module's bound version — the migration hook the doc demands.
    pub fn rebind(
        &self,
        module: &str,
        new_schema: SchemaVersion,
        migrations: &[Migration],
        reset_window_secs: u64,
    ) -> MigrationOutcome {
        let mut guard = self.inner.lock().expect("module state lock");
        let (next, outcome) = plan(guard.get(module), new_schema, migrations, reset_window_secs);
        guard.insert(module.to_string(), next);
        outcome
    }

    /// A module's current counters (a clone), for a hook that reads them or a
    /// test that asserts on them.
    pub fn counters(&self, module: &str) -> Option<Counters> {
        self.inner.lock().expect("module state lock").get(module).cloned()
    }

    /// Mutate a module's counter — the running update a stateful hook makes
    /// (e.g. increment a per-tenant tally). Returns the new value. A module
    /// not yet bound is a no-op returning 0 (a hook cannot count before the
    /// module is bound).
    pub fn bump(&self, module: &str, key: &str, delta: i64) -> i64 {
        let mut guard = self.inner.lock().expect("module state lock");
        let Some(counters) = guard.get_mut(module) else {
            return 0;
        };
        let entry = counters.values.entry(key.to_string()).or_insert(0);
        *entry += delta;
        *entry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default reset window used by the tests — the doc's "bounded window".
    const RESET_WINDOW: u64 = 30;

    fn counters(schema: SchemaVersion, pairs: &[(&str, i64)]) -> Counters {
        Counters {
            schema,
            values: pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        }
    }

    #[test]
    fn first_bind_initializes_empty_at_the_new_schema() {
        let (next, outcome) = plan(None, 1, &[], RESET_WINDOW);
        assert_eq!(next, Counters::new(1));
        assert_eq!(outcome, MigrationOutcome::Initialized { schema: 1 });
    }

    #[test]
    fn same_schema_inherits_untouched() {
        let old = counters(1, &[("tenant-a", 42), ("tenant-b", 7)]);
        let (next, outcome) = plan(Some(&old), 1, &[], RESET_WINDOW);
        assert_eq!(next, old, "counters carry forward untouched");
        assert_eq!(outcome, MigrationOutcome::Inherited { schema: 1, keys: 2 });
    }

    #[test]
    fn schema_bump_with_a_migration_inherits_transformed() {
        // v1 -> v2 doubles every counter (a bucket resize, say).
        let migrations = vec![Migration {
            from: 1,
            to: 2,
            map: Box::new(|old| old.iter().map(|(k, v)| (k.clone(), v * 2)).collect()),
        }];
        let old = counters(1, &[("tenant-a", 10)]);
        let (next, outcome) = plan(Some(&old), 2, &migrations, RESET_WINDOW);
        assert_eq!(next.schema, 2);
        assert_eq!(next.values["tenant-a"], 20);
        assert_eq!(outcome, MigrationOutcome::Migrated { from: 1, to: 2, keys: 1 });
    }

    #[test]
    fn a_multi_step_migration_chain_is_followed() {
        // v1 -> v2 -> v3, applied in order.
        let migrations = vec![
            Migration {
                from: 1,
                to: 2,
                map: Box::new(|old| old.iter().map(|(k, v)| (k.clone(), v + 1)).collect()),
            },
            Migration {
                from: 2,
                to: 3,
                map: Box::new(|old| old.iter().map(|(k, v)| (k.clone(), v * 10)).collect()),
            },
        ];
        let old = counters(1, &[("x", 5)]);
        let (next, outcome) = plan(Some(&old), 3, &migrations, RESET_WINDOW);
        // (5 + 1) * 10 = 60.
        assert_eq!(next.values["x"], 60);
        assert_eq!(outcome, MigrationOutcome::Migrated { from: 1, to: 3, keys: 1 });
    }

    #[test]
    fn schema_bump_without_a_migration_resets_with_a_stated_window() {
        let old = counters(1, &[("tenant-a", 999)]);
        let (next, outcome) = plan(Some(&old), 2, &[], RESET_WINDOW);
        // Reset: counters zeroed at the new schema.
        assert_eq!(next, Counters::new(2));
        // The bound is STATED, not silent — this is the whole point.
        assert_eq!(
            outcome,
            MigrationOutcome::Reset {
                from: 1,
                to: 2,
                window_secs: RESET_WINDOW,
            }
        );
    }

    #[test]
    fn a_broken_migration_chain_falls_back_to_reset() {
        // A migration for 1->2 exists but the target is schema 3: the chain
        // breaks at 2 (no 2->3), so it resets rather than partially migrating.
        let migrations = vec![Migration {
            from: 1,
            to: 2,
            map: Box::new(|old| old.clone()),
        }];
        let old = counters(1, &[("x", 1)]);
        let (_, outcome) = plan(Some(&old), 3, &migrations, RESET_WINDOW);
        assert!(matches!(outcome, MigrationOutcome::Reset { from: 1, to: 3, .. }));
    }

    #[test]
    fn a_downgrade_resets_never_reverse_migrates() {
        let old = counters(2, &[("x", 1)]);
        let (next, outcome) = plan(Some(&old), 1, &[], RESET_WINDOW);
        assert_eq!(next, Counters::new(1));
        assert!(matches!(outcome, MigrationOutcome::Reset { from: 2, to: 1, .. }));
    }

    #[test]
    fn module_state_rebind_and_bump_across_a_swap() {
        let state = ModuleState::new();
        // v1 module binds (genesis), accrues counters.
        assert_eq!(
            state.rebind("quota", 1, &[], RESET_WINDOW),
            MigrationOutcome::Initialized { schema: 1 }
        );
        state.bump("quota", "tenant-a", 5);
        state.bump("quota", "tenant-a", 3);
        assert_eq!(state.counters("quota").unwrap().values["tenant-a"], 8);

        // A same-schema swap (new code, same layout) inherits the 8.
        assert_eq!(
            state.rebind("quota", 1, &[], RESET_WINDOW),
            MigrationOutcome::Inherited { schema: 1, keys: 1 }
        );
        assert_eq!(state.counters("quota").unwrap().values["tenant-a"], 8);

        // A schema bump with a migration carries it forward transformed.
        let migrations = vec![Migration {
            from: 1,
            to: 2,
            map: Box::new(|old| old.iter().map(|(k, v)| (k.clone(), v * 100)).collect()),
        }];
        assert_eq!(
            state.rebind("quota", 2, &migrations, RESET_WINDOW),
            MigrationOutcome::Migrated { from: 1, to: 2, keys: 1 }
        );
        assert_eq!(state.counters("quota").unwrap().values["tenant-a"], 800);
    }
}
