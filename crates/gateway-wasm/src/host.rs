//! The wasmtime host: load, bound, and invoke signed guest policy modules.
//!
//! This is where the "why WASM over native" clause is paid for (docs/04,
//! non-negotiable): a guest runs with NO ambient authority and under two
//! hard resource bounds, so a bespoke org policy cannot do what a native
//! plugin could — read the disk, open a socket, read the clock, or burn the
//! request worker.
//!
//! **Sandbox.** The wasmtime `Engine`/`Linker` here defines NO host
//! imports: no WASI, no clock, no filesystem, no network. A guest that even
//! *imports* an unknown function fails to instantiate. The only host
//! surface is the input bytes the host copies into guest linear memory and
//! the output bytes it copies back — a pure function over the bounded ABI
//! context.
//!
//! **Fuel.** Every invocation is given a fixed [`Limits::fuel`] budget
//! (wasmtime fuel — one unit per wasm operation, roughly). A guest that
//! burns through it traps with `OutOfFuel`; the host turns that into a
//! fail-closed [`Decision`] (a reject/cut to the operator's GB-4 template),
//! never a hang and never a `Continue`. This is the CEL comprehension-ban
//! lesson generalized to arbitrary guest code (docs/04): no guest runs
//! unbounded on the request path.
//!
//! **Epochs.** Fuel bounds *work*; an epoch deadline bounds *wall time*. A
//! guest stuck in a tight loop that somehow evades the fuel accounting (or a
//! host that wants a latency ceiling regardless of op count) is preempted:
//! the host bumps the engine epoch from a watchdog and the guest traps at
//! the next epoch check. Fuel and epochs together mean a stuck/looping
//! module is interrupted at a deadline — the worker is never wedged.
//!
//! Both bounds fail the SAME way: the route fails closed to the operator
//! template. A broken module degrades its own route, never the fleet.

use std::sync::Arc;

use log::warn;
use wasmtime::{Config, Engine, Instance, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};

use crate::abi::{Decision, EndView, EventView, Hook, RequestView};

/// Per-invocation resource bounds. Defaults are conservative — a policy hook
/// is meant to be a small predicate, not a program — and every field is an
/// explicit, stated edge (docs/03: "trust comes from declared edges").
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// wasmtime fuel granted per hook invocation. One unit ~ one wasm op.
    /// Exhausting it traps `OutOfFuel` -> fail closed.
    pub fuel: u64,
    /// Epoch deadline in watchdog ticks. The guest traps once the engine epoch
    /// advances `epoch_deadline` ticks past the value at arm time. This is a
    /// *wall-time latency ceiling*, not a work budget (that is [`Self::fuel`]),
    /// so it must carry real HEADROOM over the watchdog tick: a value of `1`
    /// against a free-running tick preempts any legitimate invocation that
    /// merely straddles a single tick boundary, failing a well-behaved module
    /// closed to GB-4. With a 1ms watchdog tick, the default `10` is a ~10ms
    /// ceiling — orders of magnitude above a real hook's microseconds, so a
    /// legitimate sub-tick call is never spuriously preempted, while a genuinely
    /// stuck/looping guest still traps within the ceiling.
    pub epoch_deadline: u64,
    /// Maximum guest linear memory (bytes). A guest that tries to grow past
    /// this fails the allocation -> fail closed. Bounds a memory-bomb guest.
    pub max_memory_bytes: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            // Enough for a small JSON parse + predicate; a trivial header
            // transform uses a few thousand units. Deliberately tight.
            fuel: 5_000_000,
            // A wall-time ceiling with real headroom over the 1ms watchdog
            // tick: ~10ms. A legitimate hook (microseconds) never straddles
            // enough ticks to trap; a stuck guest still hits the wall. `1`
            // would spuriously preempt a well-behaved sub-tick call — see the
            // field doc.
            epoch_deadline: 10,
            // 16 MiB: room for a real module's tables/stack, far below what
            // a memory bomb would want.
            max_memory_bytes: 16 * 1024 * 1024,
        }
    }
}

/// Why a host invocation did not produce a trustworthy guest decision. Every
/// variant is fail-closed at the call site — the caller substitutes
/// [`Decision::fail_closed`]. Kept distinct so the LOG names exactly which
/// wall the guest hit (a fuel exhaustion and a trap are different
/// operational stories).
#[derive(Debug)]
pub enum HostError {
    /// The module bytes did not compile/validate as wasm.
    Compile(String),
    /// The module imports something the host does not provide (a sandbox
    /// escape attempt) or lacks a required export.
    Instantiate(String),
    /// The guest exhausted its fuel budget.
    OutOfFuel,
    /// The guest hit the epoch deadline (looping / too slow).
    Timeout,
    /// The guest trapped (unreachable, OOB, bad alloc, memory grow refused).
    Trap(String),
    /// The host<->guest byte boundary broke (bad ptr/len, non-UTF8, JSON
    /// that does not parse to a Decision) — a misbehaving guest.
    Boundary(String),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostError::Compile(e) => write!(f, "module failed to compile: {e}"),
            HostError::Instantiate(e) => write!(f, "module failed to instantiate: {e}"),
            HostError::OutOfFuel => write!(f, "guest exhausted its fuel budget"),
            HostError::Timeout => write!(f, "guest hit the epoch deadline (looping/too slow)"),
            HostError::Trap(e) => write!(f, "guest trapped: {e}"),
            HostError::Boundary(e) => write!(f, "host<->guest boundary fault: {e}"),
        }
    }
}

