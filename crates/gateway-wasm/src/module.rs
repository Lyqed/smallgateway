//! Module manifests and the loaded module set — the unit that binds
//! atomically with a config snapshot (GB-9, docs/03).
//!
//! A [`ModuleManifest`] is what the operator writes in config: a name, the
//! module bytes' location (inlined here as bytes for the data plane; the
//! control plane renders them into the snapshot), the signature, which hooks
//! it implements, and its counter schema version. Loading a manifest is:
//! **verify the signature FIRST, then compile** — an unsigned or tampered
//! module never even reaches the wasm compiler.
//!
//! A [`ModuleSet`] is the loaded, ready set for one snapshot. Because the
//! set is built at snapshot render time and carried inside the snapshot, a
//! request that binds `Arc<Snapshot>` binds the config version AND the exact
//! module set together — there is no torn read where the config is vN but a
//! module is vN-1 (docs/04: atomic module binding per snapshot, reusing the
//! existing SharedSnapshot per-request binding). The drain semantics fall
//! out for free: an in-flight stream holds its `Arc<Snapshot>`, so it holds
//! its module set, until it finishes.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::abi::Hook;
use crate::host::{HostError, Limits, PolicyModule};
use crate::sig::{self, SigError};
use crate::state::SchemaVersion;

/// One module as the operator declares it. The bytes are the wasm binary (or
/// `.wat`); `signature` is the hex HMAC over exactly those bytes.
#[derive(Debug, Clone)]
pub struct ModuleManifest {
    pub name: String,
    pub bytes: Vec<u8>,
    /// Hex HMAC-SHA256 over `bytes`. `None` -> unsigned -> rejected.
    pub signature: Option<String>,
    /// Which hooks this module implements. A module that does NOT list
    /// [`Hook::OnResponseEvent`] is never called per event — the phase's
    /// gate for keeping the hot path clear when a module only needs
    /// `on_request` (docs/04).
    pub hooks: Vec<Hook>,
    /// The counter schema version this module's state is laid out for
    /// (docs/03 limitation 3). A swap that changes it triggers migration.
    pub schema: SchemaVersion,
}

