# Upstream agentgateway patches (GB-4, GB-8)

Submission-ready patches for the two upstream changes pinned by
[docs/spike-b/agentgateway-embed.md](../../docs/spike-b/agentgateway-embed.md),
prepared locally only (nothing has been pushed, forked, or opened as a PR).

## Base

- **Repo:** https://github.com/agentgateway/agentgateway
- **Base commit:** `66713a6ec3bae2d597bc5639a39bb55da72e871a`
  ("llm: capture tool calls for all provider paths (#2793)", 2026-07-31) —
  the default-branch tip at clone time, and identical to the commit the
  spike-b analysis was performed against, so all file/line references in the
  analysis were exact.
- **Toolchain:** rust-toolchain.toml pins 1.97 (rustup picked it up
  automatically).
- **Work tree:** scratch clone at `/tmp/agentgateway-work`, branches
  `gb4-local-ratelimit-body` and `gb8-vertex-operator-labels`, each exactly
  one commit on top of the base commit.

## Contents

| File | What it is |
|---|---|
| `gb4-local-ratelimit-body/0001-localratelimit-support-a-custom-rate-limit-exceeded-.patch` | GB-4: operator-configurable local rate limit rejection response |
| `gb8-vertex-operator-labels/0001-vertex-support-operator-configured-billing-labels-fo.patch` | GB-8: operator-set (static + CEL) Vertex billing labels |
| `PR-GB4.md` | Draft PR title/body for GB-4 |
| `PR-GB8.md` | Draft PR title/body for GB-8 |

Both patches were produced with `git format-patch <base>..<branch>` and both
pass `git apply --check` against a pristine checkout of the base commit.

## GB-4 — localratelimit: custom rejection response

- `RateLimitSpec` (crates/agentgateway/src/http/localratelimit.rs) gains an
  optional `response` field (`body`, `contentType`, `status`), shaped after
  `filters::DirectResponse` field idioms (Bytes body with base64 serialize,
  `http_serde::option::status_code`).
- `ProxyError::RateLimitExceeded` carries the configured response
  (`Option<Box<RateLimitResponse>>`, following the boxed-response pattern of
  `GuardrailRejected`), and `into_response_with_grpc`
  (crates/agentgateway/src/proxy/mod.rs) honors it: status override in the
  status map, custom body/content-type before the plain-text fall-through.
  `X-RateLimit-*` headers are unchanged; gRPC requests keep the
  trailers-only `grpc-status` response.
- In-tree precedent followed: `remoteratelimit.rs` builds its 429 from the
  rate limit service's `raw_body`.
- New test file `crates/agentgateway/src/http/localratelimit_tests.rs`
  (wired with `#[path]` like `remoteratelimit_tests.rs`): default 429
  behavior unchanged, custom body/status/content-type, defaults when only
  `body` is set, gRPC path ignores the custom body, token-type (LLM) path
  carries the response, config deserialization via `serdes::yamlviajson`
  (the real config parse path — plain `serde_yaml` cannot deserialize
  `Bytes`), unknown response fields rejected.
- `cargo xtask schema` regenerated `schema/config.json` and
  `schema/config.md` (both included in the patch).

## GB-8 — vertex: operator-set billing labels

- New `VertexProvider` wrapper in crates/agentgateway/src/llm/mod.rs
  (mirrors the existing `BedrockProvider`/`AzureProvider` wrapper pattern in
  the same file: `#[serde(flatten)]` + Deref/DerefMut + `AIProvider::vertex()`
  constructor), holding a `labels` field.
