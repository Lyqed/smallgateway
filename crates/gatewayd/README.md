# gatewayd

The data plane: a config-driven Pingora proxy enforcing the scoped policy
chain (fleet → project → route → app), attribution (GB-1/2/3 + CEL-derived
values), operator-owned rejections (GB-4), STS session-tag credentials for
Bedrock (GB-7), Vertex billing labels (GB-8), with a streaming tap and meter
on every response. Phase 1 (milestones 1-3) standalone; Phase 2 M1 adds a
control-plane client mode.

Run it — one of two config sources:

```
# File mode (Phase 1): a local YAML, hot-reloaded on SIGHUP or the poll watcher.
gatewayd --config gateway.yaml [--listen 127.0.0.1:6188] [--poll-interval 3]

# Control-plane mode (Phase 2 M1): dial gatewayctl, receive rendered snapshots.
gatewayd --control-plane <host:port> --node-id <id> --join-token <secret> \
         [--config-source control-plane] [--listen 127.0.0.1:6188]
```

Proof lives in `scripts/demo.sh` → `demo.log` (file mode, GB-1..9), in the
budget demo `scripts/budget-demo.sh` → `budget-demo.log` (Phase 3: GB-5 caps,
GB-6 alerts, mid-stream cut, MEASURED partition overspend), in the fleet demo
`../gatewayctl/scripts/fleet-demo.sh` → `../gatewayctl/fleet-demo.log`
(control-plane mode across two nodes), and in the conformance suite below.

## Control-plane client mode (Phase 2, milestone 1)

Instead of reading a local file, gatewayd dials a `gatewayctl` endpoint with a
join token, holds one long-lived bidirectional gRPC stream open, and binds each
pushed `RenderedSnapshot` through the **existing** `SharedSnapshot` / `Reloader`
machinery (see `src/client.rs`). The seam is deliberately narrow: the control
plane is just one more reload trigger, and the doc-03 hot-swap semantics carry
over **unchanged** —

- **Drain on old.** In-flight streams finish on the version they bound at
  request start; a swap only affects new binds (`Arc<Snapshot>` refcount).
- **NACK keeps old.** A pushed snapshot that fails local validation is NACKed
  over the stream and the old snapshot keeps serving — the node is an
  *independent* validation authority, never silently wrong.
- **No-op is an ACK.** A re-push of the active render hashes to the active
  snapshot and short-circuits; the node ACKs the version it is already at.

Each push maps straight onto the wire: `Swapped`/`NoOp` → `Ack`, `Rejected` →
`Nack` (with the precise validation reason). The node also heartbeats `Status`
carrying the render hash it is actually running. Bootstrap blocks until the
first snapshot is received and bound — a node with no valid config never serves.

File mode is unchanged and fully supported; the two sources are mutually
exclusive and selected by `--config` vs `--control-plane` (or pinned with
`--config-source file|control-plane`).

## Status: Phase 1 checks

Run `cargo test -p gatewayd --test conformance` — every row is verified
against a REAL gatewayd instance (the spawned binary) with mock upstreams,
and the run writes the machine-readable summary `target/conformance.json`
(check → test → pass) for the tracker's consumption.