/// Why a module failed to load. Signature failures and compile/instantiate
/// failures are distinct — the first is an admission concern, the second a
/// build concern — but both are fail-closed: the module does not load, and
/// (per the snapshot render contract) the whole snapshot is rejected.
#[derive(Debug)]
pub enum LoadError {
    Signature { name: String, err: SigError },
    Host { name: String, err: HostError },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Signature { name, err } => {
                write!(f, "module {name:?} rejected: {err}")
            }
            LoadError::Host { name, err } => write!(f, "module {name:?} failed to load: {err}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// A loaded module: the compiled [`PolicyModule`], the declared hook set, and
/// the schema version. Cheap to clone (the compiled module is `Arc` inside).
#[derive(Clone)]
pub struct LoadedModule {
    module: PolicyModule,
    hooks: Vec<Hook>,
    schema: SchemaVersion,
}

impl LoadedModule {
    pub fn policy(&self) -> &PolicyModule {
        &self.module
    }

    pub fn schema(&self) -> SchemaVersion {
        self.schema
    }

    pub fn name(&self) -> &str {
        self.module.name()
    }

    /// Whether this module wants to be called for `hook`. The hot-path gate:
    /// `implements(OnResponseEvent)` is checked ONCE at bind, and a module
    /// that does not implement it is never invoked per event.
    pub fn implements(&self, hook: Hook) -> bool {
        self.hooks.contains(&hook)
    }
}

/// The loaded module set bound to one snapshot. Ordered by manifest order so
/// a chain of modules runs deterministically (the first module to return a
/// non-`Continue` decision wins, exactly like the policy chain composes).
#[derive(Clone, Default)]
pub struct ModuleSet {
    modules: Vec<LoadedModule>,
    /// name -> index, for migration bookkeeping across a swap.
    by_name: BTreeMap<String, usize>,
}

impl ModuleSet {
    /// Load a set of manifests: verify EACH signature against `signing_key`,
    /// then compile. Any failure fails the WHOLE set (and thus the snapshot)
    /// — a partially-loaded module set is never served. `limits` are the
    /// per-invocation bounds every module in the set runs under.
    ///
    /// This is the ONE place signature verification and compilation are
    /// sequenced; the control-plane admission gate calls
    /// [`crate::sig::verify`] on the same bytes for the same verdict BEFORE
    /// render, so an unsigned module is caught at admission and, defense in
    /// depth, again here at load.
    pub fn load(
        manifests: &[ModuleManifest],
        signing_key: &[u8],
        limits: Limits,
    ) -> Result<ModuleSet, LoadError> {
        let mut modules = Vec::with_capacity(manifests.len());
        let mut by_name = BTreeMap::new();
        for manifest in manifests {
            // Signature FIRST — an unsigned/tampered module never compiles.
            sig::verify(signing_key, &manifest.bytes, manifest.signature.as_deref()).map_err(
                |err| LoadError::Signature {
                    name: manifest.name.clone(),
                    err,
                },
            )?;
            let module = PolicyModule::compile(&manifest.name, &manifest.bytes, limits).map_err(
                |err| LoadError::Host {
                    name: manifest.name.clone(),
                    err,
                },
            )?;
            by_name.insert(manifest.name.clone(), modules.len());
            modules.push(LoadedModule {
                module,
                hooks: manifest.hooks.clone(),
                schema: manifest.schema,
            });
        }
        Ok(ModuleSet { modules, by_name })
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }

    /// The modules that implement `hook`, in manifest order — the hot-path
    /// caller iterates exactly these, so a set with no `on_response_event`
    /// module adds ZERO per-event cost.
    pub fn for_hook(&self, hook: Hook) -> impl Iterator<Item = &LoadedModule> {
        self.modules.iter().filter(move |m| m.implements(hook))
    }

    /// Look a module up by name (for state migration bookkeeping).
    pub fn get(&self, name: &str) -> Option<&LoadedModule> {
        self.by_name.get(name).map(|&i| &self.modules[i])
    }

    /// The (name, schema) of every module — what the swap path diffs against
    /// the old set to decide which modules migrate.
    pub fn schemas(&self) -> impl Iterator<Item = (&str, SchemaVersion)> {
        self.modules.iter().map(|m| (m.name(), m.schema))
    }
}

/// A shared, bindable module set — an `Arc<ModuleSet>`, so a snapshot carries
/// one and a request pins it with its snapshot. Named for symmetry with the
/// config snapshot's `Arc`; the atomicity is the same mechanism.
pub type SharedModuleSet = Arc<ModuleSet>;

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"operator-key";

    /// A minimal valid guest that implements all three hooks: `alloc` bumps a
    /// static bump-pointer, and each hook writes `{"decision":"continue"}`.
    /// `.wat`, so no external toolchain. This same fixture backs the host
    /// tests; kept here inline so the loader tests are self-contained.
    const CONTINUE_WAT: &str = include_str!("../fixtures/continue.wat");

    fn signed(name: &str, hooks: Vec<Hook>, schema: SchemaVersion) -> ModuleManifest {
        let bytes = CONTINUE_WAT.as_bytes().to_vec();
        let signature = Some(sig::sign(KEY, &bytes));
        ModuleManifest {
            name: name.to_string(),
            bytes,
            signature,
            hooks,
            schema,
        }
    }

    #[test]
    fn a_signed_module_set_loads() {
        let manifests = vec![signed("headers", vec![Hook::OnRequest], 1)];
        let set = ModuleSet::load(&manifests, KEY, Limits::default()).unwrap();
        assert_eq!(set.len(), 1);
        assert!(set.get("headers").unwrap().implements(Hook::OnRequest));
    }

    #[test]
    fn an_unsigned_module_fails_the_whole_set() {
        let mut m = signed("headers", vec![Hook::OnRequest], 1);
        m.signature = None;
        // `ModuleSet` holds compiled wasmtime handles (not Debug), so inspect
        // the Err arm directly rather than `unwrap_err`.
        let load = ModuleSet::load(&[m], KEY, Limits::default());
        assert!(
            matches!(load, Err(LoadError::Signature { err: SigError::Missing, .. })),
            "expected Signature/Missing"
        );
    }

    #[test]
    fn a_bad_signature_fails_the_whole_set() {
        let mut m = signed("headers", vec![Hook::OnRequest], 1);
        m.bytes.extend_from_slice(b"(; tamper ;)"); // sig no longer matches
        let load = ModuleSet::load(&[m], KEY, Limits::default());
        assert!(
            matches!(load, Err(LoadError::Signature { err: SigError::Mismatch, .. })),
            "expected Signature/Mismatch"
        );
    }

    #[test]
    fn for_hook_only_yields_modules_that_implement_it() {
        // One request-only module, one event module: the hot-path iterator
        // sees only the event module.
        let manifests = vec![
            signed("req-only", vec![Hook::OnRequest], 1),
            signed("evt", vec![Hook::OnResponseEvent], 1),
        ];
        let set = ModuleSet::load(&manifests, KEY, Limits::default()).unwrap();
        let event_modules: Vec<&str> =
            set.for_hook(Hook::OnResponseEvent).map(|m| m.name()).collect();
        assert_eq!(event_modules, vec!["evt"]);
        let request_modules: Vec<&str> =
            set.for_hook(Hook::OnRequest).map(|m| m.name()).collect();
        assert_eq!(request_modules, vec!["req-only"]);
    }
}
