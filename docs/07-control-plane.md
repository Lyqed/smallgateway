# The control plane: GitOps for gateway fleets, made concrete

*Design only. Phase 2 not started. This turns the sketch in
[02-architecture.md](02-architecture.md) (the control-plane section and the
GitOps capability table) into the decisions Phase 2 will be built against. It
extends the single-node snapshot semantics already shipped in
`crates/gatewayd/src/reload.rs` and `crates/gateway-core/src/snapshot.rs` to a
fleet. Where a choice is still open, it is named as an open question with a
stated bound, per the doc-03 discipline: declared edges, not magic.*

The data plane already reloads config from a file with versioned snapshots,
content hashes, ACK/NACK, and drain-on-old (doc 03). The control plane is the
same machine with the file replaced by Git and the single node replaced by a
fleet. Nothing in the reload path changes shape. It gains a second trigger (a
gRPC stream) alongside SIGHUP and the poll watcher, and the version counter
stops being process-local.

## What carries over, what is new

The single-node work is the load-bearing half. Stating the seam precisely
keeps Phase 2 from re-litigating it.

| Concern | Single node (built) | Fleet (Phase 2, new) |
|---|---|---|
| Rendered snapshot | `Snapshot { config, version, source, content_hash }`, one per process | Per-data-plane rendered snapshot, compiled centrally, addressed by node identity |
| Version | Process-local `AtomicU64`, monotonic, gaps-free | Fleet-global, derived from the source commit. A node's version is `(commit, node-render-hash)`, still monotonic per node |
| Hash | SHA-256 of source bytes, no-op check plus audit link | SHA-256 of the *rendered* bytes. A node no-ops when its new render hashes to its active render |
| Reload trigger | SIGHUP, poll watcher | Long-lived gRPC stream from the control plane, funneling into the same `Reloader::reload` |
| NACK | Loud reject, old snapshot keeps serving | Same, plus the NACK travels back over the stream and the node is marked divergent, never silent |
| Drain | `Arc<Snapshot>` refcount, two versions live | Unchanged. This is per-node and does not touch the wire |

The one-line summary: the node already knows how to accept, reject, and drain a
version. Phase 2 teaches the control plane how to *produce* per-node versions
from Git and *deliver* them, and teaches the node one more trigger.

## The snapshot distribution protocol

### What the control plane compiles

A rendered snapshot is the same object the node renders today, minus the file
read: a validated `Config`, addressed to one data plane, produced by composing
the four scoped chains (`fleet → project → route → app`, doc 02) and stamping
the result. The control plane does the compilation ArgoCD does with its
rendered-manifest pattern. Scoped chains plus values files in, one flat
per-node config out, no templating left for the gateway to do at runtime. The
gateway receives config it can validate and serve directly, exactly as if an
operator had written that flat file by hand.

Wire format of one delivered snapshot:

```
RenderedSnapshot {
  node_id:        string        // which data plane this is for
  source_commit:  string        // the config-repo commit it was rendered from
  render_hash:    string        // SHA-256 of the canonical serialized config below
  fleet_version:  uint64        // monotonic per node, assigned at delivery
  config:         bytes         // the canonically-serialized flat Config
  compiled_at:    int64         // unix seconds, control-plane clock
}
```

`config` is the same structure `Config::from_yaml` already parses and
validates. It is serialized canonically (stable key order, no incidental
whitespace) so that `render_hash` is reproducible: the same commit plus the
same node selectors always yields the same bytes and therefore the same hash.
That reproducibility is the six-month rule made mechanical (below), not a
protocol nicety.

`render_hash` is the fleet analog of the node's `content_hash`. On the node,
the incoming `config` bytes are hashed and compared to the active snapshot's
hash before anything else, reusing the existing no-op short-circuit verbatim.
A redeploy of the control plane, a stream reconnect, or a resend of the same
commit is a no-op on every node whose render is unchanged, for free.

### The stream

