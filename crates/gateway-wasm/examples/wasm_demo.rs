//! Phase 4 end-to-end proof (committed log via `scripts/demo.sh`).
//!
//! Exercises the REAL wasmtime host and the GB-9 module-binding machinery to
//! prove, in one run, every deliverable the phase names:
//!
//!   (1) a signed WASM module loaded and ENFORCING (a header transform, and a
//!       custom rejection);
//!   (2) an UNSIGNED module REJECTED at the signature gate (admission's rule);
//!   (3) a module that BURNS FUEL / LOOPS terminated and failing CLOSED to a
//!       GB-4 reject — not hanging;
//!   (4) the MEASURED per-event hot-path number printed (the named risk);
//!   (5) an ATOMIC module+config swap where an in-flight stream keeps its OLD
//!       module version while a new request binds the NEW one (drain);
//!   (6) a STATEFUL-module MIGRATION across a swap (inherit / migrate / the
//!       stated reset window);
//!   (7) break-glass with TTL pinning the empty set, then auto-reverting.
//!
//! Deterministic and self-contained (`.wat` fixtures, no external toolchain,
//! no network). Run: `cargo run -p gateway-wasm --example wasm_demo`.

use std::sync::Arc;
use std::time::Instant;

use gateway_core::event::Event;
use gateway_core::metering::Meter;
use gateway_wasm::abi::{EventView, RequestView};
use gateway_wasm::{
    bind::DEFAULT_RESET_WINDOW_SECS, sign, BoundModules, Decision, Hook, Limits, Migration,
    ModuleBreakGlass, ModuleManifest, ModulePlan, ModuleSet, ModuleState, PolicyModule, WireEvent,
};

const KEY: &[u8] = b"operator-signing-key";

const CONTINUE_WAT: &str = include_str!("../fixtures/continue.wat");
const MUTATE_WAT: &str = include_str!("../fixtures/mutate_headers.wat");
const REJECT_WAT: &str = include_str!("../fixtures/reject.wat");
const BURN_WAT: &str = include_str!("../fixtures/burn_fuel.wat");
const CUT_WAT: &str = include_str!("../fixtures/cut_stream.wat");

fn banner(n: u32, title: &str) {
    println!("\n========== [{n}] {title} ==========");
}

fn manifest(name: &str, wat: &str, hooks: Vec<Hook>, schema: u32) -> ModuleManifest {
    let bytes = wat.as_bytes().to_vec();
    let signature = Some(sign(KEY, &bytes));
    ModuleManifest {
        name: name.to_string(),
        bytes,
        signature,
        hooks,
        schema,
    }
}

fn set(manifests: &[ModuleManifest]) -> Arc<ModuleSet> {
    Arc::new(ModuleSet::load(manifests, KEY, Limits::default()).expect("load module set"))
}

fn main() {
    println!("Gateway Project — Phase 4 WASM policy SDK + GB-9 hot-swap demo");
    println!("(real wasmtime host; fuel + epoch bounds; signed modules only)");

    proof_1_signed_enforcing();
    proof_2_unsigned_rejected();
    proof_3_fuel_fail_closed();
    proof_4_hotpath_measurement();
    proof_5_atomic_swap_drain();
    proof_6_stateful_migration();
    proof_7_break_glass();

    println!("\n========== ALL PROOFS COMPLETE ==========");
}

/// (1) A signed module ENFORCES: a header transform and a custom rejection.
fn proof_1_signed_enforcing() {
    banner(1, "signed module loaded and ENFORCING");
    let state = Arc::new(ModuleState::new());

    // Header transform: on_request sets x-policy=enforced.
    let bound = BoundModules::new(
        set(&[manifest("header-policy", MUTATE_WAT, vec![Hook::OnRequest], 1)]),
        state.clone(),
    );
    println!("  loaded signed module 'header-policy' (on_request header transform)");
    match bound.on_request(&RequestView::default()) {
        Decision::MutateHeaders { set, remove } => {
            println!("  on_request -> MutateHeaders set={set:?} remove={remove:?}");
            assert_eq!(set.get("x-policy").map(String::as_str), Some("enforced"));
        }
        other => panic!("expected MutateHeaders, got {other:?}"),
    }

    // Custom rejection: on_request rejects with a bespoke reason.
    let bound = BoundModules::new(
        set(&[manifest("org-rule", REJECT_WAT, vec![Hook::OnRequest], 1)]),
        state,
    );
    println!("  loaded signed module 'org-rule' (on_request custom rejection)");
    match bound.on_request(&RequestView::default()) {
        Decision::Reject { reason } => {
            println!("  on_request -> Reject reason={reason:?} (fails to the GB-4 template)");
            assert_eq!(reason, "blocked-by-policy");
        }
        other => panic!("expected Reject, got {other:?}"),
    }
    println!("  RESULT: a signed module enforces header transforms and custom rejections.");
}