| Check | Status | Named tests (tests/conformance.rs) |
|---|---|---|
| GB-1 required attribution keys | **Implemented** | `gb1_missing_required_key_rejected_with_operator_template`, `gb1_required_key_present_reaches_the_upstream`, `gb1_app_override_pin_satisfies_a_required_key` |
| GB-2 claim mappings from verified logins | **Built, deferred by judgment** (HS256 mapping works, but this is the lowest-priority check and may not ship as a promised capability, see the note below) | `gb2_claim_mapped_key_proven_from_verified_jwt`, `gb2_forged_caller_header_never_believed_without_token` |
| GB-3 operator-pinned values | **Implemented** | `gb3_pinned_key_overwrites_caller_value` |
| GB-4 rejection templates | **Implemented** (bodies + scoped overrides; the mid-stream terminal event is typed/validated, the cut itself is Phase 3) | `gb4_unknown_route_uses_the_operator_template_verbatim`, `gb4_scoped_rejection_template_overrides_down_the_chain` |
| GB-5 spend caps | **Implemented** (budget shares: per-value caps, ~90% synchronous escalation, mid-stream cut, bounded overspend under partition MEASURED) | `gb5_value_over_its_cap_is_rejected_at_request_start_with_the_operator_template`, `gb5_budget_exhausted_mid_stream_cuts_with_the_gb4_terminal_event`, `gb5_an_uncapped_value_override_is_never_cut`, `gb5_cap_composes_down_the_scoped_chain_route_tightens_fleet` (`tests/conformance/gb5.rs`) |
| GB-6 native alerts | **Implemented** (soft 80% + hard cap fire from the enforcement layer; log + webhook-shaped sink) | `gateway_core::budget` (`AlertLatch`), `gatewayd::budget` (`alerts_fire_from_the_meter_at_soft_then_hard`), `gatewayctl::tests::budget` (`a_fleet_wide_spend_crossing_fires_a_gb6_alert_from_the_ingest`) |
| GB-7 invoice-grade attribution on AWS | **Mechanism implemented** (mock-verified; live AWS is a documented follow-up, below) | `gb7_session_tags_ride_the_credentials_to_bedrock`, `gb7_credentials_cached_per_tag_set_with_expiry`, `gb7_caller_raw_session_tag_rejected_at_config_load` |
| GB-8 invoice-grade attribution on Vertex | **Implemented** (same semantics as our upstream agentgateway PR, native) | `gb8_operator_labels_merged_into_body_operator_wins`, `gb8_unresolvable_label_fails_closed_with_gb4_template` |
| GB-9 hot-swappable config | **Implemented — full doc-03 semantics (Phase 4)**: atomic config+**module** binding per snapshot, per-stream drain, versioned counter schemas with migration hooks, break-glass with TTL. Single node; fleet distribution is the control-plane phase (same reload path, one more trigger). | reload/proxy unit tests + demo scenarios 7-9; `wasm_runtime` tests (atomic module bind, drain, unsigned-fails-bootstrap, break-glass revert); `../gateway-wasm/scripts/demo.sh` |
| Tier-1 CEL (conditions, derivations, label exprs) | **Implemented** (compile-at-load, depth/cost limits, comprehensions banned, sandboxed) | `cel_route_condition_gates_matching_beyond_prefix`, `cel_derived_attribution_value_from_claim_transform`, `cel_comprehension_label_rejected_at_config_load` |
| Tier-2 WASM policy modules | **Implemented (Phase 4, `crates/gateway-wasm`)**: signed modules only (fuel + epoch bounded, no ambient I/O), `on_request`/`on_response_end` hooks promised; per-event streaming hooks **gated off by default** behind the measured ~11.7 µs/event budget (the named risk, measured — see `../gateway-wasm/README.md`). | `gateway-wasm` unit + `sandbox_and_bounds` integration tests; `wasm_runtime` tests; `../gateway-wasm/scripts/demo.sh` |
| Scoped chain composition | **Implemented** (exhaustive precedence tests in `gateway-core/tests/scope_precedence.rs`) | `scope_chain_composes_fleet_project_route_app` |

## A note on GB-2 (identity from a verified login)

GB-2 is the lowest-priority check, and whether it ships as a promised
capability is an open decision, not a foregone one. The mechanism (map an
attribution key from a verified JWT claim instead of a caller header) is
built and tested, because building it was the cheap part. Deciding it
deserves to run is the expensive part, and the honest answer so far is
that it may not.

The failure GB-2 defends against, a caller putting a false tag in a
header, is largely self-correcting: a disputed invoice line gets caught
by the team that reads its own bill. Against that, a directory or JWT
check costs response time on every request, forever, and adds one more
component that needs an owner and a group-mapping table that will drift.
A chargeback report built on stale mappings is worse than one built on
honest headers. Coding assistants sharpen the point: the heaviest new
callers run half on a developer's machine and are hard to bind to a
directory identity at all.

