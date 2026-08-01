# gatewayctl

The control plane: ArgoCD for gateway fleets, made concrete. One binary that
compiles a config repo (Git truth) into per-node **rendered snapshots** and
distributes them to N `gatewayd` data planes over one long-lived bidirectional
gRPC stream each. Phase 2, milestone 1 — fleet distribution.

Built against the binding design in
[docs/07-control-plane.md](../../docs/07-control-plane.md); it extends the
single-node snapshot semantics already shipped in
[`gateway-core/src/snapshot.rs`](../gateway-core/src/snapshot.rs) and
[`gatewayd/src/reload.rs`](../gatewayd/src/reload.rs) to a fleet. Nothing in the
node's reload path changes shape: the control plane teaches the node one more
trigger (a `Push` over the stream) and moves the version counter off-process.

Run it:

```
gatewayctl --repo <config-repo-dir> [--listen 127.0.0.1:6187] \
           [--join-token <secret>] [--token-ttl 300] [--poll-interval 3] \
           [--push-raw <snapshot-file>]
```

Proof lives in [`scripts/fleet-demo.sh`](scripts/fleet-demo.sh) →
[`fleet-demo.log`](fleet-demo.log): one control plane, two data planes, both
joining and receiving v1, a config change rolling out v2 to both with ACKs, and
a deliberately-invalid snapshot both nodes NACK while the fleet stays on its
committed version.

## What M1 does

- **Rendered-manifest compilation** ([`render.rs`](src/render.rs)). Reads a
  config repo whose directory layout mirrors the four policy scopes
  (`fleet → project → route → app`), assembles the fragments into one flat
  `Config`, and validates it by reusing `gateway-core`'s scope composition +
  validation verbatim. Rendering is a **pure function** of the repo bytes:
  fixed read order, canonical (sorted-key) serialization, SHA-256 of the
  *rendered* bytes as `render_hash`. Same repo → same bytes → same hash,
  forever — the six-month rule made mechanical. An invalid repo is rejected at
  render time, before any node sees it.

  Repo layout (M1):

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

- **The gRPC stream** ([`server.rs`](src/server.rs), wire types in
  [`gateway-proto`](../gateway-proto)). The `FleetService.Session` bidi stream:
  the control plane is the server, the data plane dials out. It authenticates
  each `Hello` via a **join token**, pushes the current snapshot to a freshly
  joined node, and — on a local reload — runs one **all-or-nothing wave** across
  every connected node, surfacing each node's Ack/Nack. The control plane never
  pushes imperative mutations; there is no `Patch` message and there will not be
  one (docs/07 enforced at the protocol level).

- **All-or-nothing waves** ([`fleet.rs`](src/fleet.rs)). M1 implements a single
  wave over all connected nodes (multi-wave grouping by failure domain is
  deferred). The load-bearing half is real: on **any** Nack (or a silent node
  past the timeout) the wave **halts**, the divergence is logged loudly and left
  surfaced (never silent), and the fleet's committed version does **not**
  advance. A fully-acked wave commits and advances it.

- **Join-token bootstrap + identity-scoped reconnect** ([`token.rs`](src/token.rs)).
  Single-use, short-TTL tokens bound to labels. A bad token refuses the stream
  (Unauthenticated); an expired-unused token is rejected; a failed check never
  burns a live token. The FIRST successful join burns the token and binds it to
  the joining `node_id` — the M1 stand-in for the per-node cert docs/07
  describes ("the token authenticates the join; the node cert authenticates
  every subsequent stream"). A **reconnect** is that same node re-presenting its
  now-burned token: admitted as an established identity so a node can re-dial
  after a stream drop without a fresh token. A **different** node replaying that
  burned token is refused (`AlreadyUsed`) — the reconnect path is identity-scoped,
  never a bypass.

- **Node auto-reconnect + hash self-verification** ([`gatewayd/src/client.rs`](../gatewayd/src/client.rs)).
  The data-plane client SUPERVISES its stream: on a stream drop or a
  control-plane outage it keeps serving its last bound snapshot (the control
  plane is not a SPOF for serving) and re-dials with exponential backoff,
  rejoining on its established identity and resuming pushes — it neither crashes
  nor goes permanently quiet. On every push the node ACKs with the SHA-256 it
  **recomputes** of the bytes it actually bound, not the control plane's
  advertised `render_hash`; a control-plane bug (or tampering) that advertises a
  hash inconsistent with the shipped config is therefore caught at the wave as a
  `WrongHash` divergence — an independent verification, per docs/07 line 71
  ("hashes the incoming config bytes ... before anything else").

- **In-memory runtime state** ([`store.rs`](src/store.rs)). Connected nodes,
  their acked versions, last NACK, observed hash, and health. **Postgres
  replaces this later and is never truth** — every field is observed reality,
  re-derivable from Git plus the stream. There is deliberately no field for
  desired state ("this node *should* run vN"); that is recomputed from the
  applied render every time, never stored.

- **Reload triggers.** SIGHUP (immediate) and a poll watcher both re-render the
  repo and roll out a wave if the render changed — the fleet analog of the data
  plane's single reload path. A broken repo edit is rejected loudly and the last
  good render keeps being the fleet's desired state.

## Deferred beyond M1 (stated, not implied)

Per docs/07's open questions and the task's milestone scope:

- **Git integration.** M1 reads a plain directory; libgit2 / commit hashes /
  webhook-or-poll of a real repo are the layer *above* `render.rs`.
  `source_commit` is a content-derived id in M1 (`m1-<hash>`), stable for
  identical repo content, so the six-month reproducibility property already
  holds — only the id's provenance changes when Git lands.
- **Postgres.** The runtime store is in-memory (above).
- **Drift detection and self-heal reconciler.** The `Status` heartbeat carries
  the observed render hash (the input to drift detection) and the store records
  it, and the node ACK now carries a locally-recomputed hash so a delivered-vs-
  bound mismatch is caught at the wave; but the periodic
  desired-vs-delivered-vs-observed reconciler *tick* (the loop that re-pushes on
  observed drift between waves) is a later milestone.
- **Per-node certificates.** M1 binds a burned join token to its node_id and
  admits reconnects on that binding (the identity check above). Issuing a
  separate, independently-revocable node certificate — and revoking it centrally
  to eject a node — is the layer that replaces the token-binding stand-in later.
- **Config-PR admission checks.** CEL validations on config PRs before render
  are out of scope here.
- **Multi-wave rollouts** grouped by failure domain, and per-node latching.
  M1 is a single wave; the sequencing substrate is in place.

## The `--push-raw` break-glass affordance

`--push-raw <file>` arms a SIGUSR1 handler that distributes that file's bytes as
a raw snapshot **bypassing the render gate**. This exercises the node's
*independent* validation authority (docs/07: "A snapshot that fails local
validation is Nacked and the old one keeps serving"): the control plane's render
gate is the first defense, the node's NACK is the second, and this path proves
the second is real. The demo uses it to show both nodes NACK an invalid snapshot
while keeping their committed version.

## The two-binaries budget

`gatewayctl` and `gatewayd` are the two binaries;
[`gateway-proto`](../gateway-proto) and [`gateway-core`](../gateway-core) are the
libraries they share. The reconciler, gRPC server, compiler, and wave state
machine are modules in this one process, not services — docs/07's budget kept
honest.