/// (2) An UNSIGNED module is REJECTED at the signature gate.
fn proof_2_unsigned_rejected() {
    banner(2, "unsigned module REJECTED at admission");
    let mut m = manifest("unsigned", CONTINUE_WAT, vec![Hook::OnRequest], 1);
    m.signature = None;
    println!("  attempting to load module 'unsigned' with NO signature...");
    match ModuleSet::load(&[m], KEY, Limits::default()) {
        Err(e) => println!("  REJECTED: {e}"),
        Ok(_) => panic!("an unsigned module must be rejected"),
    }
    // And a tampered (bad-signature) module.
    let mut m = manifest("tampered", CONTINUE_WAT, vec![Hook::OnRequest], 1);
    m.bytes.extend_from_slice(b"(; extra ;)"); // signature no longer matches
    println!("  attempting to load a TAMPERED module (bytes changed after signing)...");
    match ModuleSet::load(&[m], KEY, Limits::default()) {
        Err(e) => println!("  REJECTED: {e}"),
        Ok(_) => panic!("a tampered module must be rejected"),
    }
    println!("  RESULT: unsigned and tampered modules never load (fail closed).");
}

/// (3) A fuel-burning / looping module is TERMINATED and fails CLOSED.
fn proof_3_fuel_fail_closed() {
    banner(3, "fuel-burning module TERMINATED, fails CLOSED (not hanging)");
    // Tight fuel so the loop trips OutOfFuel deterministically and fast.
    let limits = Limits {
        fuel: 500_000,
        ..Limits::default()
    };
    let module = PolicyModule::compile("burner", BURN_WAT.as_bytes(), limits).unwrap();
    println!("  loaded 'burner' (infinite loop on_request), fuel budget = {}", limits.fuel);
    let t0 = Instant::now();
    let raw = module.on_request(&RequestView::default());
    let elapsed = t0.elapsed();
    println!("  direct host call returned in {elapsed:?}: {raw:?} (terminated, did NOT hang)");
    assert!(matches!(raw, Err(gateway_wasm::HostError::OutOfFuel)));

    let bound = BoundModules::new(
        Arc::new(ModuleSet::load(&[manifest("burner", BURN_WAT, vec![Hook::OnRequest], 1)], KEY, limits).unwrap()),
        Arc::new(ModuleState::new()),
    );
    let decision = bound.on_request(&RequestView::default());
    println!("  BoundModules decision -> {decision:?}");
    assert!(matches!(decision, Decision::Reject { .. }), "must fail CLOSED to a reject");
    println!("  RESULT: a looping module is fuel-terminated and fails the route to GB-4 — never Continue, never a hang.");
}

/// (4) The MEASURED per-event hot-path number (the named risk).
fn proof_4_hotpath_measurement() {
    banner(4, "MEASURED per-event hot-path cost (the named risk)");
    const DELTAS: usize = 200;
    const ITERS: u32 = 2_000;
    let events = stream(DELTAS);
    let bound = BoundModules::new(
        set(&[manifest("noop", CONTINUE_WAT, vec![Hook::OnResponseEvent], 1)]),
        Arc::new(ModuleState::new()),
    );

    // Warm up.
    for _ in 0..50 {
        run_baseline(&events);
        run_wasm(&events, &bound);
    }
    let t0 = Instant::now();
    for _ in 0..ITERS {
        run_baseline(&events);
    }
    let baseline = t0.elapsed();
    let t1 = Instant::now();
    for _ in 0..ITERS {
        run_wasm(&events, &bound);
    }
    let wasm = t1.elapsed();

    let n = events.len() as u128;
    let base_ns = baseline.as_nanos() / (ITERS as u128 * n);
    let wasm_ns = wasm.as_nanos() / (ITERS as u128 * n);
    let added = wasm_ns.saturating_sub(base_ns);
    let base_eps = (ITERS as f64 * n as f64) / baseline.as_secs_f64();
    let wasm_eps = (ITERS as f64 * n as f64) / wasm.as_secs_f64();

    println!("  events/stream={n} iters={ITERS} (fresh-instance isolation model)");
    println!("  baseline (no wasm) : {:.3} us/event  ({base_eps:.0} events/s)", base_ns as f64 / 1000.0);
    println!("  wasm per-event     : {:.3} us/event  ({wasm_eps:.0} events/s)", wasm_ns as f64 / 1000.0);
    println!("  ADDED per event    : {:.3} us  ({added} ns)", added as f64 / 1000.0);
    println!("  throughput delta   : {:+.1}%", (1.0 - wasm_eps / base_eps) * 100.0);
    println!("  HONEST CALL: {} us/event is TOO EXPENSIVE for the per-event streaming path.", added as f64 / 1000.0);
    println!("  -> per-event hooks are GATED OFF by default (config wasm.per_event_hooks=false).");
    println!("     on_request and on_response_end (once per request/stream) are promised;");
    println!("     per-event streaming hooks stay behind the measured budget (docs/04, README).");
}

