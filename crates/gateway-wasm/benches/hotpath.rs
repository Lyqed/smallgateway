//! THE NAMED-RISK MEASUREMENT (docs/04): the per-event WASM hook cost on a
//! streaming path, measured against the no-WASM baseline.
//!
//! The whole point of Phase 4 is to MEASURE this before promising per-event
//! hooks. This bench builds a realistic streaming response — many canonical
//! events through the meter, exactly as the data-plane tap does — twice:
//!
//! - **baseline**: meter each event, no WASM (the Phase 1-3 hot path).
//! - **wasm_per_event**: the same, plus a trivial signed `on_response_event`
//!   module invoked PER EVENT (the phase's new capability under test).
//!
//! The delta between the two, divided by the event count, is the added
//! per-event WASM cost in microseconds. criterion reports it; the README
//! records the number and the honest call (promise per-event, or gate it).
//!
//! Run: `cargo bench -p gateway-wasm`. A plain `main` (below, gated to a
//! `--nocapture`-friendly run under `cargo test --bench` is not used;
//! criterion drives it) prints a one-line human summary too.

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};

use gateway_core::event::Event;
use gateway_core::metering::Meter;
use gateway_wasm::{
    BoundModules, Hook, Limits, ModuleManifest, ModuleSet, ModuleState, WireEvent,
};
use gateway_wasm::abi::EventView;

const KEY: &[u8] = b"bench-operator-key";
const CONTINUE_WAT: &str = include_str!("../fixtures/continue.wat");

/// A realistic streaming response: a start, N content deltas, a usage frame,
/// an end — the shape the adapters emit and the meter consumes.
fn stream(content_deltas: usize) -> Vec<Event> {
    let mut events = Vec::with_capacity(content_deltas + 3);
    events.push(Event::MessageStart {
        message_id: Some("msg_bench".into()),
        model: Some("bench-model".into()),
    });
    for i in 0..content_deltas {
        events.push(Event::ContentDelta {
            index: 0,
            text: format!("token-{i} "),
        });
    }
    events.push(Event::UsageDelta {
        input_tokens: Some(128),
        output_tokens: Some(content_deltas as u64),
    });
    events.push(Event::MessageEnd {
        stop_reason: Some("stop".into()),
    });
    events
}

fn bound_continue() -> BoundModules {
    let bytes = CONTINUE_WAT.as_bytes().to_vec();
    let signature = Some(gateway_wasm::sign(KEY, &bytes));
    let manifest = ModuleManifest {
        name: "bench-noop".into(),
        bytes,
        signature,
        hooks: vec![Hook::OnResponseEvent],
        schema: 1,
    };
    let set = Arc::new(ModuleSet::load(&[manifest], KEY, Limits::default()).unwrap());
    BoundModules::new(set, Arc::new(ModuleState::new()))
}

/// The baseline hot path: meter every event, no WASM.
fn run_baseline(events: &[Event]) -> u64 {
    let mut meter = Meter::new();
    for e in events {
        meter.observe(e);
    }
    meter.estimated_output_tokens()
}

/// The WASM hot path: meter every event AND invoke the per-event hook.
fn run_wasm(events: &[Event], bound: &BoundModules) -> u64 {
    let mut meter = Meter::new();
    for e in events {
        meter.observe(e);
        let view = EventView {
            event: WireEvent::from(e),
            est_output_tokens: meter.estimated_output_tokens(),
        };
        // The decision is consumed exactly as the tap would (continue/cut).
        let _ = black_box(bound.on_response_event(&view));
    }
    meter.estimated_output_tokens()
}

fn hotpath(c: &mut Criterion) {
    const DELTAS: usize = 200; // a medium streaming response
    let events = stream(DELTAS);
    let bound = bound_continue();

    // A one-line human-readable summary printed before criterion's own stats,
    // so a demo log captures the number without parsing criterion output.
    print_summary(&events, &bound);

    let mut group = c.benchmark_group("hotpath");
    group.throughput(Throughput::Elements(events.len() as u64));
    group.bench_function("baseline_no_wasm", |b| {
        b.iter(|| black_box(run_baseline(black_box(&events))))
    });
    group.bench_function("wasm_per_event", |b| {
        b.iter(|| black_box(run_wasm(black_box(&events), black_box(&bound))))
    });
    group.finish();
}

/// Hand-rolled A/B timing over many iterations, printed as the demo's
/// headline number (criterion's HTML/stats are the rigorous form; this is
/// the greppable one-liner the committed demo log shows).
fn print_summary(events: &[Event], bound: &BoundModules) {
    const ITERS: u32 = 2_000;
    // Warm up (compile caches, branch predictors).
    for _ in 0..50 {
        black_box(run_baseline(events));
        black_box(run_wasm(events, bound));
    }

    let t0 = Instant::now();
    for _ in 0..ITERS {
        black_box(run_baseline(black_box(events)));
    }
    let baseline = t0.elapsed();

    let t1 = Instant::now();
    for _ in 0..ITERS {
        black_box(run_wasm(black_box(events), black_box(bound)));
    }
    let wasm = t1.elapsed();

    let n_events = events.len() as u128;
    let base_per_event_ns = baseline.as_nanos() / (ITERS as u128 * n_events);
    let wasm_per_event_ns = wasm.as_nanos() / (ITERS as u128 * n_events);
    let added_ns = wasm_per_event_ns.saturating_sub(base_per_event_ns);

    // Throughput: events per second on each path.
    let base_eps = (ITERS as f64 * n_events as f64) / baseline.as_secs_f64();
    let wasm_eps = (ITERS as f64 * n_events as f64) / wasm.as_secs_f64();
    let throughput_delta_pct = (1.0 - wasm_eps / base_eps) * 100.0;

    eprintln!("[hotpath-measurement] events/stream={n_events} iters={ITERS}");
    eprintln!(
        "[hotpath-measurement] baseline={:.3}us/event  wasm={:.3}us/event  \
         added={:.3}us/event ({added_ns}ns)",
        base_per_event_ns as f64 / 1000.0,
        wasm_per_event_ns as f64 / 1000.0,
        added_ns as f64 / 1000.0,
    );
    eprintln!(
        "[hotpath-measurement] throughput baseline={base_eps:.0} ev/s  wasm={wasm_eps:.0} ev/s  \
         delta={throughput_delta_pct:+.1}%",
    );
}

criterion_group!(benches, hotpath);
criterion_main!(benches);
