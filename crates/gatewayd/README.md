# gatewayd

The standalone data plane: a config-driven Pingora proxy enforcing the
scoped policy chain (fleet → project → route → app), attribution
(GB-1/2/3 + CEL-derived values), operator-owned rejections (GB-4), STS
session-tag credentials for Bedrock (GB-7), Vertex billing labels (GB-8),
with a streaming tap and meter on every response. Phase 1, milestones 1-3.
Run it:

```
gatewayd --config gateway.yaml [--listen 127.0.0.1:6188] [--poll-interval 3]
```

Proof lives in `scripts/demo.sh` → `demo.log`, and in the conformance
suite below.

## Status: Phase 1 checks

Run `cargo test -p gatewayd --test conformance` — every row is verified
against a REAL gatewayd instance (the spawned binary) with mock upstreams,
and the run writes the machine-readable summary `target/conformance.json`
(check → test → pass) for the tracker's consumption.

| Check | Status | Named tests (tests/conformance.rs) |
|---|---|---|
| GB-1 required attribution keys | **Implemented** | `gb1_missing_required_key_rejected_with_operator_template`, `gb1_required_key_present_reaches_the_upstream`, `gb1_app_override_pin_satisfies_a_required_key` |
| GB-2 claim mappings from verified logins | **Implemented** (HS256; asymmetric algs later) | `gb2_claim_mapped_key_proven_from_verified_jwt`, `gb2_forged_caller_header_never_believed_without_token` |
| GB-3 operator-pinned values | **Implemented** | `gb3_pinned_key_overwrites_caller_value` |
| GB-4 rejection templates | **Implemented** (bodies + scoped overrides; the mid-stream terminal event is typed/validated, the cut itself is Phase 3) | `gb4_unknown_route_uses_the_operator_template_verbatim`, `gb4_scoped_rejection_template_overrides_down_the_chain` |
| GB-5 spend caps | Phase 3 | — |
| GB-6 native alerts | Phase 3 | — |
| GB-7 invoice-grade attribution on AWS | **Mechanism implemented** (mock-verified; live AWS is a documented follow-up, below) | `gb7_session_tags_ride_the_credentials_to_bedrock`, `gb7_credentials_cached_per_tag_set_with_expiry`, `gb7_caller_raw_session_tag_rejected_at_config_load` |
| GB-8 invoice-grade attribution on Vertex | **Implemented** (same semantics as our upstream agentgateway PR, native) | `gb8_operator_labels_merged_into_body_operator_wins`, `gb8_unresolvable_label_fails_closed_with_gb4_template` |
| GB-9 hot-swappable config | **Implemented** (single node; fleet waves are Phase 2) | reload/proxy unit tests + demo scenarios 7-9 |
| Tier-1 CEL (conditions, derivations, label exprs) | **Implemented** (compile-at-load, depth/cost limits, comprehensions banned, sandboxed) | `cel_route_condition_gates_matching_beyond_prefix`, `cel_derived_attribution_value_from_claim_transform`, `cel_comprehension_label_rejected_at_config_load` |
| Scoped chain composition | **Implemented** (exhaustive precedence tests in `gateway-core/tests/scope_precedence.rs`) | `scope_chain_composes_fleet_project_route_app` |

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
GB-4 `missing_attribution` rejection, fail closed, before the provider
sees the request. On labeled vertex routes the request body is buffered
for the merge (upstream leg re-framed chunked); a body that is not a
JSON object is refused with a plain 400 — no spend can have occurred.

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
- **No stateful-policy migration yet.** Anything that owns counters
  (budgets, rate limits, quota shares) is a state-migration problem across
  a swap — inherit versioned counters or reset them (doc 03, limitation 3).
  Phase 3/4 scope. The GB-7 credential cache deliberately lives OUTSIDE
  snapshots: its key carries every input that changes the minted
  credentials, so a config swap simply stops hitting stale entries.
- **Single node.** Fleet distribution (ACK/NACK waves, canary
  configuration) is the control-plane phase; this crate is one node
  latching or rejecting its own file.
- **GB-7 live-AWS verification** — see "GB-7 status" above.