- `VertexLabel { key, value, expression }` copies the `AwsSessionTag` shape
  from crates/agentgateway/src/http/auth/aws.rs (#2435/#2447), including the
  static/dynamic split (`VertexLabels::try_new`), config-load validation
  (Google Cloud label key/value charset + length + count limits), custom
  de/ser helpers, `expressions()` for CEL registration, and fail-closed
  per-request resolution.
- Label CEL expressions are registered in
  `BackendPolicies::register_cel_expressions`
  (crates/agentgateway/src/store/binds.rs) alongside the AWS auth
  expressions, so the request snapshot machinery captures what they need.
- `ChatRequestContext` gains the request snapshot; `render_vertex_gemini`
  resolves operator labels and passes them into
  `conversion::vertex_gemini::from_completions::translate`, which now takes
  an `operator_labels` parameter and merges them over client-passthrough
  labels (operator wins on key conflicts).
- New `AIError::LabelResolution` variant in crates/llm/src/lib.rs; failures
  reject the request (maps through `ProxyError::Processing`, same fail-closed
  posture as AWS session tags).
- Tests: crates/llm/src/conversion/vertex_gemini_tests.rs (merge semantics:
  add, merge, operator-wins, empty no-op) and
  crates/agentgateway/src/llm/tests.rs (config-time validation, YAML config
  parse incl. rejection of invalid keys, end-to-end render with a request
  snapshot resolving a CEL label from a header, fail-closed on unresolvable
  expression).
- One insta snapshot updated
  (`types/local_tests/llm_simple_normalized.snap`): serde `flatten` buffers
  the provider struct through a map during serialization, which alphabetizes
  field order in the normalized-config dump (`model, projectId, region`
  instead of `model, region, projectId`). Cosmetic only; wire format
  (JSON/YAML object) is order-insensitive.
- Known tradeoff, same as the existing Bedrock/Azure wrappers: serde does
  not enforce `deny_unknown_fields` through `#[serde(flatten)]`, so unknown
  keys in the vertex provider block are no longer rejected at parse time.
  Called out in the PR draft.
- `cargo xtask schema` regenerated the schema; the flattened wrapper takes
  over the `VertexProvider` definition and adds `labels` +
  `VertexLabel`/`Expression` refs.

## Verification (exact commands, run in /tmp/agentgateway-work)

Scoped to the affected crates (`agentgateway`, `agent-llm`); the full
workspace `--all-targets` suite (integration tests, UI, etc.) was not run.

On `gb4-local-ratelimit-body`:

```
cargo build -p agentgateway                     # ok
cargo test -p agentgateway --lib http::localratelimit
    # ok. 20 passed (8 new tests + 12 pre-existing bucket tests)
cargo test -p agentgateway --lib http::remoteratelimit   # ok. 30 passed
cargo test -p agentgateway --lib llm::tests              # ok. 55 passed
cargo test -p agentgateway --lib proxy::                 # ok. 73 passed
cargo test -p agentgateway --lib
    # ok. 1625 passed; 0 failed; 1 ignored
cargo xtask schema                              # regenerated, diff committed
cargo clippy -p agentgateway --all-targets      # clean, no warnings
cargo fmt --check -- --config imports_granularity=Module,group_imports=StdExternalCrate,normalize_comments=true
    # clean (this is what upstream `make lint` runs)
```

On `gb8-vertex-operator-labels`:

```
cargo build -p agent-llm && cargo build -p agentgateway  # ok
cargo test -p agent-llm
    # ok. 258 passed; 0 failed (includes 4 new vertex_gemini label-merge
    # tests and the golden tests over the changed translate path; the other
    # 4 new label tests are in the agentgateway crate's llm::tests below)
cargo test -p agentgateway --lib llm::tests              # ok. 59 passed
cargo test -p agentgateway --lib types::                 # ok. 228 passed
cargo test -p agentgateway --lib store::                 # ok. 26 passed
cargo test -p agentgateway --lib
    # ok. 1621 passed; 0 failed; 1 ignored
cargo xtask schema                              # regenerated, diff committed
cargo clippy -p agent-llm -p agentgateway --all-targets  # clean, no warnings
cargo fmt --check -- --config imports_granularity=Module,group_imports=StdExternalCrate,normalize_comments=true
    # clean
```

Patch integrity:

```
git worktree add .verify 66713a6ec3bae2d597bc5639a39bb55da72e871a
git apply --check gb4-local-ratelimit-body/0001-*.patch   # clean
git apply --check gb8-vertex-operator-labels/0001-*.patch # clean
```

The two branches are independent (both based directly on `66713a6`); they do
not conflict textually except both regenerate `schema/config.json`/`config.md`,
so whichever lands second needs a trivial `cargo xtask schema` rerun after
rebase.

## Submission notes

- Upstream `CONTRIBUTION.md` (at the base commit) prescribes: fork → feature
  branch → `make lint` + `make test` → PR against main, conventional-commit
  style messages. It does not mention a CLA or DCO at this commit —
  **re-check CLA/DCO requirements at submission time** (the repo is a Linux
  Foundation project; LF projects commonly require DCO sign-off, and a
  `Signed-off-by` line can be added with `git commit --amend -s` before
  pushing if required).
- Review reality per the spike-b governance notes: small sponsored PRs merge
  in hours; external feature PRs (e.g. #2023) can take weeks. Both drafts
  reference the in-tree precedents to shorten review.
