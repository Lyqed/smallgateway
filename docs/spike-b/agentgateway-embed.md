# Spike B, candidate 2 — agentgateway embed/extend feasibility

*Analysis of actual source, no proxy build. Companion to the Pingora half of
the bake-off ([04-build-plan.md](../04-build-plan.md), Phase 0).*

**Repo:** https://github.com/agentgateway/agentgateway (canonical, linked from
agentgateway.dev; Apache-2.0 per `LICENSE`, 4,177 stars / 694 forks at
analysis time).
**Commit analyzed:** `66713a6ec3bae2d597bc5639a39bb55da72e871a`
(2026-07-31, "llm: capture tool calls for all provider paths (#2793)").
All file paths below are repo-relative at that commit; line numbers are exact
at that commit.

---

## 1. Workspace shape and embeddability

### Crates (workspace `Cargo.toml`, members list)

| Crate | Path | Target | Role |
|---|---|---|---|
| `agentgateway` | `crates/agentgateway` | **lib only** (no `[[bin]]`) | The proxy core: listeners, routing, policies, LLM orchestration, MCP/A2A, telemetry, UI assets |
| `agentgateway-app` | `crates/agentgateway-app` | bin | Thin clap CLI; `main.rs` is one line calling `agentgateway_app::run()` |
| `agent-llm` | `crates/llm` | lib | Provider adapters, request/response/stream translation, tokenizer — **no dependency on the proxy** |
| `agent-core` | `crates/core` | lib | strng, drain, readiness, metrics, telemetry plumbing (ztunnel lineage — `crates/agentgateway-app/src/lib.rs:1` credits istio/ztunnel) |
| `agent-http` | `crates/http` | lib | 88-line facade (`crates/http/src/lib.rs`) |
| `agent-xds` | `crates/xds` | lib | ADS gRPC client with ACK/NACK (see §2e) |
| `agent-celx`, `cel`, `cel-derive` | `crates/celx`, `crates/cel-fork/*` | lib | Vendored fork of cel-rust v0.13.0 (MIT) plus extensions (see §2d) |
| `agent-pool`, `agent-hbone`, `htpasswd-verify-fork`, `protos`, `xtask` | `crates/*` | lib | Connection pooling, HBONE tunneling, vendored htpasswd, generated protos, build tasks |

The Go `controller/` directory is the k8s controller; the k8s control plane
proper lives out-of-repo in kgateway.

### Is the proxy core consumable as a library?

**Technically yes, practically only as a git dependency with caveats:**

- Not on crates.io: `publish = false` workspace-wide (`Cargo.toml:46`) and
  `version = "0.0.0"` with the comment "We do not use this version"
  (`Cargo.toml`, `[workspace.package]`). No semver, no API stability contract.
- The public surface is real: `crates/agentgateway/src/lib.rs:23-48` exports
  every module (`proxy`, `llm`, `cel`, `http`, `store`, `state_manager`, …),
  and `crates/agentgateway/src/app.rs:14` exposes
  `pub async fn run(config: Arc<Config>, …) -> anyhow::Result<Bound>` — the
  bin crate is a genuinely thin wrapper, so an external crate *can* embed the
  whole gateway and drive it programmatically.
