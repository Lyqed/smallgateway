//! Integration proofs for the non-negotiable sandbox and resource bounds
//! (docs/04): no ambient I/O reachable, fuel exhaustion terminates
//! fail-closed, and the epoch deadline preempts a stuck guest
//! deterministically. These run the real wasmtime host against the `.wat`
//! fixtures — the same modules the demo loads.

use std::sync::Arc;
use std::time::Duration;

use gateway_wasm::abi::{Decision, RequestView};
use gateway_wasm::host::{HostError, Limits, PolicyModule, Watchdog};
use gateway_wasm::{sign, BoundModules, Hook, ModuleSet, ModuleState, ModuleManifest};

const KEY: &[u8] = b"operator-key";
const IMPORT_WAT: &str = include_str!("../fixtures/import_host.wat");
const BURN_WAT: &str = include_str!("../fixtures/burn_fuel.wat");
const CONTINUE_WAT: &str = include_str!("../fixtures/continue.wat");

/// Sandbox: a module that imports a host function (any ambient capability —
/// filesystem, clock, socket) is REJECTED at compile/load. The host defines
/// no imports, so the guest cannot even NAME a capability it was not given.
#[test]
fn a_module_importing_host_io_is_rejected_at_load() {
    let result = PolicyModule::compile("escape", IMPORT_WAT.as_bytes(), Limits::default());
    match result {
        Err(HostError::Instantiate(msg)) => {
            assert!(
                msg.contains("imports") && msg.contains("no host imports"),
                "rejection must name the import ban: {msg}"
            );
        }
        Err(other) => panic!("expected an Instantiate rejection, got {other}"),
        Ok(_) => panic!("a module importing host I/O must NOT load — sandbox breach"),
    }
}

/// Fuel: a looping module trapped by the fuel budget fails CLOSED (reject),
/// never hangs, never returns Continue — the generalized CEL-DoS lesson.
#[test]
fn a_fuel_exhausting_module_terminates_fail_closed() {
    let limits = Limits { fuel: 200_000, ..Limits::default() };
    let module = PolicyModule::compile("burner", BURN_WAT.as_bytes(), limits).unwrap();
    // The direct host call classifies the wall as OutOfFuel...
    match module.on_request(&RequestView::default()) {
        Err(HostError::OutOfFuel) => {}
        other => panic!("expected OutOfFuel, got {other:?}"),
    }
    // ...and the BoundModules wrapper turns that into a fail-closed reject.
    let set = Arc::new(
        ModuleSet::load(
            &[ManifestBuilder::new("burner", BURN_WAT, Hook::OnRequest).build()],
            KEY,
            limits,
        )
        .unwrap(),
    );
    let bound = BoundModules::new(set, Arc::new(ModuleState::new()));
    assert!(matches!(
        bound.on_request(&RequestView::default()),
        Decision::Reject { .. }
    ));
}

/// Epoch: a stuck guest is preempted at the deadline even when given
/// effectively unlimited fuel. A watchdog bump advances the epoch and the
/// guest traps `Interrupt` -> fail-closed. Deterministic: the test bumps the
/// epoch from another thread instead of sleeping.
#[test]
fn a_looping_module_is_epoch_preempted_with_ample_fuel() {
    // Huge fuel so fuel is NOT what stops it — the epoch must be.
    let limits = Limits {
        fuel: u64::MAX,
        epoch_deadline: 1,
        ..Limits::default()
    };
    let module = PolicyModule::compile("looper", BURN_WAT.as_bytes(), limits).unwrap();

    // A background thread bumps the engine epoch shortly after the call
    // starts; with epoch_deadline=1 the guest traps at the next check.
    let engine = module.engine().clone();
    let bumper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        Watchdog::bump_epoch(&engine);
        // Keep bumping in case the first is missed before the deadline arms.
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(10));
            Watchdog::bump_epoch(&engine);
        }
    });

    let outcome = module.on_request(&RequestView::default());
    bumper.join().unwrap();
    match outcome {
        Err(HostError::Timeout) => {}
        other => panic!("expected epoch Timeout preemption, got {other:?}"),
    }
}

/// The happy path still works end to end: a signed continue module runs under
/// the bounds and returns Continue — the bounds do not break a well-behaved
/// module.
#[test]
fn a_well_behaved_module_runs_under_the_bounds() {
    let module = PolicyModule::compile("noop", CONTINUE_WAT.as_bytes(), Limits::default()).unwrap();
    assert_eq!(
        module.on_request(&RequestView::default()).unwrap(),
        Decision::Continue
    );
}

/// A tiny manifest builder for the tests (signs with the shared KEY).
struct ManifestBuilder {
    name: String,
    wat: String,
    hook: Hook,
}

impl ManifestBuilder {
    fn new(name: &str, wat: &str, hook: Hook) -> ManifestBuilder {
        ManifestBuilder {
            name: name.to_string(),
            wat: wat.to_string(),
            hook,
        }
    }

    fn build(self) -> ModuleManifest {
        let bytes = self.wat.into_bytes();
        let signature = Some(sign(KEY, &bytes));
        ModuleManifest {
            name: self.name,
            bytes,
            signature,
            hooks: vec![self.hook],
            schema: 1,
        }
    }
}
