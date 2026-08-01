# gatewayctl

The control plane: GitOps for gateway fleets, made concrete. One binary that
compiles a config repo (**Git truth**) into per-node **rendered snapshots** and
distributes them to N `gatewayd` data planes over one long-lived bidirectional
gRPC stream each, detects drift and self-heals it, gates config PRs at
admission, and — Phase 5 — runs **config-canary analysis between waves with
auto-rollback** and a **Git-native manual judgment gate**. Phase 2 (fleet
distribution, Git truth, drift self-heal, admission, multi-wave + GatewaySets),
Phase 3 (GB-5 budget shares), and Phase 5 (config canaries) land here; Phase 4
(WASM hooks) lands in `gatewayd`/`gateway-wasm`.

Built against the binding design in
[docs/07-control-plane.md](../../docs/07-control-plane.md); it extends the
single-node snapshot semantics already shipped in
[`gateway-core/src/snapshot.rs`](../gateway-core/src/snapshot.rs) and
[`gatewayd/src/reload.rs`](../gatewayd/src/reload.rs) to a fleet. Nothing in the
node's reload path changes shape: the control plane teaches the node one more
trigger (a `Push` over the stream) and moves the version counter off-process.

Run it:

```
# serve mode: source config from a directory OR a Git ref/commit
gatewayctl --repo <config-repo-dir> [--listen 127.0.0.1:6187] \
           [--join-token <secret>] [--token-ttl 300] [--poll-interval 3] \
           [--reconcile-interval 5] [--push-raw <snapshot-file>] \
           [--break-glass-file <file>] \
           [--label-token <region=eu,...>:<secret>]...  # labeled join tokens

gatewayctl --git-repo <repo-path> --git-ref <HEAD|branch|tag|sha> \
           [--listen 127.0.0.1:6187] [--reconcile-interval 5] …

# admission gate (CI): exit non-zero if the candidate config PR is blocked
gatewayctl admit --repo <config-repo-dir>
gatewayctl admit --git-repo <repo-path> --git-ref <ref>
```

Proof lives in two demos:

- [`scripts/fleet-demo.sh`](scripts/fleet-demo.sh) →
  [`fleet-demo.log`](fleet-demo.log) (milestone 1): one control plane, two data
  planes joining and receiving v1, a config change rolling out v2 with ACKs, a
  deliberately-invalid snapshot both nodes NACK while the fleet stays committed,
  and a control-plane outage the nodes survive and auto-reconnect from.
- [`scripts/git-drift-demo.sh`](scripts/git-drift-demo.sh) →
  [`git-drift-demo.log`](git-drift-demo.log) (milestone 2): config sourced from
  a **real Git commit** (the snapshot carries the SHA), a node **drifting** and
  the reconciler **healing it back to desired within one tick** (the
  desired/delivered/observed three-hash compare printed), **break-glass**
  tolerated for its TTL then healed after it lapses, and an **admission failure
  blocking a bad config** with the failing rule named.
- [`scripts/multiwave-demo.sh`](scripts/multiwave-demo.sh) →
  [`multiwave-demo.log`](multiwave-demo.log) (milestone 3): three **region-
  labeled** nodes and a `waves.yaml` ordering canary → eu → us. An ordered
  **multi-wave rollout** (each wave fully acked before the next), a **GatewaySet**
  stamping `tier: gold` onto eu so eu renders a **distinct hash** from one
  selector, a **newly-joined** eu node picking up the stamp on its first render,
  and a **silent wave-2 node HALTING wave 2 and FREEZING wave 3** while wave 1
  stays advanced — the mixed per-wave committed state surfaced, never "shrug".
- [`examples/canary.rs`](examples/canary.rs) →
  [`canary-demo.log`](canary-demo.log) (**Phase 5**): three region-labeled nodes
  over real gRPC and three scenarios — a **healthy canary passing analysis** and
  the rollout advancing; a **token-spend anomaly** on the canary wave (8x the
  baseline per node, infra metrics fine) **auto-rolling-back** the wave and
  **freezing the later waves**, with the tripping metric + wave + reverted-to
  version surfaced; and a rollout **paused at a Git-native judgment gate** until
  the approval artifact (`approvals/canary.approved`) is committed, then
  proceeding. Analysis runs from the fleet's OWN telemetry — no new service.