- **Patch wall:** the workspace pins four `[patch.crates-io]` git forks
  (`Cargo.toml:2-5`: `schemars`, `http-serde`, `wiremock`, `async-openai`,
  all under howardjohn's account). Cargo patches do not propagate to
  consumers, so any embedding workspace must copy those four patch lines and
  track their revs forever. `agent-llm` depends on the patched `async-openai`
  (`crates/llm/Cargo.toml`), so even a minimal embed inherits this.
- **Toolchain:** `rust-toolchain.toml` pins Rust 1.90, edition 2024
  (workspace `Cargo.toml`, `rust-version = "1.90"`). Our Spike A crate builds
  on 1.83; embedding forces the bump.
- **Pull-in:** `crates/agentgateway/Cargo.toml` lists 135 workspace
  dependencies; `Cargo.lock` is 8,833 lines. A full embed brings MCP (rmcp
  v3.1 *and* a legacy rmcp 0.10 with a second reqwest), A2A, the embedded UI
  assets, AWS/Azure SDKs, HBONE, and the k8s xDS client — none of it feature-
  gated off (features at `crates/agentgateway/Cargo.toml:11-20` cover
  allocators/TLS/schema only, not subsystem trimming).
- **Minimal embed that is actually defensible:** `crates/llm` (`agent-llm`)
  alone — provider adapters, typed wire formats, SSE and AWS-event-stream
  codecs, tokenizer — depending only on `agent-core` + `agent-http` + SDK
  type crates. That subset is a parts bin, not a proxy.

**Embeddable: partial.** Lib targets exist and are public, but unpublished,
unversioned, patch-encumbered, and monolithic.

---

## 2. Located paths

### (a) LLM provider adapters and streaming — distance from our canonical event model

Providers: `crates/llm/src/{openai,anthropic,bedrock,vertex,gemini,azure,copilot,custom}.rs`;
typed wire formats: `crates/llm/src/types/{completions,messages,responses,bedrock,vertex_gemini,embeddings,…}.rs`;
translations: `crates/llm/src/conversion/*` (12,775 lines including tests, `wc -l`).

Streaming machinery:

- Generic streamed-body transformer: `crates/llm/src/parse/transform.rs`
  (`TransformedBody` / `parser()`: tokio-util `Decoder` → handler →
  `Encoder` over `http_body` frames, backpressure-native).
- SSE codec: `crates/llm/src/parse/sse.rs`; AWS event-stream (binary) codec:
  `crates/llm/src/parse/aws_sse.rs`.
- Dispatch: `ChatTranslation::stream` at
  `crates/agentgateway/src/llm/mod.rs:536`, selected from the ordered
  pairwise table `CHAT_TRANSLATIONS` at `crates/agentgateway/src/llm/mod.rs:300`,
  entered via `process_streaming` at `crates/agentgateway/src/llm/mod.rs:2255`.

**Key architectural finding: there is no canonical internal event model.**
Streaming translation is pairwise, input-format × output-format
(Completions/Messages/Responses × OpenAI/Anthropic/Bedrock/VertexGemini), and
each pair carries its own hand-rolled state machine. Example: the
Anthropic-client-over-OpenAI-provider path,
`crates/llm/src/conversion/completions.rs:259` (`translate_stream`), keeps a
private `StreamState { sent_message_start, pending_tool_calls, pending_stop_reason, pending_usage, … }`
and emits Anthropic `MessagesStreamEvent`s directly.

Closest analogues to our `MessageStart/ContentDelta/ToolCallDelta/UsageDelta/MessageEnd/Error`:

- Per-provider typed event enums (e.g. `messages::MessagesStreamEvent` in
  `crates/llm/src/types/messages.rs`) — provider protocols reified, not a
  neutral model.
- The shared side-channel `LLMInfo`/`LLMResponse` + `StreamingUsageGuard`
  (`crates/llm/src/lib.rs:219-320`): every streaming path accumulates usage,
  first-token time, finish reason, completion text, and tool calls into one
  structure (e.g. `passthrough_stream` at
  `crates/llm/src/conversion/completions.rs:1150`). This is their
  `UsageDelta`+`MessageEnd` equivalent — but it is observability
  bookkeeping flowing to logs/metrics/rate-limit true-up, not an event stream
  a policy chain can consume mid-flight.

**Distance: conceptually close on data (all six of our event kinds exist as
fragments of their state machines), structurally far on shape.** Our model is
a hub (N adapters → 1 event stream → M renderers); theirs is spoke-to-spoke
(N×M direct translations). Retro-fitting the hub upstream means rewriting the
~12.7k-line conversion layer — not a config knob, and that layer churned in
three separate PRs (#2776, #2783, #2793) in the final week before the
analyzed commit.

### (b) GB-4: the non-customizable local rate-limit 429 body

- The local rate limiter: `crates/agentgateway/src/http/localratelimit.rs`.
  `RateLimitSpec` (lines 28-44) has exactly four fields — `max_tokens`,
  `tokens_per_fill`, `fill_interval`, `type` (requests|tokens). **No body,
  content-type, or CEL field.** `check_request` (line 73) and
  `check_llm_request` (line 88) return
  `ProxyError::RateLimitExceeded { limit, remaining, reset_seconds }`;
  the LLM-token call site is `crates/agentgateway/src/proxy/httpproxy.rs:486`.
- The rejection body is produced in
  **`ProxyError::into_response_with_grpc`, `crates/agentgateway/src/proxy/mod.rs:237`**:
  status mapped to 429 at line 296, `X-RateLimit-*` headers attached at lines
  341-357, and then the body falls through to the generic terminal at
  **lines 404-406**:
  `rb.header(CONTENT_TYPE, "text/plain").body(http::Body::from(msg))` — the
  Display string of the error. That fall-through is the exact spot a
  custom-body change lands.
- The in-tree machinery to wire in already exists:
  `filters::DirectResponse` policy
  (`crates/agentgateway/src/types/agent.rs:2683`, local-config form at
  `crates/agentgateway/src/types/local.rs:43` and `:2653`), and the remote
  rate limiter already builds its own bespoke 429
  (`crates/agentgateway/src/http/remoteratelimit.rs:441`) — precedent that
  per-error response bodies are acceptable upstream. This confirms
  [06-prior-art.md](../06-prior-art.md)'s "config-knob-sized" estimate: add
  an optional response override to `RateLimitSpec`, honor it in
  `into_response_with_grpc` (or intercept `RateLimitExceeded` before it).
- **The streaming half of GB-4 has no landing spot upstream.** Local limits
  reject pre-stream and true-up post-hoc via `amend_tokens`
  (`crates/agentgateway/src/http/localratelimit.rs:125`); there is no
  mid-stream cut that could emit an operator-defined terminal SSE event.
  That capability needs the canonical event layer — i.e., it stays ours.

### (c) Vertex/Gemini translation path (GB-8, PR #2023)

- **PR #2023 is merged** (opened 2026-06-01 by external contributor htimur,
  merged 2026-07-24 by howardjohn, merge commit `20398af`), and the native
  path is in-tree at the analyzed commit:
  - Path construction: `crates/llm/src/vertex.rs:139-160` —
    `…/publishers/google/models/{model}:generateContent`, streaming
    `:streamGenerateContent?alt=sse` (line 144).
  - Request translation: `crates/llm/src/conversion/vertex_gemini.rs`
    (`from_completions` at line 34; streaming response translation
    `translate_stream` at line 1300).
  - Typed request incl. `labels`: `crates/llm/src/types/vertex_gemini.rs:34`.
  - It is preferred over Google's OpenAI-compat endpoint by an explicit
    quirk entry in `CHAT_TRANSLATIONS`
    (`crates/agentgateway/src/llm/mod.rs:305-311`); `native_gemini`
    selection at `crates/agentgateway/src/proxy/httpproxy.rs:1153-1159`.
- **Remaining GB-8 gap at this commit:** billing `labels` are client
  passthrough only — `req.rest.get("labels")` at
  `crates/llm/src/conversion/vertex_gemini.rs:142` — and the Vertex provider
  config (`crates/llm/src/vertex.rs:16-26`) has only
  `model`/`region`/`project_id`. Caller-sent labels violate our
  proven-or-assigned rule (doc 06). The exact precedent to copy is the
  CEL-valued AWS session tags from #2435/#2447:
  `crates/agentgateway/src/http/auth/aws.rs:134-172`
  (`AwsSessionTag { key, value: Option<String>, expression: Option<Arc<cel::Expression>> }`).
  An operator-set labels field on the Vertex provider, merged over (or
  replacing) client labels, is the same shape on the second cloud —
  config-knob-sized, as doc 06 predicted.

### (d) CEL integration

- Interpreter: vendored fork of cel-rust v0.13.0 (MIT) at
  `crates/cel-fork/cel` + `crates/cel-fork/cel-derive` (origin recorded in
  `crates/cel-fork/cel/Cargo.toml`); extension functions in `crates/celx`
  (e.g. CIDR).
- Gateway binding layer: `crates/agentgateway/src/cel/` —
  `Expression` (`mod.rs:40`) with strict/permissive compilation
  (`new_strict` `:338`, `new_permissive` `:315`), `ContextBuilder`
  (`:142`) with lazy per-request property registration, and custom function
  registration (`register_custom_functions`, `:105`).
- The part worth stealing outright: **dependency-analyzed lazy context** —
  `needs_llm` / `needs_llm_prompt` / `needs_llm_completion` /
  `needs_llm_tool_calls` (`mod.rs:288-308`) walk the compiled AST so
  expensive LLM material is only captured when an expression references it.
  CEL values are already threaded into credentials (AWS session tags, §2c),
  conditional `directResponse`, authorization, and transformations — the
  "CEL everywhere" premise of Phase 1 is proven in this codebase.

### (e) Config loading/reload; xDS

- Static parse: `crates/agentgateway/src/config.rs` (1,700 lines); local
  file format: `crates/agentgateway/src/types/local.rs`.
- **Hot reload exists but is file-watch swap, not snapshots:**
  `crates/agentgateway/src/state_manager.rs` — `LocalConfigProvider::run`
  (line 129) loads (`reload_config`, line 237), then `watch_config_file`
  (line 140) re-parses on change; `reload_config_after_change` (line 285)
  keeps the previous state on parse failure. No versioning, no ACK to
  anything, no drain coordination with in-flight streams.
- **A real xDS client exists:** `crates/xds/src/client.rs` — long-lived ADS
  gRPC with explicit `XdsSignal::Ack | Nack` (lines 453-461), per-resource
  rejection aggregation with severities (`RejectedConfig`, lines 53-97),
  typed per-type-url handlers, reconnect with capped backoff (line 498+).
  Wired in `state_manager.rs:64-66` (`ADDRESS_TYPE`, `ADP_TYPE` handlers).
  This is the ztunnel ADS shape; the server side lives in kgateway (k8s
  only).
- **What our snapshot ACK/NACK model can reuse:** the client loop, the
  NACK-with-collected-reasons discipline, and the handler registration shape
  are directly reusable (or portable — the crate is self-contained). What it
  does not give us: whole-snapshot atomicity (resources apply incrementally
  per type-url, not as an all-or-nothing versioned bundle), join-token
  bootstrap, or drift detection. Those remain greenfield exactly as doc 04
  assumes.

---

## 3. Governance (extend-don't-fork viability)

- **Foundation:** Linux Foundation — `CHARTER.md` is a full LF Projects, LLC
  series technical charter with a TSC whose voting members are the
  maintainers; `README.md:130-131` states "Agentgateway is a Linux
  Foundation project." The repo claims no CNCF status at this commit (its
  k8s control-plane sibling kgateway is the CNCF-track project).
  Maintainership is charter-open: "A Contributor may become a Maintainer by
  a majority approval of the TSC" (`CHARTER.md` §2c).
