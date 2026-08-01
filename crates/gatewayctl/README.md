# gatewayctl

The control plane: GitOps for gateway fleets, made concrete. One binary that
compiles a config repo (**Git truth**) into per-node **rendered snapshots** and
distributes them to N `gatewayd` data planes over one long-lived bidirectional
gRPC stream each, detects drift and self-heals it, and gates config PRs at
admission. Phase 2, milestones 1 (fleet distribution) and 2 (Git truth, drift
self-heal, admission).

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
> canary is "waves with analysis between them". The ordered-wave substrate is
> built here; the analysis (metrics gate between waves) is Phase 5 and stays
> deferred.

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

- **Config canary ANALYSIS between waves** (docs/04 Phase 5). Multi-wave IS built
  — it is the ordered-wave substrate a canary sits on. What is deferred is the
  metric/analysis gate _between_ waves (advance only if wave _k_'s SLOs hold);
  today a wave advances on a full ACK, not on an analysis verdict.
- **Postgres.** The runtime store is in-memory; Postgres replaces it later and,
  per docs/07, is never truth (observed reality only, re-derivable from Git plus
  the stream).
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
runner, and wave state machine are modules in this one process, not services —
docs/07's budget kept honest. `gix` is the only new dependency family
(pure-Rust, local-read-only), and `cel` is already in the tree via gateway-core.
