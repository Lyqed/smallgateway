# Spike B, candidate 1 — the minimal streaming proxy on Pingora

Phase 0 of [the build plan](../../docs/04-build-plan.md): the foundation
bake-off, Pingora side. A reverse proxy on `pingora-proxy` 0.8.1 (latest on
crates.io at spike time) that forwards to a configurable upstream and taps
the response body **without buffering**: every chunk is fed through the
matching [spike-event-model](../event-model) adapter and a per-request
`Meter`, while the identical bytes stream on to the client.

Provider selection per request: `x-spike-provider` header first, then a
route prefix (`/openai/...`, `/anthropic/...`, `/bedrock/...`), default
OpenAI.

## Run it

```bash
cargo build            # needs rustup; rust-toolchain.toml pins 1.97 (see below)
cargo test             # provider-selection unit tests

# terminal 1 — mock upstream streaming a fixture, one frame per 80ms:
cargo run --bin mock_upstream -- --port 6190 \
  --fixture ../event-model/fixtures/openai.sse --provider openai

# terminal 2 — the proxy (flags or SPIKE_* env vars):
cargo run --bin spike-proxy-pingora -- --listen 127.0.0.1:6188 \
  --upstream-host 127.0.0.1 --upstream-port 6190 --upstream-tls false

# terminal 3:
curl -N -H 'x-spike-provider: openai' http://127.0.0.1:6188/v1/chat/completions -d '{}'
```

Flags/env: `--listen`/`SPIKE_LISTEN`, `--upstream-host`/`SPIKE_UPSTREAM_HOST`,
`--upstream-port`/`SPIKE_UPSTREAM_PORT`, `--upstream-tls`/`SPIKE_UPSTREAM_TLS`,
`--sni`/`SPIKE_UPSTREAM_SNI` (defaults to host).

`scripts/demo.sh` runs the whole proof and writes [demo.log](demo.log)
(committed): for each of the three providers, plus a route-prefix-only run,

- **(a) the client received the full body incrementally** — the timestamped
  client reads arrive one-per-mock-frame at the mock's 80 ms cadence
  (7 frames → 7 reads for OpenAI, 9 → 9 for Anthropic, 11 → 11 for Bedrock),
  so Pingora forwarded each upstream chunk as it arrived, no coalescing, no
  buffering; and
- **(b) the proxy logged the canonical event stream and the metering
  report** — the same events and the same `+11.1%` plumbing-fixture estimate
  error that Spike A's CLI produces on those fixtures. The Bedrock run
  serves the real CRC-framed event-stream encoding split into 96-byte chunks,
  so binary frame boundaries never align with body chunks and the tap still
  reassembles correctly.

The mock upstream (`src/bin/mock_upstream.rs`) is std-only: a `TcpListener`
serving the fixture over HTTP/1.1 chunked transfer with a delay per chunk,
so nothing downstream *can* see the body other than incrementally.

## Bake-off findings

### Integration size

| Piece | LOC |
|---|---|
| `src/main.rs` — ProxyHttp impl + tap + config + bootstrap | 230 |
| `src/provider.rs` — per-request adapter selection (55 code + 40 test) | 95 |
| **Proxy total** | **325** |
| `src/bin/mock_upstream.rs` + `scripts/demo.sh` (proof harness) | 267 |

The Pingora-specific part — the `ProxyHttp` impl plus the ~15-line `main` —
is roughly 140 lines, and it compiled against 0.8.1 on the first attempt.
The tap itself (`response_body_filter`) is ~35 lines including logging.

### Build

- **Cold build: ~20 s wall** (debug; `cargo clean` then `cargo build`,
  17.5 s compile of 222 external crates, 24-core machine, rustc 1.97).
  Release build: another 26 s. Lockfile pins `pingora 0.8.1`.
- **Toolchain gotcha:** pingora 0.8.1 declares `rust-version = 1.84` and its
  2026 dependency tree pulls edition-2024 crates (clap 4.6, hashbrown 0.17,
  indexmap 2.14 …). The repo's ambient toolchain was a stale stable 1.83 and
  failed at manifest parse (`feature edition2024 is required`). This crate
  carries a `rust-toolchain.toml` pinning 1.97. The last pingora that builds
  on ≤1.83 is 0.6.0; a data-plane on Pingora means tracking a fairly fresh
  MSRV.

### How the ProxyHttp body-filter API fits

The hook is exactly the shape our `on_response_event` needs:

```rust
fn response_body_filter(&self, session: &mut Session,
    body: &mut Option<Bytes>, end_of_stream: bool, ctx: &mut Self::CTX)
    -> Result<Option<Duration>>
```

- **Owned bytes, mutable.** `&mut Option<Bytes>` per chunk (`Bytes` is
  refcounted). A tap just reads; a rewriting policy (GB-4 templates, PII
  redaction, dialect translation) can replace the chunk or set `None` to
  withhold it. Chunk boundaries from the upstream read are preserved — the
  demo shows a 1:1 mock-frame-to-client-read mapping — which is precisely
  what the partial-frame push parsers were built for.