impl std::error::Error for HostError {}

/// Per-store data: the memory/fuel limiter wasmtime consults on every grow.
struct HostState {
    limits: StoreLimits,
}

/// A compiled, ready-to-instantiate policy module. Compilation (the
/// expensive step — cranelift) happens ONCE at load; each invocation is a
/// cheap instantiate + call. Cloneable-cheap: the `Engine` and `Module` are
/// `Arc` inside wasmtime, so a `PolicyModule` is shared across the fleet's
/// request workers.
#[derive(Clone)]
pub struct PolicyModule {
    engine: Engine,
    module: Module,
    limits: Limits,
    /// The module's declared name (from its manifest) — for logs.
    name: Arc<str>,
}

impl PolicyModule {
    /// Compile guest bytes under the sandbox engine. Accepts wasm binary OR
    /// `.wat` text (wasmtime's `wat` feature), so fixtures need no external
    /// toolchain. Fuel and epoch interruption are turned ON at the engine
    /// level here — a module compiled without them could never be bounded.
    ///
    /// This does NOT verify the signature: signature verification is a
    /// separate gate ([`crate::sig::verify`]) the loader runs FIRST, so the
    /// same compile path serves both "load a trusted module" and tests. The
    /// module loader ([`crate::module`]) wires them in the required order.
    pub fn compile(name: &str, bytes: &[u8], limits: Limits) -> Result<PolicyModule, HostError> {
        let mut config = Config::new();
        config.consume_fuel(true);
        config.epoch_interruption(true);
        // No cache, deterministic. Async off — hooks are synchronous.
        let engine = Engine::new(&config).map_err(|e| HostError::Compile(e.to_string()))?;
        let module =
            Module::new(&engine, bytes).map_err(|e| HostError::Compile(e.to_string()))?;
        // Sandbox wall 1: the module may import NOTHING. A guest that tries
        // to import a host function (the classic escape) fails here, at load.
        if let Some(import) = module.imports().next() {
            return Err(HostError::Instantiate(format!(
                "module {name:?} imports {}::{} — policy modules run with no host imports \
                 (no I/O, no clock, no host calls); an importing module is rejected",
                import.module(),
                import.name(),
            )));
        }
        Ok(PolicyModule {
            engine,
            module,
            limits,
            name: Arc::from(name),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The engine, so a watchdog can bump its epoch to preempt a stuck guest.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Invoke `on_request`. Marshals `view` in, runs the guest under the
    /// bounds, and reads a [`Decision`] back — or a [`HostError`] the caller
    /// turns into a fail-closed decision.
    pub fn on_request(&self, view: &RequestView) -> Result<Decision, HostError> {
        let input = serde_json::to_vec(view)
            .map_err(|e| HostError::Boundary(format!("serialize RequestView: {e}")))?;
        self.invoke(Hook::OnRequest, &input)
    }

    /// Invoke `on_response_event` — the HOT path. `view` carries one event
    /// plus the running token tally.
    pub fn on_response_event(&self, view: &EventView) -> Result<Decision, HostError> {
        let input = serde_json::to_vec(view)
            .map_err(|e| HostError::Boundary(format!("serialize EventView: {e}")))?;
        self.invoke(Hook::OnResponseEvent, &input)
    }

    /// Invoke `on_response_end` with the reconciled terminal counts.
    pub fn on_response_end(&self, view: &EndView) -> Result<Decision, HostError> {
        let input = serde_json::to_vec(view)
            .map_err(|e| HostError::Boundary(format!("serialize EndView: {e}")))?;
        self.invoke(Hook::OnResponseEnd, &input)
    }

    /// The one invocation path: fresh instance, fuel armed, epoch deadline
    /// armed, input copied in, hook called, output copied out. A fresh
    /// `Store` per call means guest globals never leak between requests —
    /// per-invocation isolation, the same property CEL gets for free.
    fn invoke(&self, hook: Hook, input: &[u8]) -> Result<Decision, HostError> {
        let state = HostState {
            limits: StoreLimitsBuilder::new()
                .memory_size(self.limits.max_memory_bytes)
                .build(),
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|s| &mut s.limits);

        // Arm the two bounds. Fuel: a fixed budget. Epoch: trap once the
        // engine epoch advances `epoch_deadline` past the current tick — the
        // watchdog ([`Watchdog`]) is what advances it.
        store
            .set_fuel(self.limits.fuel)
            .map_err(|e| HostError::Instantiate(e.to_string()))?;
        store.set_epoch_deadline(self.limits.epoch_deadline);

        let linker: Linker<HostState> = Linker::new(&self.engine);
        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(|e| HostError::Instantiate(e.to_string()))?;

        let out = self.call_hook(&mut store, &instance, hook, input)?;
        serde_json::from_slice::<Decision>(&out)
            .map_err(|e| HostError::Boundary(format!("guest returned non-Decision JSON: {e}")))
    }

    /// Copy `input` into guest memory via its `alloc` export, call the hook
    /// (which returns a packed `(ptr<<32)|len`), and copy the output bytes
    /// out. Every wasmtime error is classified into the fuel/epoch/trap
    /// buckets so the log names the wall.
    fn call_hook(
        &self,
        store: &mut Store<HostState>,
        instance: &Instance,
        hook: Hook,
        input: &[u8],
    ) -> Result<Vec<u8>, HostError> {
        let memory = instance
            .get_memory(&mut *store, "memory")
            .ok_or_else(|| HostError::Instantiate("module exports no `memory`".into()))?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&mut *store, "alloc")
            .map_err(|e| HostError::Instantiate(format!("module exports no `alloc`: {e}")))?;
        let func = instance
            .get_typed_func::<(i32, i32), i64>(&mut *store, hook.export_name())
            .map_err(|e| {
                HostError::Instantiate(format!(
                    "module exports no `{}`: {e}",
                    hook.export_name()
                ))
            })?;

        let len = input.len() as i32;
        let ptr = self.classify(alloc.call(&mut *store, len))?;
        if ptr < 0 {
            return Err(HostError::Boundary("alloc returned a negative pointer".into()));
        }
        memory
            .write(&mut *store, ptr as usize, input)
            .map_err(|e| HostError::Boundary(format!("write input to guest memory: {e}")))?;

        let packed = self.classify(func.call(&mut *store, (ptr, len)))?;
        let out_ptr = (packed >> 32) as u32 as usize;
        let out_len = (packed & 0xffff_ffff) as u32 as usize;
        // Bound the readback: a guest cannot make the host copy an arbitrary
        // span. The output must fit within the (already memory-limited) guest
        // heap; a ptr/len past the end is a boundary fault, fail closed.
        let data = memory.data(&*store);
        let end = out_ptr
            .checked_add(out_len)
            .ok_or_else(|| HostError::Boundary("output span overflows".into()))?;
        if end > data.len() {
            return Err(HostError::Boundary(format!(
                "output span {out_ptr}..{end} exceeds guest memory {}",
                data.len()
            )));
        }
        Ok(data[out_ptr..end].to_vec())
    }

    /// Map a wasmtime call result into a classified [`HostError`]. A trap
    /// carrying wasmtime's `Trap::OutOfFuel` / `Trap::Interrupt` becomes the
    /// specific fuel/timeout error; anything else is a generic trap. This is
    /// the single place guest failure is turned into a NAMED wall.
    fn classify<T>(&self, r: Result<T, wasmtime::Error>) -> Result<T, HostError> {
        r.map_err(|e| {
            if let Some(trap) = e.downcast_ref::<wasmtime::Trap>() {
                match trap {
                    wasmtime::Trap::OutOfFuel => return HostError::OutOfFuel,
                    wasmtime::Trap::Interrupt => return HostError::Timeout,
                    other => return HostError::Trap(other.to_string()),
                }
            }
            HostError::Trap(e.to_string())
        })
    }
}

/// A watchdog that advances an engine's epoch on a fixed tick, so an armed
/// `epoch_deadline` actually fires. One per host is enough — every module
/// shares nothing but wants its epoch bumped; a single background thread
/// bumps ALL registered engines. The data plane starts one at boot.
///
/// Kept minimal: this crate exposes the primitive ([`bump_epoch`]) and a
/// convenience spawner; gatewayd owns the thread lifecycle. Tests bump the
/// epoch by hand to make preemption deterministic (no sleeping).
pub struct Watchdog;

impl Watchdog {
    /// Advance the engine's epoch by one. A stuck guest traps at its next
    /// epoch check once `epoch_deadline` bumps have accumulated since its call
    /// armed (the default is a ~10ms ceiling at a 1ms tick; a test that pins
    /// `epoch_deadline: 1` traps after a single bump). Public so a test can
    /// preempt deterministically and gatewayd's ticker can call it on a timer.
    pub fn bump_epoch(engine: &Engine) {
        engine.increment_epoch();
    }

    /// Spawn a background thread that bumps `engine`'s epoch every
    /// `tick`. Returns immediately; the thread lives for the process. With
    /// `epoch_deadline: N` and tick `t`, a guest is preempted within about
    /// `N * t`. gatewayd calls this once at startup per loaded engine.
    pub fn spawn(engine: Engine, tick: std::time::Duration) {
        std::thread::Builder::new()
            .name("wasm-epoch-watchdog".into())
            .spawn(move || loop {
                std::thread::sleep(tick);
                engine.increment_epoch();
            })
            .map(|_| ())
            .unwrap_or_else(|e| warn!("[wasm] failed to spawn epoch watchdog: {e}"));
    }
}