- **Maintainer concentration: high.** `CODEOWNERS` is a single line —
  `* @agentgateway/maintainers`. GitHub contributors API: howardjohn 877
  commits vs. EItanya 196 (#2), npolshakova 109 (#3). Of six sampled merged
  PRs (#2793, #2755, #2740, #2766, #2745, #2023), **howardjohn merged five**,
  including everyone else's; one was self-merged by markuskobler. Day-to-day
  merge authority is effectively one person, with a handful of others able.
- **Review latency: excellent for small/sponsored work, slow for external
  features.** Of 30 PRs merged 2026-07-28..31, nearly all merged within
  1-24 hours of opening (e.g. #2788 in ~15 min, #2766 in ~1.5 h). The
  counter-sample: #2023, an external feature PR, took ~7.5 weeks
  (2026-06-01 → 2026-07-24) — but it did land, verbatim in the direction doc
  06 wanted.
- **Release cadence: monthly minor trains** with alpha/beta/rc tags:
  v1.2.0 (2026-05-14), v1.3.0 (2026-06-18), v1.4.0 (2026-07-27),
  patch v1.4.1 two days later (GitHub releases API).
- **Velocity as a fork-cost proxy:** ~30 PRs merged in the four days before
  the analyzed commit, much of it inside `crates/agentgateway/src/llm` and
  `crates/llm` — precisely the code a fork or deep embed would touch. A fork
  diverges within weeks.

---

## 4. Verdict — ranked

**1. Upstream-extend-and-wrap.** Extend: land GB-4 (429 body override — the
exact fall-through is `crates/agentgateway/src/proxy/mod.rs:404-406`, with
`DirectResponse` and `remoteratelimit.rs:441` as in-tree precedent) and GB-8
(operator-set/CEL Vertex labels — copying `http/auth/aws.rs:134-172` onto
`vertex.rs`'s provider) as small PRs. Both are genuinely config-knob-sized;
#2023's merge proves the direction is welcome, and doc 06's thesis holds:
these PRs pay off whether or not agentgateway is our foundation. Wrap: keep
our data plane built on our own canonical event model (Spike A), because the
one thing our architecture cannot inherit is the thing agentgateway
structurally lacks — a hub event stream that policies can meter and cut
mid-flight (§2a, §2b). Selectively vendor or depend on the separable parts
(`crates/llm` typed formats and codecs, `crates/xds` client, the lazy-CEL
pattern in `crates/agentgateway/src/cel`).

**2. Build-on-Pingora (the other bake-off arm), with parts from here.** From
this side of the bake-off: nothing found disqualifies Pingora, and the
biggest cost it implies — rebuilding LLM awareness — is smaller than it
looks, because Spike A already covers the three-provider parsing core and
`agent-llm`'s typed formats are borrowable. Final call belongs to the Pingora
spike, but the burden of proof has shifted onto embedding.

**3. Embed-as-library: partial, and not as the foundation.** Full-proxy
embed is technically possible (`app::run` is public, lib-only target) but
strategically weak: unpublished (`publish = false`, `Cargo.toml:46`), no
semver, four `[patch.crates-io]` forks to mirror (`Cargo.toml:2-5`),
Rust 1.90 pin, 8,833-line lockfile including MCP/A2A/UI we don't want, and —
decisive — our event model would sit on top of pairwise translations that
upstream rewrote three times in the week before the analyzed commit. Embedding
`agent-llm` alone as a parts bin is the only embed worth keeping on the table.

**4. Fork: rejected.** ~30 merged PRs in four days concentrated in the LLM
layer means immediate divergence; one dominant merger means no shared review
economy for a fork to draft behind; and the LF charter plus observed
receptiveness (#2023 merged) makes upstreaming strictly cheaper than
carrying patches.