One long-lived bidirectional gRPC stream per data plane, xDS-shaped (doc 01,
Q5: "build on top, xDS-style versioned snapshots over gRPC; Envoy proved the
shape, we need the shape, not Envoy"). The control plane is the server; the
data plane dials out and holds the stream open. Dial-out, not dial-in, is the
heterogeneous-fleet requirement: a DMZ box or an edge node can reach the
control plane without the control plane needing a route back in.

Messages, data plane to control plane:

- `Hello { node_id, join_token | node_cert, labels, current_fleet_version }`,
  sent once on connect; `current_fleet_version` lets a reconnecting node skip a
  redelivery it already has.
- `Ack { fleet_version, render_hash }`: the node accepted and swapped to this
  version. `render_hash` echoes back so the control plane can detect a node
  that acked the wrong bytes (a bug or tampering), not just the wrong number.
- `Nack { fleet_version, render_hash, reason }`: the node rejected this
  version; the old one keeps serving. `reason` carries the same precise
  validation errors the node already logs on a file NACK.
- `Status { observed_render_hash, health, in_flight_streams }`: periodic
  heartbeat carrying what the node is *actually* running, the input to drift
  detection.

Messages, control plane to data plane:

- `Push { RenderedSnapshot }`: deliver a new version.
- `AckOfStatus {}`: liveness, nothing more.

The control plane never pushes imperative mutations over this stream. There is
no `Patch` message and there will not be one; that is limitation 1 of doc 03
enforced at the protocol level. The only way to change a running node is a new
rendered snapshot, and the only source of a rendered snapshot is a commit.

### ACK/NACK semantics, extended

The node's three outcomes (`Swapped`, `NoOp`, `Rejected`) already exist as
`ReloadOutcome` and map straight onto the wire:

- `Swapped { old, new }` becomes `Ack { fleet_version: new, render_hash }`.
- `NoOp { active }` becomes `Ack { fleet_version: active, render_hash }`. A
  no-op is an ack: the node confirms it is already at the pushed version. The
  control plane treats "acked the same version we asked for" identically
  whether the node swapped or no-opped.
- `Rejected { active }` becomes `Nack { fleet_version: pushed, render_hash,
  reason }`, and the node stays at `active`.

A node that never answers (dead stream, partition) is neither acked nor nacked.
It is *unknown*, a third state the single-node code never needed. Unknown is
surfaced with an age ("last acked v41, silent 90s") and is bounded by the
staleness rule below. Unknown is not treated as acked and not treated as
divergent-on-purpose; it is treated as unreachable, which is honest.

## Partial application: all-or-nothing waves, chosen

Doc 03, limitation 1, states the fork: "all-or-nothing waves, or per-node
latching with divergence surfaced, never silent." Phase 2 chooses
**all-or-nothing waves**, per the build plan ("all-or-nothing waves first,
per-node latching later").

A wave is a set of nodes grouped by failure domain (region, cluster, cloud,
from labels). Applying a commit walks the waves in order:

1. Push the new rendered snapshots to every node in the wave.
2. Wait for every node in the wave to `Ack` the exact `render_hash`, within a
   per-wave timeout.
3. If all ack: proceed to the next wave.
4. If any node `Nack`s or times out (goes unknown past the bound): **halt**.
   The wave does not "mostly succeed." Nodes already advanced in earlier waves
   stay advanced; the halting wave and all later waves stay on their prior
   version. The rollout stops with an explicit, named divergence: "commit
   `abc123` applied to waves 1 and 2, halted at wave 3 on node `edge-fra-2`
   (NACK: unknown provider `foo`)."

Why waves over latching, for the MVP:

- **Blast radius is the whole point of a fleet control plane.** A bad commit
  that would take down every node takes down one failure domain instead, and
  the halt stops it there. Per-node latching lets a bad commit dribble across
  the entire fleet one node at a time, each one individually "surfaced" but the
  aggregate still a fleet-wide outage.
- **It reuses the node code unchanged.** Each node still does exactly one
  swap-or-reject; the sequencing lives entirely in the control plane. Latching
  adds per-node desired-vs-latched bookkeeping that is real Phase-2 scope we
  don't need to prove the thesis.
- **The canary story (Phase 5) is waves with analysis between them.** Choosing
  waves now is choosing the substrate config canaries sit on later, not a
  detour.

Divergence is surfaced, never silent, in both the halted-rollout case and the
steady state. A halted rollout leaves the fleet in a *known, named, mixed*
state: the control plane records exactly which waves are on which commit, and
that mixed state is queryable and alertable. It is not "some nodes are on new,
some on old, shrug." It is "waves 1 and 2 on `abc123`, waves 3 and up on
`def456`, halted 11:04 on this node for this reason." The mixed state is
legitimate and temporary; the operator either fixes the config (new commit,
rollout resumes from the halt) or reverts (roll waves 1 and 2 back to
`def456`).

Per-node latching is deferred, not rejected forever. The build plan puts it
"later," and the honest reason is that some operations (a slow, deliberate
per-node migration) want it. It is an open question below, with the bound that
until it lands, the only application mode is waves.

## Truth in Git

### Repo layout mirrors the four scopes

The config repo directory structure is the policy scope hierarchy, one level
per directory, so a reviewer reads scope from the path (doc 01, Q1; doc 02,
"each level maps to a Git directory"):

```
fleet/
  base.chain.yaml          # fleet-level policy: STS credential chain, global defaults
  providers/               # provider refs available fleet-wide
projects/
  <project>/
    base.chain.yaml        # project-level: attribution enforcement, project defaults
    values.yaml            # project-scoped values (budgets, labels)
    routes/
      <route>/
        route.chain.yaml   # route-level: the route itself, per-route policy
        apps/
          <app>/
            values.yaml    # app-level: per-app overrides (a TPM cap, a pinned tag)
selectors/
  gatewaysets.yaml         # label selectors -> which nodes get which projects (Phase 5 stub)
```

Each `*.chain.yaml` composes with an explicit `base` marker (doc 02, the
APIM `<base/>` idea done granularly). The chain format is the small, total
policy DSL from doc 01, Q6: declarative, no unbounded loops, diffable in
review, compilable to a flat snapshot. A route-level chain prepends and appends
around `base`, and `base` is the project chain, whose `base` is the fleet
chain. The composition is the same one the standalone data plane already runs
in Phase 1 (`fleet → project → route → app`). Phase 2 only changes where the
four levels come from: four Git directories instead of four sections of one
file.

### Rendered-manifest compilation, reviewable diffs

The control plane does not template inside the gateway. It compiles: for each
node, it selects the projects/routes/apps that node's labels match, composes
the four chains into one flat `Config`, and serializes it canonically. That
flat rendered config is what ships in `RenderedSnapshot.config`.

The rendered config is itself committable and reviewable. A PR that changes
`fleet/base.chain.yaml` produces a rendered-config diff *per affected node*.
The reviewer sees not "the fleet base changed" but "on `edge-fra-2` this route
gained this header rewrite; on `vm-us-1` nothing changed because it doesn't
match that project." This is the ArgoCD rendered-manifest pattern's whole
value: the diff a human reviews is the diff the fleet will actually run, not a
template whose effect you have to simulate in your head. In-gateway templating
is banned precisely because it would move that diff out of review and into
runtime, where nobody sees it until it's live.

### Reproducible from a commit hash: the six-month rule, mechanical

Doc 01, Q8 asks: "how does a change stay owned for six months?" and answers
"mechanically, not aspirationally: every rendered snapshot is reproducible from
a commit hash." Here is the mechanism:

1. `RenderedSnapshot.source_commit` names the exact config-repo commit.
2. Compilation is a pure function of `(commit, node labels)` to rendered
   config. No wall-clock, no external lookups, no randomness in the render.
   Same inputs, same bytes, same `render_hash`, forever.
3. Therefore, to answer "what was `edge-fra-2` running at 03:14 on the night of
   the incident," you take the `source_commit` from that node's delivery record
   (Postgres, below), check it out, re-run compilation for that node's labels,
   and get back the identical bytes, verified by `render_hash` matching.

"What changed and who changed it" is `git log` on the config repo. "What was
actually running" is a re-render of a recorded commit. Neither is an
investigation. Break-glass overrides (doc 00, doc 01) are themselves commits
with a TTL, so they show up in exactly the same history and re-render the same
way; a 3am override is not a hole in the record, it is a short-lived commit.

## Join-token bootstrap

A new data plane self-populates its full bundle from a join token plus a Git
path, the "app-of-apps bootstrap" row of the doc-02 table, the `argocd
cluster add` ergonomic.

The flow:

1. An operator mints a join token in the control plane: a single-use,
   short-TTL secret bound to a set of labels (`region=fra, env=edge,
   cloud=bare-metal`). The token authorizes *joining as a node with these
   labels*, nothing more.
2. The new gateway starts with only the token and the control-plane address, no
   config file. It dials the stream and sends `Hello { join_token, labels }`.
3. The control plane validates the token (unused, unexpired, labels match),
   then issues the node a longer-lived node identity (a per-node certificate or
   equivalent) and immediately compiles-and-pushes the node's first
   `RenderedSnapshot` from current Git for its labels.
4. The node validates and swaps to it (the same `Reloader::reload` path, first
   version instead of a reload) and `Ack`s. It is now a full member of the
   fleet, running Git truth, with zero config authored by hand on the box.
5. The join token is burned. Reconnects thereafter use the node identity, not
   the token.

The token authenticates the *join*; the node cert authenticates every
subsequent stream. A stolen join token is bounded by single-use and short TTL;
a stolen node cert is revocable centrally (the control plane stops pushing and
drops the stream). Neither secret is config truth, so neither leaks the config
repo: a compromised node receives only its own rendered snapshot, never the
repo or other nodes' configs.

## Drift detection and self-heal

The reconciler is the control plane's loop, and it compares exactly three
things per node, all of which already exist as concepts:

- **Desired**: the `render_hash` the control plane computes for that node from
  the current applied commit (respecting wave state; a node in a
  not-yet-applied wave's desired hash is its *prior* commit's hash, not the
  newest).
- **Delivered**: the `render_hash` the control plane last pushed to that node.
- **Observed**: the `render_hash` the node reports in `Status`, what it is
  *actually* running.

Drift is any mismatch among the three, and each mismatch has one convergence
action:

| Desired | Delivered | Observed | Meaning | Action |
|---|---|---|---|---|
| = | = | = | Converged | none |
| ≠ | = | = | New commit, not yet rolled to this wave | push in wave order |
| = | = | ≠ | Node drifted (restart on stale local file, break-glass, tampering) | re-push desired; node swaps back |
| = | ≠ | any | Delivery lost (reconnect, dropped push) | re-push |
| = | = | unknown | Node silent | wait to staleness bound, then alert |

Self-heal is re-push. The node cannot be quietly wrong: its `Status` carries
the hash of what it runs, so a node running anything other than its desired
render is caught on the next heartbeat and pushed back. A node that
persistently NACKs the desired render (its local environment genuinely can't
serve it) is *deliberately divergent* and stays surfaced. The reconciler does
not hide a NACK by retrying it into oblivion; it reports "node X has NACKed
desired v52 four times, reason Y" and leaves it visibly divergent for a human.

At fleet scale this is O(nodes) hash comparisons per reconcile tick, no config
re-render unless the commit changed, and no per-request control-plane
involvement at all. The data plane serves entirely from its local snapshot
whether or not the control plane is up. Control-plane downtime freezes the
fleet at its last-acked versions; it does not stop traffic. That is the
ArgoCD property (the reconciler is not in the request path) and it is why the
control plane is not a SPOF for serving, only for *changing*.

## Postgres for runtime state only, never truth

The control plane's Postgres holds observed reality and nothing desired (doc
00, doc 01 Q1). Concretely, the tables are:

- **Snapshot delivery status**, per node: last delivered `fleet_version`,
  `render_hash`, `source_commit`, delivery timestamp, ack/nack/unknown state.
  This is the record you re-render from to answer "what was running when."
- **ACK/NACK log**: every `Ack`/`Nack` with its `render_hash`, reason, and
  time. The wave-rollout state machine reads this to decide halt-or-proceed.
- **Observed health**: the `Status` heartbeats, carrying node liveness,
  `observed_render_hash`, in-flight stream counts, last-seen time.
- **Node registry**: issued node identities, labels, join-token audit (minted,
  used, expired).

Why none of it is authoritative: every row is *derivable or re-derivable from
Git plus the stream*, and losing it costs history, not truth. Wipe Postgres and
the fleet keeps serving (nodes run local snapshots); the reconciler rebuilds
delivery status from the next round of `Status` heartbeats, and the desired
state was never here to lose. It is the config repo. Postgres records what
happened; Git decides what should happen. The moment any desired-state field
("this node *should* run version N") is written to Postgres instead of derived
from a commit, truth has forked and the doc-01 Q1 invariant is broken. The
schema is designed so there is no column to write it to.

Delivery status stores `source_commit`, not desired state, and the distinction
is load-bearing. `source_commit` is a *record of what was pushed* (history);
desired state is *what should be pushed now* (truth), and that is recomputed
from the current applied commit every tick, never read from the table.

## The two-binaries budget

The control plane stays one binary (doc 00, principle 1: "one control plane, a
single binary with Postgres for runtime state"). Reconciler, gRPC server,
compiler, admission-check runner, and wave state machine are modules in one
process, not services. Postgres is a datastore the one binary talks to, not a
second component in the budget, the same way ArgoCD counts as its handful of
components, not "ArgoCD plus etcd."

What would violate the budget, named so it can be refused in review:

- **A separate compiler/renderer service.** Compilation is a pure function; it
  is a function call, not a microservice. Splitting it out is the first step
  toward Spinnaker's Front50.
- **A message broker between control plane and data planes.** The long-lived
  gRPC stream *is* the transport. A Kafka/NATS hop adds a component, a SPOF, and
  a place for truth to accumulate.
- **A dedicated metrics/analysis service for canaries.** Config-canary analysis
  (Phase 5) reads the gateway's own telemetry; it is a module, per doc 00's
  "Kayenta-style analysis as a Git-native mechanism, not a pipeline engine."
- **A second control-plane database for desired state.** There is exactly one
  source of desired state (Git) and one runtime-state store (Postgres). A third
  store is either redundant or a truth fork.
- **Any control-plane component in the request path.** The data plane must
  serve with the control plane down. Anything that breaks that turns two
  loosely-coupled binaries into one distributed monolith.

Regional control planes (doc 02's "hierarchical control planes, v2")
are *the same one binary* run in a hierarchy, a regional instance consuming a
root's rendered fleet config, not a new component type. That is scaling the
one binary, not adding a second.

## Open questions and staleness bounds

Stated plainly, as declared edges (doc 00, principle 3; doc 03's closing
discipline).

- **The staleness window is not zero, and Phase 2 must publish its bound.**
  Between a merge to the config repo and the last node acking the resulting
  rendered snapshot, Git says X and some nodes run X-1 (doc 03, limitation 1).
  The bound is (repo-poll or webhook latency) + (compile time) + (per-wave push
  plus ack time) x (number of waves). Waves *widen* this window deliberately,
  in exchange for blast-radius control. The number is measured in Phase 2, not
  guessed, and published like the metering error bound was in Phase 0.

- **Unknown-node timeout.** How long a silent node stays "unknown" before it is
  alerted, and whether an unknown node blocks a wave indefinitely or the wave
  proceeds-with-exclusion after a bound. The MVP choice: unknown past the
  timeout *halts the wave* (conservative, since a node we can't confirm might be
  serving anything). Whether that is too strict for large fleets with routine
  churn is open; the bound until it's revisited is "unknown halts."

- **Per-node latching.** Deferred (above). Open: the exact desired-vs-latched
  data model, and which operations (deliberate per-node migration) justify it.
  Until it lands, waves are the only mode, stated, not implied.

- **Config-repo webhook vs poll.** Whether the control plane learns of a new
  commit by webhook (fast, needs an inbound path) or poll (slow, works
  anywhere). Likely both, poll as the floor. This directly sets the first term
  of the staleness bound.

- **Render-hash canonicalization is a compatibility surface.** Once nodes
  compare an incoming render hash to their active one, the canonical
  serialization is frozen: change it and every node sees a spurious diff and
  re-swaps identical config fleet-wide. It joins the snapshot wire format on
  doc 00's list of irreversible-once-public decisions and stays provisional
  until Phase 2 code forces it.

- **Stateful-module drift is out of scope here.** This document converges
  *config*. Migrating live counters (budgets, quota shares) across a swap is
  doc 03, limitation 3, and lands in Phase 3/4 with budget shares. The
  reconciler above converges rendered config, not counter state, and does not
  pretend to.

## Cross-references

- [00-principles.md](00-principles.md): two binaries plus Git; the six-month
  ownership rule; stated invariants over magic.
- [01-design-questions.md](01-design-questions.md): Q1 (truth in Git), Q4
  (budget shares), Q5 (xDS-style transport, greenfield control plane), Q7
  (bounded staleness), Q8 (six-month rule mechanical).
- [02-architecture.md](02-architecture.md): the control-plane section and the
  GitOps capability table this document makes concrete.
- [03-hot-swap.md](03-hot-swap.md): the three limitations; limitation 1 is the
  partial-application decision made here.
- [04-build-plan.md](04-build-plan.md): Phase 2 scope and exit criteria (a
  three-node heterogeneous fleet converges from Git; a bad PR is rejected at
  admission; a killed node rejoins and self-heals).
- `crates/gatewayd/src/reload.rs`, `crates/gateway-core/src/snapshot.rs`: the
  single-node snapshot/version/hash/ACK-NACK/drain machinery this extends.