There is also a real chance the work is never ours to do. Identity
verified at the source may arrive as part of the platform, and a
deferral that ends with someone else building the thing is the cheapest
engineering there is. So GB-2 stays built-but-deferred: the code is
present and the tests pass, the check sits last in priority, and the
project does not present it as a settled, first-class feature. GB-3
(operator-assigned values) carries the load in the meantime, which is
the mode most callers actually use.

When a fleet DOES turn it on (an explicit `auth:` block in that fleet's
Git repo — absent means off, zero verification cost, zero behavior
change), a verified claim reaches everything an attribution value
reaches: required keys and pins via `from_claims`, CEL derivations and
labels via `jwt.claims.<claim>`, and — since the per-request role
identity work — STS session tags, the templated RoleSessionName, and
operator-forced guardrail values. The two-hop conformance test exercises
exactly this: `from_claims: { user: sub }` feeds `{{user}}` into the
RoleSessionName, so a verified login lands in CloudTrail's identity
column. One honesty boundary: verification is HS256 (shared secret)
today, which fits a fleet that mints its own tokens; RS256/JWKS against
a real IdP is the follow-up that would make GB-2 production-ready for
directory-backed identity, and it is not built.

## The scoped policy chain

Config composes `fleet → project → route → app` (docs/02): lists
(`required_keys`, `labels`) inherit when absent, REPLACE when present
without the explicit `<base>` marker, and splice the parent at the
marker's position; maps (`pinned`, `from_claims`, `derived`) merge with
the lower scope winning within one origin — a key pinned at one scope and
claim-mapped/derived at another is a **contradictory pin** and fails
validation with both scopes named. Rejection templates override per
reason. Routes reference a project (`project:`); apps are keyed by the
RESOLVED value of one attribution key (`apps.key`), so an app is an
adjudicated identity, and an app override may satisfy a requirement
(e.g. pin a required key) — enforcement runs against the FINAL composed
policy. An app override may not redefine its own selector key.

## Tier-1 CEL

