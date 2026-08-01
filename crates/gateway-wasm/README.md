# gateway-wasm — tier-2 WASM policy modules (Phase 4)

Signed WASM policy modules on wasmtime: the tier-2 extensibility half of
docs/02 (tier 1 is CEL). A **library**, not a binary — the two-binary budget
(`gatewayd` + `gatewayctl`) holds. `gatewayd` embeds this host to run modules
on the request/response path; `gatewayctl` calls its signature verifier at
admission. `wasmtime` is the one new dependency family this phase adds.

## The measured per-event hot-path cost (the named risk)

docs/04 names *"WASM on the hot path"* a risk: per-event hooks on streaming
paths need real performance validation **before we promise them**. This phase
measured it and made the honest call.

**Measurement** (`benches/hotpath.rs`, and printed by the demo): a realistic
streaming response — a `MessageStart`, 200 `ContentDelta`s, a `UsageDelta`, a
`MessageEnd` — run through the meter twice: once with no WASM (the Phase 1–3
hot path), once with a trivial signed `on_response_event` module invoked **per
event**. The delta divided by the event count is the added per-event cost.

| Path | Per event | Throughput |
|---|---|---|
| baseline (no WASM) | ~0.002–0.004 µs | ~250–380 M events/s |
| **WASM per event** (fresh-instance isolation) | **~11.7 µs** (release) / ~12.7 µs (debug) | ~78–86 K events/s |
| **added per event** | **~11.7 µs** | ~100% throughput collapse |

Reproduce: `cargo bench -p gateway-wasm`, or `cargo run -p gateway-wasm
--example wasm_demo --release` (proof 4). The committed log is `demo.log`.

**The honest call.** ~11.7 µs/event is **too expensive to promise on the
per-event streaming path.** The cost is dominated by the fresh-`Store` +
instantiate the host does **per invocation** — deliberate, for per-invocation
isolation (a guest's globals never leak between events, the same property CEL
gets for free). So:

- **`on_request` and `on_response_end` are PROMISED.** Once per request / once
  per stream — a ~12 µs one-off is negligible against an LLM round-trip.
- **Per-event streaming hooks (`on_response_event`) are GATED OFF by default**
  behind the measured budget. The config flag `wasm.per_event_hooks` defaults
  to `false`; a module may declare `on_response_event`, but the data plane
  does **not** invoke it per event unless the operator explicitly enables the
  gate, having read this number. The proxy double-gates it: config-on **and** a
  module implements the hook, resolved **once** at request bind, so a node with
  the gate off pays **zero** per-event cost (one bool check).

This is the *valid, expected* Phase 4 outcome docs/04 anticipates: a clean,
measured *"not on the per-event streaming path yet, by default"* rather than an
unmeasured promise. A future spike (a wasmtime **pooling allocator** +
pre-instantiated `InstancePre` to amortize the instantiate cost) is the path to
promising per-event hooks; the ABI and gate here do not change when it lands.

## The four walls (why WASM over a native plugin)

1. **No ambient authority** (`host.rs`). The wasmtime `Engine`/`Linker`
   defines **no host imports** — no WASI, no clock, no filesystem, no network.
   A module that even *imports* an unknown function fails to instantiate. A
   guest is a pure function over the bounded ABI context.
2. **Per-invocation fuel** (`Limits::fuel`). Every hook runs under a fixed
   wasmtime fuel budget; exhaustion traps `OutOfFuel` and the host fails the
   route **closed** (a GB-4 reject/cut), never a hang, never `Continue`. This
   is the CEL comprehension-ban lesson (docs/02, `expr.rs`) generalized to
   arbitrary guest code: no guest runs unbounded on the request path.
3. **Epoch preemption** (`Watchdog`). Fuel bounds *work*; an epoch deadline
   bounds *wall time*. A watchdog bumps the engine epoch on a 1 ms tick and the
   default `epoch_deadline` is **10 ticks (~10 ms)** — a real latency ceiling
   with headroom, not the tick itself. A stuck/looping guest traps `Interrupt`
   at the ceiling and fails closed, while a legitimate microsecond hook (which
   cannot straddle 10 ticks) is never spuriously preempted. A deadline of `1`
   against a free-running tick would falsely reject a well-behaved sub-tick
   call, so the ceiling is deliberately several ticks above the tick.
4. **Signed modules only** (`sig.rs`). A module carries a hex HMAC-SHA256
   signature over its exact bytes; the host verifies it against the operator
   key **before** compiling. Unsigned or tampered → rejected. HMAC (symmetric)
   is the same primitive the GB-2 JWT path uses, so **no new crypto dependency
   family** — wasmtime is the only new one. Verified at admission
   (`gatewayctl`, the `no-unsigned-wasm-module` rule) **and** at load
   (`ModuleSet::load`), defense in depth.

## The ABI (narrow and serializable)