## What it does

### Config source: directory or Git ([`source.rs`](src/source.rs))

The desired config is read through one `ConfigSource` trait with two variants:

- **`DirectorySource`** — a loose directory on disk (the milestone-1 path). Its
  `source_commit` is a content-derived id (`dir-<hash>`), stable for identical
  content.
- **`GitSource`** — reads the four-scope repo at a specific **ref or commit**
  (`HEAD`, a branch, a tag, a full/short SHA) out of a real Git repository using
  the pure-Rust [`gix`](https://crates.io/crates/gix) crate (no C toolchain, no
  network features compiled in — just the local-repo read path). Its
  `source_commit` is the resolved **40-hex commit SHA**, so a `RenderedSnapshot`
  records the exact commit it was rendered from.

Both variants resolve to the same source-agnostic `ResolvedRepo` (a
deterministically-sorted `(path, bytes)` set), so the directory and Git paths
feed byte-identical inputs into byte-identical assembly — the same content
renders to the same `render_hash` either way, and a historical commit re-renders
its own bytes without a checkout.

### Rendered-manifest compilation ([`render.rs`](src/render.rs))

Assembles the four-scope fragments into one flat `Config` and validates it by
reusing `gateway-core`'s scope composition + validation verbatim. Rendering is a
**pure function** of the resolved bytes: fixed read order, canonical (sorted-key)
serialization, SHA-256 of the *rendered* bytes as `render_hash`. Same commit →
same bytes → same hash, forever — **the six-month rule made mechanical**: a
control-plane restart re-derives the identical snapshot from the same commit, and
"what was `edge-fra-2` running at 03:14" is a pure re-render of a recorded
commit. An invalid repo is rejected at render time, before any node sees it.

Repo layout (mirrors the four scopes, docs/07):

```text
<repo>/
  providers.yaml            # the providers: map (fleet-wide refs)
  rejections.yaml           # the mandatory GB-4 rejections: block
  auth.yaml                 # optional auth: block
  fleet/base.chain.yaml     # the fleet-scope attribution:/labels:
  projects/<p>/base.chain.yaml
  routes/<name>.route.yaml  # one routes: entry per file
  apps.yaml                 # optional apps: block
```

### Config-PR admission ([`admission.rs`](src/admission.rs))

Admission runs against a **candidate** config (a directory or a Git ref/commit)
**before it can become desired**, with a precise per-rule error naming exactly
what is wrong. Two rule families:

- **Built-in Baseline gates**: **GB-1** (every route enforces at least one
  effective attribution key), **GB-4** (both mandatory rejection templates
  present and non-empty), a **forbidden-construct** gate (an in-gateway-templating
  `{{ … }}` directive outside the two allowed rejection placeholders is banned so
  the reviewed diff is the served diff — docs/07), and an **override-governance**
  gate (an app override that raises a pinned numeric cap beyond a configured
  factor must carry an `override-approved` label).
- **CEL-expressed rules**: operator-authored predicates over the candidate
  document (`{ id, expr, message }`), evaluated with the same sandboxed `cel`
  interpreter gateway-core compiles route conditions with. Must return `true` to
  admit; a broken rule blocks rather than silently passing.

Exposed two ways: the **`admit` subcommand** exits non-zero on a block so CI can
gate a PR, and the pre-rollout gate runs admission **automatically before any
rollout** (startup, SIGHUP, or poll) — a blocked candidate never becomes desired.

### The gRPC stream ([`server.rs`](src/server.rs))

The `FleetService.Session` bidi stream (wire types in
[`gateway-proto`](../gateway-proto)): the control plane is the server, the data
plane dials out. It authenticates each `Hello` via a **join token**, pushes the
joining node its **own per-node desired render** immediately (GatewaySet-stamped
when its labels match — see below), and — on a reload — walks the **ordered wave
plan** ([`rollout.rs`](src/rollout.rs)), surfacing each node's Ack/Nack. It also
**self-heals one drifted node** (`heal_node`) by re-pushing desired to just that
node. The control plane never pushes imperative mutations; there is no `Patch`
message and there will not be one (docs/07 at the protocol level). The session
table and push/ack correlation live in [`server.rs`](src/server.rs); the wave
walk and self-heal live in [`rollout.rs`](src/rollout.rs).