`match:` route conditions, `derived:` attribution values, and label
`expression:`s compile at config load (a typo is a startup/reload error,
never a request-time surprise) and evaluate per request against a small
documented context — `request.method`, `request.path`, `request.headers`,
`jwt.claims` (verified only; `{}` otherwise), and, for label expressions
only, `attribution`. The interpreter (the maintained `cel` crate) is
sandboxed — no I/O; on top: source ≤512 bytes, bracket nesting ≤16,
unknown context variables rejected at compile, and comprehension macros
(`map`/`filter`/`all`/`exists`/`exists_one` — CEL's only loops) rejected
at compile, so evaluation is loop-free and its cost stays bounded no
matter what a caller puts in a header (`has(…)` is not a comprehension
and keeps working). An erroring route condition never selects a route;
an erroring derivation leaves its key unresolved (a required key then
rejects, fail closed).

## GB-7 status: what is proven, what is deferred

Proven here, against a mock STS + mock Bedrock pair (no real AWS account
exists in this environment):

- **AssumeRole with session tags** where every tag value is operator
  static or a RESOLVED attribution value; a tag referencing a key that is
  only caller-asserted (required but neither pinned, claim-mapped, nor
  derived) is rejected at config load — *never caller-raw*.
- **Per-tag-set credential cache** honoring the STS-granted expiry with a
  refresh margin: one exchange per unique tag-set (the demo and the
  conformance test read the mock STS's exchange counter off the echoed
  access key id).
- **SigV4 signing** of the upstream request (the signature math is
  verified against the official AWS documentation example in
  `gateway-core/src/aws.rs` tests), and independent **verification** at
  the mock Bedrock, which decodes the session tags from the security
  token and echoes them — the attribution rode the credentials.

Documented follow-up for live AWS (deliberately out of scope until a real
account exists to verify against):

1. the AssumeRole call itself must be signed with base credentials
   (instance profile / env chain);
2. Bedrock requires a signed payload hash — requests here are signed
   `x-amz-content-sha256: UNSIGNED-PAYLOAD`;
3. role trust policies, `sts:TagSession` permissions, and cost-allocation
   tag activation in the payer account.

## GB-8: Vertex billing labels

The same semantics we wrote for upstream agentgateway
(`upstream/agentgateway/gb8-vertex-operator-labels`), native: labels on
any scope of a chain that targets a vertex-kind provider — static
`value`, `from_attribution` (a resolved attribution key), or a CEL
`expression` — are merged into the outbound `generateContent` body.
Operator labels win key conflicts (a caller cannot override the
gateway's cost attribution); non-conflicting client labels pass through.
Keys/static values are validated against Google Cloud's label rules at
load, dynamic values per request. Unresolvable → the route's effective
GB-4 `default_response` rejection, fail closed, before the provider
sees the request. On labeled vertex routes the request body is buffered
for the merge (upstream leg re-framed chunked); a body that is not a
JSON object is refused with a plain 400 — no spend can have occurred.

## GB-5 spend caps via budget shares, GB-6 alerts, mid-stream enforcement

The Phase 3 stateful layer (docs/01 Q4; docs/02 "GB-5 at fleet scale — budget
shares"; docs/04 Phase 3). Proof: `scripts/budget-demo.sh` → `budget-demo.log`.

### The 100k-token scenario, five lines of YAML

A spend cap is a field on any scope's `attribution`, keyed by attribution key,
in **tokens** (`demo/budget.yaml`):

```yaml
spend_caps:
  team:
    default: 100000            # every value of `team` capped at 100k tokens
    overrides:
      ml-research: 200000      # a Git-reviewed per-value lift
      free-tier: 5000          # and a per-value tighten
```

It composes down the scoped chain exactly like the pins: a lower scope's
`default` and per-value entries win (a route can tighten the fleet default; an
app override can loosen one value). A value with no default and no override is
**uncapped**; a `null` override is an explicit "this value is uncapped."

### Budget shares (the distributed-systems core)

A spend limit per attribution value enforced across N data planes is the hard
problem. The chosen design (docs/01 Q4) is **budget shares**, not a central
counter (a hop + SPOF per request) and not pure-local buckets (unbounded
overspend):

- The control plane allocates each data plane a **share** of the cap from
  observed spend telemetry (a node reports its cumulative spend up the existing
  FleetService stream as a `UsageReport`; the control plane rebalances and grants
  shares back, so a **hot node gets a bigger slice**).
- A data plane spends **freely against its local share** — the common path is one
  in-memory counter check per capped tag, **no per-request hop, no SPOF**.
- It **escalates to a synchronous check** with the control plane only above **~90%**
  local-share consumption (a `SyncCheck`, answered with a fresh `ShareGrant`).

### Bounded overspend under partition — MEASURED, not estimated

When a node cannot reach the control plane it fails to a **documented
bounded-overspend policy**: it spends only up to the share it already holds, then
hard-denies. The bound is one in-flight stream's tail past the share — never the
unbounded local-bucket failure. The number is measured, not estimated:

```
[MEASURED] partition bounded-overspend: cap=100000 tokens, held_share=40000,
spent=41600, overspend=1600 tokens (1.60% of the cap); the node was UNREACHABLE
and stopped at its share + one stream's tail, never unbounded
```

(`budget::tests::measured_bounded_overspend_under_partition`, captured in
`budget-demo.log` scenario 5.)

### Enforcement, at request start and mid-stream

- **Request start.** A value already at its cap is rejected with the operator's
  GB-4 template (status + body), which now also accepts optional `{{cap}}` and
  `{{spend}}` token placeholders. No token reaches the upstream.
- **Mid-stream.** The Meter tap over the canonical event stream tallies output
  tokens incrementally; when the running tally crosses the bound the stream is
  **cut** with the operator's GB-4 **streaming terminal event** (`event:` /
  `data:`) rather than running to completion — GB-4 extended to streaming. The
  live estimate (`chars/4`) meters the stream and is **reconciled to the
  provider's terminal usage frame** at stream end (docs/01 Q3); a cap tightened
  mid-stream does **not** retroactively apply (docs/03 limitation 2 — the stream
  meters under the version it bound, `cfg=vN` on the `[budget]` line).

### GB-6 alerts from the enforcement layer

Alerts fire **at the point of enforcement**, not reconstructed later from logs: a
soft alert when a spender crosses 80% and a hard alert when it hits the cap, each
carrying the attribution value, cap, current spend, and node/fleet context. The
data plane fires per-node from the meter; the control plane fires **fleet-wide**
from the aggregated usage telemetry (so a crossing no single node reached alone
is still caught). Delivery is pluggable — the milestone ships a structured
`log::warn!` sink plus a webhook-shaped JSON body (logged, not yet POSTed).

## Hot reload: what is promised

The doc-03 semantics (`docs/03-hot-swap.md`), made real for a single node:

- **Versioned snapshots.** Rendering = load + validate + compose + stamp:
  every accepted config becomes an immutable `Snapshot` (composed
  effective policies included) with a monotonically increasing version
  and the SHA-256 of its source bytes. Validation is fail-fast; a
  rejected file consumes no version number.
- **Atomic per-request binding.** A request binds one `Arc<Snapshot>` at
  request start and consults only that snapshot for its whole lifetime —
  routing, attribution, upstream choice, metering. A request never sees
  two versions (no torn reads). New requests bind the newest snapshot.
- **Drain.** A swap never rebinds an in-flight request. The old snapshot
  stays resident until its last in-flight stream drops it (Rust
  refcounting, made explicit and tested — `reload.rs` / `proxy.rs` tests),
  so during the overlap two versions are live simultaneously, on purpose.
- **NACK keeps old.** A reload whose file fails validation is REJECTED:
  the old snapshot keeps serving, and the rejection is logged at error
  level with the precise validation errors and the still-active version —
  divergence surfaced, never silent (doc 03, limitation 1).
- **Identical content is a no-op.** The reload path hashes the file first;
  a matching hash logs at debug level and changes nothing.
- **Two triggers, one path.** SIGHUP and a poll-based mtime watcher
  (`--poll-interval` seconds, default 3, `0` disables) both funnel through
  the same reload routine.
- **Versions are observable.** Every `[req]`, `[attr]`, `[gb7]`, `[gb8]`
  line, and the end-of-stream `[meter]` line, carries `cfg=vN`; the
  reload path logs old→new version, source hash, and swap timestamp. That
  is the published bounded-staleness evidence: which version metered
  which stream is a grep.

## What is NOT promised

- **No mid-stream rebind.** A cap or policy tightened mid-stream does not
  apply to streams already running; they finish under the version they
  started with. This is doc 03, limitation 2 — a bounded-staleness
  semantic to state, not a bug to fix. The `cfg=vN` on the `[meter]` line
  is the error bound made visible.
- **Stateful-policy migration — DELIVERED in Phase 4** (`crates/gateway-wasm`
  `state.rs`, and `wasm_runtime.rs` here). A WASM module that owns counters
  gets a versioned counter schema and a migration hook: on a swap,
  `ModulePlan::diff` **inherits** counters on an unchanged schema, **migrates**
  them (transformed) on a schema bump with a declared migration, or **resets**
  them with a **stated bounded window** on a bump with none — never silent
  (doc 03 limitation 3, now addressed). The counters live OUTSIDE snapshots
  (like GB-5's budget counters and the GB-7 credential cache), so they survive
  the swap and migrate per the module's declared schema. GB-5 budget counters
  themselves still carry forward as-is (their cap is per-pinned-policy, not a
  migratable schema). The migration *catalog* (declaring per-module migration
  chains) is a wired seam (`migrations_for`), empty today — so an undeclared
  schema bump resets with the stated window, the honest default.
- **GB-5 durable counters are in-memory.** Postgres-backed durable spend
  counters are deferred and, per docs/07, never truth; wipe the state and the
  next round of usage telemetry rebuilds the shares. Richer GB-6 alert sinks
  (a real webhook POST, a pager, a bus) are deferred behind the shipped
  log + webhook-shaped emitter.
- **The GB-7 credential cache** deliberately lives OUTSIDE snapshots: its key
  carries every input that changes the minted credentials, so a config swap
  simply stops hitting stale entries.
- **Single node.** Fleet distribution (ACK/NACK waves, canary
  configuration) is the control-plane phase; this crate is one node
  latching or rejecting its own file.
- **GB-7 live-AWS verification** — see "GB-7 status" above.