- **Per-session ctx.** `type CTX`, created per request by `new_ctx()`, is
  handed `&mut` to every hook. Adapter + `Meter` + counters slot in with no
  ceremony. One wart: `new_ctx()` cannot see the request, so provider
  binding happens in `request_filter` with a default-then-replace dance.
  `CTX: Send + Sync` forces `Box<dyn Adapter + Send + Sync>` — free for
  plain-data parsers.
- **End-of-stream signal.** An explicit `end_of_stream: bool` on the last
  call (empty or not). Reliable in the demo for h1 chunked; this is the
  anchor for Spike A's "usage frame precedes MessageEnd" ordering contract
  and for emitting the metering report.
- **Backpressure semantics.** Verified in `pingora-proxy` source: upstream
  read and downstream write are separate tasks joined by a **bounded mpsc of
  4 `HttpTask`s**, and the upstream read loop waits on a channel permit when
  the downstream is slow (`proxy_h1.rs`: "No permit, wait on more capacity
  to avoid starving"). So a slow client throttles the upstream read with a
  small fixed buffer — the no-whole-response-buffering property holds by
  construction. Bonus: returning `Ok(Some(duration))` from the filter sleeps
  the downstream writer for that chunk — a crude but real per-chunk pacing
  hook.
- **Sync filter.** The body filter is not `async` (request/response *header*
  filters are). Pure-CPU policies (parse, meter, CEL) are fine inline; any
  per-chunk decision needing I/O — GB-5's synchronous budget escalation
  above ~90% — has to hop through a channel to an async task or pre-fetch
  its answer. This is a real design constraint, not a blocker.

### What was awkward

- The MSRV/edition-2024 toolchain chase (above) — the only build friction.
- `new_ctx()` before request visibility; per-request re-init in
  `request_filter`.
- `Server::run_forever()` owns the process (signals, optional daemonize,
  zero-downtime fd-handoff upgrade). Embedding a control-plane client means
  registering it as a pingora background service rather than owning `main`
  ourselves. Workable, but the process model is Pingora's, not ours.
- Sparse examples for body filtering; the accurate mental model (task
  channel, permits, `HttpTask`) came from reading `proxy_h1.rs`, not docs.

### What is missing for our needs

- **Mid-stream cut with a graceful terminal event (GB-4 streaming).** The
  filter cannot force end-of-stream: `end_of_stream` is an input, not an
  output. Options today: (1) return `Err` — aborts the session, client sees
  a truncated chunked body, not an operator-defined terminal SSE event;
  (2) replace the current chunk with the terminal event and swallow every
  subsequent chunk — client sees a clean cut, but the upstream keeps
  generating (and billing) until it finishes. Doing GB-4 properly needs a
  small pingora-proxy change (filter returns "this is the last chunk; finish
  the downstream encoding cleanly and drop upstream") — a fork-and-patch or
  upstream PR. This is the one place where Pingora's abstraction is actively
  in the way, and it is bounded.
- **Per-session policy state beyond one request.** CTX is per-request.
  Cross-request state (budget counters, app quotas) lives in our own shared
  structures — expected for any foundation, just noting Pingora gives no
  primitive.
- **Config hot-swap hooks.** Pingora's story is zero-downtime *binary*
  upgrade and graceful restart; there is no in-process config reload
  facility and the listener set is fixed once services start. The doc-03
  semantics (atomic snapshot binding, drain for in-flight streams, counter
  migration) would be built entirely by us — e.g. `ArcSwap` of a snapshot in
  the `ProxyHttp` struct, drain tracking per session. Pingora neither helps
  nor obstructs.
- **Async per-chunk hooks** for policies that must consult remote state
  mid-stream (GB-5 escalation) — see the sync-filter note above.

### Preliminary verdict

Pingora fits the data plane's core claim almost exactly: the streaming body
filter with owned chunks, per-request ctx, an explicit end-of-stream signal,
and permit-based backpressure is our `on_response_event` shape out of the
box, and the tap-without-buffering property is structural, not fought-for.
The integration cost was one sitting and ~325 lines including config, with a
20-second cold build; nothing in the happy path pushed back. The costs are
equally clear: a fresh-MSRV dependency, a process model Pingora owns, sync
body filters, and — the one genuine gap — no graceful mid-stream cut, which
GB-4 streaming needs and which requires patching pingora-proxy rather than
configuring it. Everything else on our list (scoped policy chains, hot
snapshots, fleet) was always going to be ours regardless of foundation.
Verdict so far: a strong foundation that gives us transport and freedom and
nothing else — exactly what "build the governance ourselves" wants. The
deciding comparison against candidate 2 (agentgateway) is whether its
built-in LLM-awareness and credential paths outweigh owning these four gaps
on a base that has already proven the streaming tap in 35 lines.
