# The central design questions

*Every architectural argument in this project reduces to one of these. When a
review stalls, name the question being fought over and point here.*

## Q1. Where does truth live?

**Git.** The runtime never becomes authoritative. The control plane's Postgres
holds runtime state (observed spend, fleet health, snapshot ACKs) — never
desired state. The full argument, and the imperative-vs-reconciler tension it
creates during hot swaps, is in [03-hot-swap.md](03-hot-swap.md), Limitation 1.

The corollary question — *what happens when reality must diverge* (3am
incident) — is answered by break-glass with TTL: visible, temporary,
auto-reverting, logged.

## Q2. Buffer or stream?

**Stream, always.** The APIM lesson: a gateway that can only transform what it
buffers makes streaming a second-class citizen, and streaming is the majority
of LLM traffic. The canonical event stream (doc 02) is the foundational bet:
every provider's wire format normalizes into one internal event model, and
every policy — metering, redaction, enforcement, rewriting — operates on
events with backpressure, never on buffered wholes.

## Q3. Who counts the tokens?

**Both, reconciled.** Incremental tally on the event stream for live
enforcement; the provider's terminal usage frame as the authoritative number
for billing and attribution. The delta between the two is published as a known
error bound, not hidden.

## Q4. Where does shared state live?

The trade-off triangle: **central counter** (a hop and a SPOF on every
request) vs **local buckets** (unbounded overspend) vs **hybrid budget
shares** (bounded overspend, rebalanced continuously). We choose budget
shares: the control plane allocates per-data-plane shares from observed spend
telemetry, data planes escalate to synchronous checks only above ~90%
consumption, and the bounded-overspend semantics are documented plainly. This
is the hard distributed-systems problem of the project — GB-5 at fleet scale.

## Q5. Build on top vs use a framework vs greenfield?

The most important balance to strike, and it is answered *per layer*, not once:

| Layer | Answer | Why |
|-------|--------|-----|
| HTTP/proxy engine | **Build on top** — spike Pingora vs embedding agentgateway, two weeks, then commit | Solved problem; novelty budget says no |
| Provider adapters | **Greenfield** | This is where the canonical event model lives; it *is* the product |
| Expression language | **Use existing** — CEL | Sandboxed, no I/O, microseconds; agentgateway validated it in this exact domain |
| Plugin runtime | **Use existing** — WASM (wasmtime) | Component model gives language-agnostic policy modules; we own the SDK, not the VM |
| Control plane / reconciler | **Greenfield** | The actual product; nothing to build on top of exists |
| Fleet transport | **Build on top** — xDS-style versioned snapshots over gRPC | Envoy proved the shape; we need the shape, not Envoy |

The decision rule between *extend* and *fork* and *wait*: extend when the
upstream accepts the change on their timeline (the agentgateway PRs in doc 06
are this path working), fork only when the divergence is the product, wait
when the gap is on someone's public roadmap with a date. Floor-plus-hatch: the
framework is the floor; know where your escape hatch is before you stand on
it.

## Q6. Which programming language — or a new one?

Two different questions hiding as one:

- **The implementation language: Rust.** Not from fashion — from the
  requirements. Streaming proxies need predictable latency (no GC pauses
  inside a token stream), memory safety at the trust boundary, and first-class
  WASM hosting. Pingora and agentgateway — the two candidate foundations — are
  both Rust, so the build-on-top path and the language choice reinforce each
  other. The control plane could be Go by ecosystem gravity, but two languages
  is a tax on a community that needs shared ownership; Rust for both, one
  binary each.
- **The invented language: yes, but small.** The creativity the project can
  afford is a *policy composition language* — the scoped chain format
  (fleet → project → route → app with an explicit `base` marker) plus CEL for
  expressions. Declarative, total (no unbounded loops), diffable in review,
  compilable to a rendered snapshot. Inventing a general-purpose language is
  where the timeframe goes to die; inventing a 200-line-spec DSL that makes
  config reviews readable is exactly the kind of creativity we keep.

## Q7. What do we promise about live change?

GB-9 asks that the rules can change while it runs. We promise **bounded
staleness, honestly stated** — versioned snapshots, ACK/NACK, drain semantics
for in-flight streams, explicit state-migration policy for stateful modules.
What we refuse to promise: instant fleet-wide enforcement over streaming
traffic, because nobody can deliver it and pretending otherwise is how
gateways lose trust. Full treatment in [03-hot-swap.md](03-hot-swap.md).

## Q8. How does a change stay owned for six months?

Mechanically, not aspirationally: every config change is a Git commit with an
author; every rendered snapshot is reproducible from a commit hash; every
break-glass override names its operator and expires; every incident timeline
can be replayed from snapshot history. The six-month rule from doc 00 works
because the system is built so that "what changed and who changed it" is never
an investigation — it is a `git log`.