### Multi-wave rollout grouped by failure domain ([`waves.rs`](src/waves.rs) + [`rollout.rs`](src/rollout.rs))

A `waves.yaml` in the config repo defines an **ordered** list of waves, each a
label selector over node labels (region/cluster/cloud). A node belongs to the
**first** wave whose selector it matches; a node matching none is its own
implicit final wave. Selectors are simple label equality / set-membership, not an
expression language:

```yaml
# waves.yaml — rollout order canary -> eu -> us
waves:
  - name: canary
    selector: { region: canary }        # equality
  - name: eu
    selector: { region: [eu-west, eu-central] }  # set membership
  - name: us
    selector: { region: us }
```

Applying a commit walks the waves **in order** (`roll_out_plan`): push the new
render to every node in wave _k_, wait for **every** node to Ack the exact
`render_hash` within the per-wave timeout, and only then proceed to wave _k+1_.
On any Nack or timeout the wave **halts**: wave _k_ **and all later waves** stay
on their prior committed version (frozen, never pushed), while earlier waves that
already acked **stay advanced**. The per-wave committed state is recorded and
surfaced — "waves [canary, eu] advanced to `abc123`; halted at [us]; later waves
frozen on prior commit" — never "some on new, some on old, shrug" (docs/07). A
repo with no `waves.yaml` is the **degenerate one-wave case** (one everything-
wave over the whole fleet) — byte-for-byte the milestone-1/2 single-wave
behavior, and its tests still pass unchanged.

The reconciler-vs-wave race fix extends to multi-wave: the wave-in-flight guard
covers a node in **any** active wave during a rollout, and after a halt a node in
a not-yet-applied or frozen later wave is **pending, not drifted**
(`node_pending_in_unapplied_wave`) — the reconciler leaves it on its prior
version rather than dragging it forward past its wave's turn (proven in
[`tests/reconcile.rs`](tests/reconcile.rs)).

> Multi-wave is the substrate config **canaries** sit on (docs/04 Phase 5): a
> canary is "waves with analysis between them". Both the substrate AND the
> analysis are built — see **Config canaries** below.

### Config canaries: analysis between waves + auto-rollback ([`canary.rs`](src/canary.rs) + [`canary_rollout.rs`](src/canary_rollout.rs) + [`telemetry.rs`](src/telemetry.rs))

