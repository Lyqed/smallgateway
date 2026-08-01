# Build plan — from scratch, step by step

*Risk-ordered: the highest-risk novel claim gets validated first, and every
phase ships something a platform team can run. Anti-goal at every step: don't
accrete Spinnaker.*

## Phase 0 — the spikes (weeks 1–2, throwaway code)

Nothing gets committed to until both spikes report.

**Spike A: the canonical event model.** Normalize three providers — OpenAI SSE
deltas, Anthropic events, Bedrock event-stream — into the internal event model
(`MessageStart / ContentDelta / ToolCallDelta / UsageDelta / MessageEnd /
Error`) with backpressure, and prove streaming token metering: incremental
tally reconciled against each provider's terminal usage frame, with the
measured error bound written down. This is the highest-risk novel claim in the
whole design; if it doesn't hold, the architecture changes now, cheaply.

**Spike B: the foundation bake-off.** Build the same minimal streaming proxy
twice — once on Pingora (Cloudflare's Rust proxy library, designed exactly for
custom proxies with streaming body filters), once embedding/extending
agentgateway (Apache-2.0; the credential paths are known, CEL and
LLM-awareness already there, but the WASM and fleet layers would be ours on
top of someone else's governance). Two weeks, then commit. The control plane
is greenfield either way.

**Exit criteria:** event model validated on three providers with a published
metering error bound; foundation chosen with the trade-offs written into doc
06.

> **Phase 0 closed — 1 August 2026.** Spike A: canonical event model proven
> on three wire formats; metering error bound measured from 17 machine-
> recorded transcripts (see `spikes/event-model/README.md`) — chars/4 lands
> within ~±50%, worst misses structural (tool-call scaffolding, tiny
> responses). Spike B: Pingora arm positive (streaming tap without
> buffering, 1:1 chunk cadence, `spikes/proxy-pingora/`); agentgateway arm
> (`docs/spike-b/agentgateway-embed.md`) found the core unpublished and,
> decisively, no internal canonical event model to inherit. **Decision: the
> data plane is built on Pingora** (Rust 1.97 toolchain accepted as the
> cost); the agentgateway GB-4/GB-8 changes proceed as parallel upstream
> contributions, per the extend-don't-fork strategy. Fork rejected.

## Phase 1 — standalone data plane (Baseline-conformant from a file)

One binary, one static config file, no control plane. Immediately useful, and
it earns a tracker row.

1. Provider adapters for the three spiked providers, emitting the canonical
   event stream.
2. Policy chain with scoped inheritance (`fleet → project → route → app`,
   explicit base marker) — even though "fleet" is a single node for now, the
   composition model is day-one.
3. **GB-1**: required attribution keys as a route field; requests without the
   tag are rejected.
4. **GB-2/GB-3**: claim mappings from verified JWTs, operator-pinned values
   for everything else. Two origins for every tag: proven or assigned, never
   believed.
5. **GB-4**: operator-defined rejection bodies — including the streaming
   terminal event, the thing nothing in the matrix does.
6. **GB-7/GB-8**: invoice-grade attribution on AWS and Vertex, the
   agentgateway PR patterns (session tags, per-request credential values) as
   native features.
7. CEL everywhere a condition or derivation is needed. No WASM yet.

**Exit criteria:** the Baseline checks pass as automated conformance tests;
the tracker row is verified by the same public-documentation standard as every
other gateway.

## Phase 2 — control plane MVP (Git sync, snapshots, drift)

1. Config repo layout mirroring the policy scopes; the control plane compiles
   scoped chains + templates into per-data-plane rendered snapshots.
2. xDS-style long-lived gRPC to data planes: versioned snapshots, ACK/NACK,
   explicit partial-application policy (all-or-nothing waves first — per-node
   latching later).
3. Drift detection and self-heal; divergence surfaced, never silent.
4. Join-token bootstrap: a new data plane self-populates its full bundle.
5. Admission control: CEL validations on config PRs before render.

**Exit criteria:** a three-node heterogeneous fleet (one container, one VM,
one cluster pod) converges from Git; a bad PR is rejected at admission; a
killed node rejoins and self-heals.

## Phase 3 — the stateful layer (the hard half)

1. **GB-5**: every spender capped by default — budget shares allocated by the
   control plane from observed spend telemetry, continuous rebalancing,
   synchronous escalation above ~90% consumption, bounded-overspend semantics
   published.
2. **GB-6**: alert rules firing from the enforcement layer itself — someone is
   told when a cap is hit, natively, not via a metrics side-channel.
3. Mid-stream enforcement wired to the shares: budget exhausted mid-generation
   cuts the stream with the GB-4 terminal event.

**Exit criteria:** the 100k-token scenario is five lines of YAML; the
overspend bound under partition is measured and documented, not estimated.

## Phase 4 — WASM SDK and hot swap

1. WASM policy modules (signed, admission-checked), epoch-based preemption and
   per-event fuel budgets. Performance validation on hot streaming paths
   before the hooks are promised publicly.
2. **GB-9**: hot-swappable config with the full doc-03 semantics — atomic
   module binding per snapshot, drain for in-flight streams, versioned counter
   schemas with migration hooks for stateful modules.
3. Break-glass with TTL: visible, temporary, auto-reverting.

## Phase 5 — fleet ergonomics

1. **GatewaySets**: label selectors × generators stamping config across the
   fleet.
2. Projects/tenancy scoping.
3. Config canaries: wave rollouts by failure domain, analyzed on error rate,
   p99, and token-spend anomaly, auto-rollback. Manual judgment gates as
   Git-native mechanisms (approvals on the wave PR), not a pipeline engine.

## Named risks, carried openly

- **The name.** "The Gateway Project" collides hard with Kubernetes Gateway
  API mindshare — it will be misread in exactly the community we are
  recruiting from. Renaming is cheap until the repo is public; the decision is
  deferred but the deadline is Phase 1's tracker row.
- **WASM on the hot path.** Per-event hooks on streaming paths need real
  performance validation before we promise them. That is why they sit in
  Phase 4, behind a spike, not in the pitch.
- **The strongest competitor is "good enough."** Not any gateway —
  "agentgateway CRDs + ArgoCD is good enough." We win only with the
  domain-aware reconciler and the non-k8s fleet story; if a phase doesn't
  advance one of those two, it is scope creep.
