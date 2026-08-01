# gatewayd

The standalone data plane: a config-driven Pingora proxy enforcing GB-1/2/3
(attribution), GB-4 (operator-owned rejections), with a streaming tap and
meter on every response. Phase 1, milestones 1 + 2. Run it:

```
gatewayd --config gateway.yaml [--listen 127.0.0.1:6188] [--poll-interval 3]
```

Proof lives in `scripts/demo.sh` → `demo.log`.

## Hot reload: what is promised

The doc-03 semantics (`docs/03-hot-swap.md`), made real for a single node:

- **Versioned snapshots.** Rendering = load + validate + stamp: every
  accepted config becomes an immutable `Snapshot` with a monotonically
  increasing version and the SHA-256 of its source bytes. Validation is
  fail-fast; a rejected file consumes no version number.
- **Atomic per-request binding.** A request binds one `Arc<Snapshot>` at
  request start and consults only that snapshot for its whole lifetime —
  routing, attribution, upstream choice, metering. A request never sees two
  versions (no torn reads). New requests bind the newest snapshot.
- **Drain.** A swap never rebinds an in-flight request. The old snapshot
  stays resident until its last in-flight stream drops it (Rust
  refcounting, made explicit and tested — `reload.rs` / `proxy.rs` tests),
  so during the overlap two versions are live simultaneously, on purpose.
- **NACK keeps old.** A reload whose file fails validation is REJECTED: the
  old snapshot keeps serving, and the rejection is logged at error level
  with the precise validation errors and the still-active version —
  divergence surfaced, never silent (doc 03, limitation 1).
- **Identical content is a no-op.** The reload path hashes the file first;
  a matching hash logs at debug level and changes nothing.
- **Two triggers, one path.** SIGHUP and a poll-based mtime watcher
  (`--poll-interval` seconds, default 3, `0` disables) both funnel through
  the same reload routine.
- **Versions are observable.** Every `[req]` and `[attr]` line, and the
  end-of-stream `[meter]` line, carries `cfg=vN`; the reload path logs
  old→new version, source hash, and swap timestamp. That is the published
  bounded-staleness evidence: which version metered which stream is a grep.

## What is NOT promised

- **No mid-stream rebind.** A cap or policy tightened mid-stream does not
  apply to streams already running; they finish under the version they
  started with. This is doc 03, limitation 2 — a bounded-staleness
  semantic to state, not a bug to fix. The `cfg=vN` on the `[meter]` line
  is the error bound made visible.
- **No stateful-policy migration yet.** Anything that owns counters
  (budgets, rate limits, quota shares) is a state-migration problem across
  a swap — inherit versioned counters or reset them (doc 03, limitation 3).
  Phase 3/4 scope; nothing in the current config is stateful.
- **Single node.** Fleet distribution (ACK/NACK waves, canary
  configuration) is the control-plane phase; this crate is one node
  latching or rejecting its own file.