A guest exports `memory`, `alloc(i32)->i32`, and any of `on_request` /
`on_response_event` / `on_response_end`, each `(ptr,len)->i64` returning a
packed `(out_ptr<<32)|out_len`. The host copies a bounded JSON context in and a
JSON `Decision` out. The context types (`abi.rs`) are the entire surface a
guest sees:

- `RequestView` — method, path, lowercase headers, **resolved** attribution.
- `EventView` — one canonical `Event` + the running estimated-output tally.
- `EndView` — the reconciled terminal token counts.

Decisions map onto primitives that already exist (reused, not parallel):
`Continue`, `MutateHeaders`, `Reject` (→ the operator's GB-4
`missing_attribution` template), `CutStream` (→ the GB-4 streaming terminal
event, the same machinery GB-5 mid-stream enforcement uses).

Fixtures are `.wat` (loaded via wasmtime's `wat` feature — no external
toolchain): `continue`, `mutate_headers`, `reject`, `cut_stream`, `burn_fuel`
(loops → fuel/epoch termination), `import_host` (imports host I/O → rejected).

## GB-9 hot-swap (full doc-03 semantics)

- **Atomic module binding per snapshot.** The compiled module set is keyed by
  snapshot version and stored **before** the snapshot cell advances (`gatewayd`
  `wasm_runtime.rs`), so a request that reads config vN always finds module set
  vN — config and modules bind together, no torn read. Reuses the existing
  `Arc<Snapshot>` per-request pin.
- **Drain.** A request holds its `BoundModules` (an `Arc`) for its whole life,
  so an in-flight stream keeps its module version until it finishes — exactly
  as it keeps its config version (docs/03 limitation 2).
- **Stateful-module migration (limitation 3).** A module may own counters
  (`state.rs`); they live in a process-lifetime `ModuleState` **outside**
  snapshots (like GB-5's `NodeBudgets`). On a swap, `ModulePlan::diff` runs per
  module: **same schema → inherit** untouched; **schema bump with a declared
  migration → inherit transformed**; **schema bump with no migration → RESET**,
  with the bounded-overspend window **stated** (default 30 s, the rebalance
  interval) and logged, never silent. The mechanism ships; a production
  migration *catalog* is a documented seam (`migrations_for` in `wasm_runtime`).
- **Break-glass with TTL.** `SIGUSR1` pins the empty module set (disable all
  modules) for a bounded, auto-reverting window (`ModuleBreakGlass`, reusing the
  reconciler break-glass shape) — visible, temporary, auto-reverting (docs/04).

## Config

```yaml
wasm:
  per_event_hooks: false        # GATED: per-event streaming hooks off (measured budget)
  modules:
    - name: header-policy
      source: modules/header-policy.wasm   # or .wat, relative to the config
      signature: "<hex HMAC-SHA256 over the module bytes>"
      hooks: [on_request]                  # any of on_request/on_response_event/on_response_end
      schema: 1                            # counter schema version (migration)
      reset_window_secs: 30                # bounded reset window if a schema bump has no migration
```

The operator signing key is `GATEWAYD_WASM_SIGNING_KEY` (a dev key is used,
loudly, when unset). Sign a module with `gateway_wasm::sign(key, bytes)`.

## What is NOT promised (deferred, stated)

- **Per-event streaming hooks by default** — gated behind the measured ~11.7 µs
  budget; enable `per_event_hooks` only having read the number. A pooling /
  `InstancePre` spike is the path to promising them.
- **Asymmetric module signing** — a build system signs, the fleet verifies with
  a public key. Today it is operator-held HMAC (symmetric), which fits the
  operator-runs-the-fleet trust model (docs/02, GB-7). The verification seam
  does not change when the algorithm does.
- **A migration catalog** — the migration *mechanism* ships and is tested; a
  declarative per-module migration catalog (in the manifest / control plane) is
  the `migrations_for` seam, wired empty today (so a schema bump resets with the
  stated window — the honest default).
- **Control-plane-mode module distribution** — file mode loads modules from
  disk; the control plane inlining module bytes into the rendered snapshot is
  the same `WasmModule.source` reference and reuses this loader (Phase 5 fleet
  ergonomics).
- **Counters are in-memory** — like GB-5, wiped on restart and rebuilt from
  telemetry; Postgres durability is deferred (docs/07: runtime state is never
  truth).

## Tests

`cargo test -p gateway-wasm` covers: signature verify (unsigned/tampered/wrong
key rejected), the sandbox (host-import module rejected), fuel termination
fail-closed, epoch preemption (deterministic, watchdog-bumped), the ABI
round-trip, atomic module binding + per-stream drain, and every migration
branch. `tests/sandbox_and_bounds.rs` is the integration proof against the real
host. `gatewayd`'s `wasm_runtime` tests prove the atomic snapshot pairing, the
drain across a real swap, unsigned-fails-bootstrap, and break-glass revert.