Phase 5 (docs/04; docs/07 "the canary story is waves with analysis between
them"; docs/00 "Kayenta-style analysis + manual judgment gates as **Git-native
mechanisms, not a pipeline engine**"). The canary rollout
(`roll_out_plan_canary`) is the analysis-gated **superset** of the plain
multi-wave walk: after a wave acks and **before** advancing to the next, it opens
an **analysis window** over the fleet's OWN telemetry and compares the canary
wave against a **baseline** (the not-yet-rolled later waves, still on the old
version). On a PASS it advances; on a FAIL it **auto-rolls-back**. When the
canary policy is disabled the walk degenerates to exactly the Phase-2 behavior.

**The three signals**, plain Rust over telemetry the fleet already ingests — no
metrics service, no new dependency (docs/07 anti-goal):

- **Error rate** — `errors / requests`, from the `Status` heartbeat's tallies and
  NACKs the stream already folds in. Fails when the canary's rate exceeds
  baseline by more than `max_error_rate_increase` (an absolute delta).
- **Latency p99** — a plain sorted-sample percentile of the observed latencies.
  Fails when the canary p99 exceeds baseline by more than `max_p99_factor`.
- **Token-spend anomaly** — the **domain-aware** signal nothing else has: the
  canary wave's per-node spend against the baseline's, read from the **budget
  ledger** (`UsageReport` telemetry, [`budget.rs`](src/budget.rs)). A config
  change that suddenly makes a wave spend far more — a bad route, a retry loop,
  the wrong (pricier) model — trips when the spend factor exceeds
  `max_spend_factor` **or** (with enough baseline spread) crosses `spend_zscore`.

The policy lives in the Git config repo (`canary.yaml`), **admission-checked like
any config** — a nonsensical threshold is blocked at admission, not discovered
mid-rollout:

```yaml
# canary.yaml — analysis between waves + a manual gate after the canary wave
enabled: true
window_secs: 60
max_error_rate_increase: 0.05   # +5 points over baseline
max_p99_factor: 1.5             # 1.5x the baseline p99
max_spend_factor: 2.0           # 2x the baseline per-node spend
spend_zscore: 3.0
metrics: { error_rate: true, p99: true, spend_anomaly: true }
manual_gate_after: [canary]     # the Git-native judgment gate (below)
```

**Auto-rollback** reuses the existing halt/freeze + revert machinery: on a FAIL
the failing wave is **reverted** (its prior render re-pushed so the nodes return
to the old config) and **all later waves are frozen**; the fleet's committed
version does **not** advance. The tripping metric, the wave, and the reverted-to
version are surfaced loudly — "ROLLED BACK at wave `canary` on token-spend
anomaly … reverted to `def456`; later waves frozen". Earlier already-analyzed-
healthy waves stay advanced; a canary that never fully committed rolls back
cleanly (docs/04). An **inconclusive** window (no telemetry) is **fail-closed** —
the rollout does not advance past a canary it could not measure.

### Git-native manual judgment gate ([`canary_rollout.rs`](src/canary_rollout.rs))

The second Spinnaker idea, done Git-native (docs/00; docs/04 "approvals on the
wave PR"). A policy marks a wave boundary as requiring **manual approval**
(`manual_gate_after`). The gate is satisfied by an **artifact in the config
repo** — `approvals/<wave>.approved` — **not** a pipeline "click to approve"
engine and **not** a running-state stages machine. The mechanism, stated plainly:
to approve a held wave, the operator adds `approvals/<wave>.approved` to the
config repo and **commits it** (the reviewed, audited, revertable wave-PR
approval), the same way every other desired-state change is expressed. The
control plane pauses at the gated wave and **polls the config source**
(`RepoGateSignal`, poll is the floor — docs/07) until the artifact appears, then
proceeds. There is no approval database, no click endpoint, no stages engine —
the approval lives in Git like all other truth.

### GatewaySets ([`gatewayset.rs`](src/gatewayset.rs))

A **GatewaySet** is a label selector plus a config overlay that stamps config
across every node matching the selector, so an operator writes **one** GatewaySet
instead of per-node files. It lives in the config repo (`gatewaysets.yaml`) and
composes as the **outermost** overlay onto the assembled scoped chain at **render
time**, then validates through the same `Config::from_yaml` gate every render
passes:

```yaml
# gatewaysets.yaml — stamp tier: gold onto every eu node
gatewaysets:
  - name: eu-gold-tier
    selector: { region: eu }
    overlay:
      fleet:
        attribution:
          pinned: { tier: gold }
```

The overlay **deep-merges** (a GatewaySet wins on the keys it names, leaves
siblings untouched). The render stays a pure function of `(repo bytes, node
labels)`: same repo + same labels ⇒ same `render_hash`, forever — **no live
templating in the data plane** (the reviewed diff is the served diff, docs/07).
Adding or removing a node with matching labels picks up / drops the stamp on the
next render with no per-node file edited. A GatewaySet is **admission-checked like
any config**: `admit_source` renders and admits each GatewaySet's stamped variant
(under a representative matching label set), so an overlay that breaks a Baseline
guarantee is caught at admission — attributed to the GatewaySet by name — never
discovered only when a matching node NACKs.

Nodes carry their labels from the **join token**; mint labeled tokens with
`--label-token <k=v,...>:<secret>` (repeatable) so a joining node lands in the
right wave and picks up the right GatewaySets.

### All-or-nothing waves ([`fleet.rs`](src/fleet.rs))

`Fleet` owns the applied render, the per-node monotonic versions, the per-wave
committed map, and the adjudication of **one** wave's results. On **any** Nack (or
a silent node past the timeout) the wave **halts**, the divergence is logged
loudly and left surfaced (never silent), and the fleet's committed version does
**not** advance. A fully-acked wave commits and advances it. The fleet also
records each node's **delivered** `render_hash` (the last hash pushed) — the
middle column of the drift truth table.

### Drift detection and self-heal ([`reconcile.rs`](src/reconcile.rs))

A periodic reconcile tick (`--reconcile-interval`, default 5s) compares three
hashes per node — **desired** (what current Git renders), **delivered** (what the
CP last pushed), **observed** (the `Status` heartbeat's `observed_render_hash`,
already in the proto) — and drives the docs/07 truth table row-for-row:

| desired | delivered | observed | case | action |
|---|---|---|---|---|
| = | = | = | `InSync` | none |
| ≠ | = | = | `DeliveryStale` (new commit not yet delivered / lost push) | re-push desired |
| = | = | ≠ | `NodeDrifted` (restart on stale file, break-glass, tamper) | re-push desired; node swaps back |
| = | = | *unset* | `ObservedUnknown` (no heartbeat yet) | wait |

Self-heal is a re-push. A node that **persistently NACKs** desired past a
threshold is declared `PersistentlyDivergent` — surfaced loudly and left visibly
divergent for a human, never retried into oblivion (docs/07). The classification
is a **pure function** (`classify`) of the three hashes plus break-glass state,
so every truth-table row is unit-tested without a network; the end-to-end
drift→heal is proven over real gRPC in [`tests/reconcile.rs`](tests/reconcile.rs).

### GB-5 budget-share allocation ([`budget.rs`](src/budget.rs))

The control-plane half of GB-5 (docs/01 Q4; docs/02 "GB-5 at fleet scale —
budget shares"; docs/04 Phase 3). The **data plane** owns the local counters and
the enforcement decision (`gatewayd`); the control plane owns the fleet-wide
half. Over the **same FleetService stream**, a node reports its observed
per-spender spend (`UsageReport`); the control plane folds it into an in-memory
fleet ledger, **rebalances each node's share** of every capped value, and grants
the shares back (`ShareGrant`). The allocation is a pure function of the
telemetry: a node keeps its own consumed portion plus a slice of the remaining
fleet headroom **weighted by its share of observed spend** — so a **hot node gets
a bigger slice** — with a cold-node floor so a fresh node still gets a starting
slice, and the sum of shares never exceeds the cap (headroom divided, not
invented). A node near the limit of its share sends a **`SyncCheck`** (the ~90%
synchronous escalation) and gets a fresh grant back immediately.

GB-6 alerts also fire from **this** enforcement point: a fleet-wide spend
crossing 80% (soft) or the cap (hard) — a crossing no single node reached alone —
is raised at ingest, carrying the value, cap, spend, and node/fleet context, into
a pluggable alert sink. The allocation math is unit-tested in
[`src/budget.rs`](src/budget.rs); the wire path (UsageReport → rebalance →
ShareGrant, SyncCheck → regrant, fleet-wide alert) end-to-end over real gRPC in
[`tests/budget.rs`](tests/budget.rs). The **bounded-overspend under partition**
is a data-plane property (the node spends only up to its held share when the
control plane is unreachable) — MEASURED in `gatewayd`'s
`scripts/budget-demo.sh`.

Runtime spend state is in-memory and **never truth**: wipe it and the next round
of usage telemetry rebuilds the shares from the caps (which come from Git).

### Break-glass with TTL ([`store.rs`](src/store.rs) + `--break-glass-file`)

A node may be marked **break-glass** for a bounded window (`--break-glass-file`
arms a SIGUSR2 handler; each line is `node_id [ttl_secs]`). While the window is
open the reconciler **tolerates** the node's drift and does not fight it, logging
the override and its expiry; when the TTL lapses the reconciler resumes and heals
the node back to desired (docs/00 break-glass with TTL). Break-glass is checked
first in `classify`, so it suppresses even a persistent-NACK surfacing for its
duration.

### Node auto-reconnect + hash self-verification ([`gatewayd/src/client.rs`](../gatewayd/src/client.rs))

The data-plane client supervises its stream: on a stream drop or a control-plane
outage it keeps serving its last bound snapshot (the control plane is not a SPOF
for serving) and re-dials with backoff, rejoining on its established identity. On
every push the node ACKs with the SHA-256 it **recomputes** of the bytes it bound
— not the advertised `render_hash` — so an inconsistent advertisement (bug or
tampering) is caught at the wave as a `WrongHash` divergence.

### Join-token bootstrap + identity-scoped reconnect ([`token.rs`](src/token.rs))

Single-use, short-TTL tokens bound to labels. A bad token refuses the stream; a
failed check never burns a live token. The first join burns the token and binds
it to the joining `node_id` (the M1 stand-in for a per-node cert); a reconnect is
that same node re-presenting its burned token, and a different node replaying it
is refused.

### In-memory runtime state ([`store.rs`](src/store.rs))

Connected nodes, acked versions, last NACK, consecutive-NACK count, observed
hash, health, and break-glass windows. **Postgres replaces this later and is
never truth** — every field is observed reality, re-derivable from Git plus the
stream. There is deliberately no field for desired state; that is recomputed from
the applied render every tick, never stored.

## The `--push-raw` break-glass / drift affordance

`--push-raw <file>` arms a SIGUSR1 handler that distributes that file's bytes as
a raw snapshot **bypassing the render gate**. Two uses:

- an **invalid** snapshot exercises the node's independent NACK defense (the M1
  demo: both nodes NACK, the wave halts, the fleet stays committed);
- a **valid-but-not-desired** snapshot makes a node **drift** out of band for the
  milestone-2 demo — the node binds it, diverges from the Git desired, and the
  reconciler heals it back.

## Deferred beyond this milestone (stated, not implied)

Per docs/07's open questions and the task's milestone scope:

- **Projects / tenancy scoping** (docs/04 Phase 5, item 2). **NOT built — the
  honest remaining Phase-5 item.** The four-scope repo layout (`projects/<p>/…`)
  and per-project base chains exist in render, but project-level **tenancy
  scoping** — isolating which projects a config PR, a rollout, or a set of nodes
  belongs to, and gating on it — is not implemented. GatewaySets + labels cover
  fleet-wide stamping and wave targeting today; per-tenant scoping/RBAC is the
  deferred piece.
- **Node-emitted canary metrics on the wire.** The canary analysis reads the
  fleet's own telemetry, and the wire (`Status.health`, `UsageReport`) is frozen
  and stable. Today the node hard-codes `health: "ok"`; encoding real per-window
  request/error/p99 tallies into the free-form `health` string (which the sink
  already parses — [`telemetry.rs`](src/telemetry.rs)) is a `gatewayd`-side
  follow-up. Token-spend telemetry IS live end-to-end (the GB-5 `UsageReport`
  path), so the domain-aware spend-anomaly signal is fully wired; error-rate/p99
  lean on NACK signals + the health string until the node emits the tallies.
- **Postgres.** The runtime store — and the GB-5 fleet budget ledger — are
  in-memory; Postgres replaces them later and, per docs/07, is never truth
  (observed reality only, re-derivable from Git plus the stream). **GB-5
  durable spend counters** are the specific in-memory state deferred here.
- **Counter-schema migration across hot-swaps** (docs/03 limitation 3). GB-5
  budget counters carry forward across a config swap as-is (a swap changes the
  *cap* a request reads, not the running counter); **versioned counter schemas
  with migration hooks** for a *changed* stateful-module schema stay deferred to
  Phase 4.
- **Richer GB-6 alert sinks.** The milestone ships a structured log sink plus a
  webhook-shaped JSON body; a real webhook POST, a pager, or a bus is deferred
  behind that seam.
- **Per-node latching** (docs/03 limitation 1's other fork). Waves are the chosen
  application mode; per-node desired-vs-latched bookkeeping is deferred, not
  rejected forever — some slow deliberate per-node migrations want it.
- **Config-repo webhook.** The change trigger is a poll (the floor, docs/07); a
  webhook (fast, needs an inbound path) is the later optimization.
- **Per-node certificates.** A burned join token bound to its node_id stands in
  for the per-node cert; issuing a separately-revocable node certificate is
  deferred.

## The two-binaries budget

`gatewayctl` and `gatewayd` are the two binaries;
[`gateway-proto`](../gateway-proto) and [`gateway-core`](../gateway-core) are the
libraries they share. The reconciler, gRPC server, compiler, admission-check
runner, wave state machine, **and the config-canary analysis + judgment gate**
are modules in this one process, not services — docs/07's budget kept honest, and
the canary anti-goal ("a dedicated metrics/analysis service for canaries") held:
the analysis is plain Rust statistics ([`canary.rs`](src/canary.rs)) over
telemetry the fleet already collects, and the gate is a Git artifact, not a
pipeline engine. `gix` is the only new dependency family (pure-Rust,
local-read-only), and `cel` is already in the tree via gateway-core; **Phase 5
adds no new dependency.**