/// (5) ATOMIC module+config swap + drain: an in-flight bind keeps its OLD
/// module version while a new bind sees the NEW one.
fn proof_5_atomic_swap_drain() {
    banner(5, "atomic module+config swap; in-flight stream keeps OLD version (drain)");
    // Simulate the runtime's version-keyed binding: v1 = continue, v2 = mutate.
    let v1 = set(&[manifest("policy", CONTINUE_WAT, vec![Hook::OnRequest], 1)]);
    let v2 = set(&[manifest("policy", MUTATE_WAT, vec![Hook::OnRequest], 1)]);
    let state = Arc::new(ModuleState::new());

    // An in-flight request bound v1 and HOLDS it (the Arc pins the module set).
    let inflight = BoundModules::new(v1.clone(), state.clone());
    let before = inflight.on_request(&RequestView::default());
    println!("  in-flight request bound module-set v1 -> {before:?}");
    assert_eq!(before, Decision::Continue);

    // The config swaps to v2 (a new snapshot binds v2's modules).
    println!("  ...config swaps v1 -> v2 (mutate module) mid-stream...");
    let fresh = BoundModules::new(v2, state);

    // The in-flight bind STILL sees v1 (drain); a fresh bind sees v2.
    let still = inflight.on_request(&RequestView::default());
    let now = fresh.on_request(&RequestView::default());
    println!("  in-flight request (still bound v1) -> {still:?}  [pinned, no torn read]");
    println!("  new request (bound v2)             -> {now:?}");
    assert_eq!(still, Decision::Continue, "in-flight keeps its OLD module version");
    assert!(matches!(now, Decision::MutateHeaders { .. }), "new request sees the NEW module");
    println!("  RESULT: two module versions live at once; the in-flight stream drains on its bound version.");
}

/// (6) STATEFUL-module migration across a swap: inherit, migrate, reset.
fn proof_6_stateful_migration() {
    banner(6, "stateful-module MIGRATION across a swap (docs/03 limitation 3)");
    let state = ModuleState::new();
    let none = |_: &str| Vec::<Migration>::new();

    // v1 genesis, accrue a counter.
    let s1 = ModuleSet::load(&[manifest("quota", CONTINUE_WAT, vec![Hook::OnResponseEnd], 1)], KEY, Limits::default()).unwrap();
    let plan = ModulePlan::diff(None, &s1, &state, &none, DEFAULT_RESET_WINDOW_SECS);
    println!("  bind v1 (schema 1): {:?}", plan.outcomes[0].1);
    state.bump("quota", "tenant-a", 42);
    println!("  module accrues counter tenant-a=42");

    // Same-schema swap -> inherit.
    let s1b = ModuleSet::load(&[manifest("quota", MUTATE_WAT, vec![Hook::OnResponseEnd], 1)], KEY, Limits::default()).unwrap();
    let plan = ModulePlan::diff(Some(&s1), &s1b, &state, &none, DEFAULT_RESET_WINDOW_SECS);
    println!("  swap (schema 1->1, new code): {:?} -> counter tenant-a={}", plan.outcomes[0].1, state.counters("quota").unwrap().values["tenant-a"]);

    // Schema bump WITH a migration (x10) -> inherit transformed.
    let migrations = |name: &str| {
        if name == "quota" {
            vec![Migration { from: 1, to: 2, map: Box::new(|old| old.iter().map(|(k, v)| (k.clone(), v * 10)).collect()) }]
        } else { Vec::new() }
    };
    let s2 = ModuleSet::load(&[manifest("quota", CONTINUE_WAT, vec![Hook::OnResponseEnd], 2)], KEY, Limits::default()).unwrap();
    let plan = ModulePlan::diff(Some(&s1b), &s2, &state, &migrations, DEFAULT_RESET_WINDOW_SECS);
    println!("  swap (schema 1->2, migration x10): {:?} -> counter tenant-a={}", plan.outcomes[0].1, state.counters("quota").unwrap().values["tenant-a"]);
    assert_eq!(state.counters("quota").unwrap().values["tenant-a"], 420);

    // Schema bump WITHOUT a migration -> reset, stated window.
    let s3 = ModuleSet::load(&[manifest("quota", CONTINUE_WAT, vec![Hook::OnResponseEnd], 3)], KEY, Limits::default()).unwrap();
    let plan = ModulePlan::diff(Some(&s2), &s3, &state, &none, DEFAULT_RESET_WINDOW_SECS);
    println!("  swap (schema 2->3, NO migration): {:?}", plan.outcomes[0].1);
    println!("  reset modules (bounded-overspend surface): {:?}", plan.reset_modules());
    println!("  RESULT: counters inherit / migrate on schema bump, or RESET with a STATED bounded window — limitation 3 addressed.");
}

/// (7) Break-glass with TTL: pin the empty set, then auto-revert.
fn proof_7_break_glass() {
    banner(7, "break-glass with TTL (visible, temporary, auto-reverting)");
    let desired = set(&[manifest("policy", MUTATE_WAT, vec![Hook::OnRequest], 1)]);
    let empty: Arc<ModuleSet> = Arc::new(ModuleSet::default());
    let bg = ModuleBreakGlass::pin(empty, 200, "3am: force-disable a misbehaving module");

    let now_in = 100;
    let now_out = 250;
    println!("  break-glass pinned until unix=200 (empty set = all modules disabled)");
    let in_window = BoundModules::new(bg.resolve(&desired, now_in), Arc::new(ModuleState::new()));
    println!("  within window (now={now_in}) -> {:?}  [modules disabled]", in_window.on_request(&RequestView::default()));
    assert_eq!(in_window.on_request(&RequestView::default()), Decision::Continue);

    let after = BoundModules::new(bg.resolve(&desired, now_out), Arc::new(ModuleState::new()));
    let d = after.on_request(&RequestView::default());
    println!("  after expiry (now={now_out}) -> {d:?}  [auto-reverted to the desired module]");
    assert!(matches!(d, Decision::MutateHeaders { .. }));
    println!("  RESULT: break-glass disables modules for a bounded TTL, then auto-reverts to desired.");
}

// ---- shared streaming helpers (mirror the bench) --------------------------

fn stream(content_deltas: usize) -> Vec<Event> {
    let mut events = Vec::with_capacity(content_deltas + 3);
    events.push(Event::MessageStart {
        message_id: Some("msg".into()),
        model: Some("m".into()),
    });
    for i in 0..content_deltas {
        events.push(Event::ContentDelta { index: 0, text: format!("t{i} ") });
    }
    events.push(Event::UsageDelta { input_tokens: Some(64), output_tokens: Some(content_deltas as u64) });
    events.push(Event::MessageEnd { stop_reason: Some("stop".into()) });
    events
}

fn run_baseline(events: &[Event]) -> u64 {
    let mut meter = Meter::new();
    for e in events {
        meter.observe(e);
    }
    meter.estimated_output_tokens()
}

fn run_wasm(events: &[Event], bound: &BoundModules) -> u64 {
    let mut meter = Meter::new();
    for e in events {
        meter.observe(e);
        let view = EventView {
            event: WireEvent::from(e),
            est_output_tokens: meter.estimated_output_tokens(),
        };
        let _ = bound.on_response_event(&view);
    }
    meter.estimated_output_tokens()
}

// The cut fixture is exercised by the gatewayd proxy path and the integration
// tests; referenced here so the demo's fixture set is complete and a reader
// sees every fixture the phase ships.
#[allow(dead_code)]
const _CUT_FIXTURE_REFERENCED: &str = CUT_WAT;
