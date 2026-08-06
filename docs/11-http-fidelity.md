# HTTP fidelity

*The design wasn't too close to HTTP. It wasn't close enough.*

On 2 August 2026 the design docs and the promoted data plane went through an
adversarial HTTP-fidelity review: six dimensions — streaming wire semantics,
header hygiene, connection lifecycle and cancellation, body and content
coding, error semantics, and how real clients actually talk to LLM APIs —
with every finding independently verified against both the docs and the
`crates/` code before it counted. Thirty-seven findings survived; none were
refuted. This document is the distillation, kept in the repo for the same
reason doc 03 exists: every edge below must become a **stated invariant**,
never a silent behavior discovered in an incident.

What survived the review intact is worth saying first. The tap architecture
is right: chunking invariance holds, adapter state is bounded on conforming
streams, the canonical event model normalizes three wire formats, and the
build-on-top bet on Pingora was not challenged by a single finding. What did
not survive is the assumption hiding inside Q2's "stream, always": that the
streaming happy path *is* the protocol. HTTP has a response head, a content
coding layer, a connection lifecycle, and a retry vocabulary — and the
current design ignores all four. Almost every fix below consists of using
something HTTP already handed us. That is the meta-lesson: the philosophy
was under-applied, not wrong.

Four decisions follow. Each names the miss, the recommended default, the
alternative, and what must be published either way. The full ledger of
findings is at the end so nobody has to wonder what else the review found.

## Decision 1 — dispatch on the response head; meter the non-streaming half

**The miss.** Nothing in the design decides stream-vs-JSON per response, and
non-streaming responses are unmetered by omission. HTTP itself never marks a
response as a stream; the only reliable discriminator is the response head —
status plus Content-Type (`text/event-stream`,
`application/vnd.amazon.eventstream`, `application/json`). The request's
`"stream": true` is a hint, not a contract: every provider answers 4xx
errors as `application/json` even when the request asked for a stream.
Today the tap feeds *every* response body to a streaming adapter
unconditionally.

**Why it breaks the thesis.** Three ways. A `stream: false` request produces
zero events, zero usage, and a near-zero cap charge — any caller escapes
GB-5 and the ledger entirely by not streaming, and real traffic is heavily
non-streaming (embeddings, batch and eval jobs, tool-calling loops, SDK
defaults). The provider's usage object in a JSON body — the easiest
authoritative number of all — is never read. And a JSON error response on a
streaming route lands in the SSE parser, which never sees a block
terminator and silently buffers the whole body, falsifying its own
bounded-state claim.

**The fix.** In the response-header filter, pick the tap from the head the
proxy already parsed: `text/event-stream` → SSE adapter,
`vnd.amazon.eventstream` → frame decoder, `application/json` → a bounded
terminal parse that extracts usage/model/id at end-of-stream and emits the
same canonical `MessageStart / UsageDelta / MessageEnd`, anything else →
passthrough with metering explicitly marked absent. Q2's no-buffering rule
is about streams; a Content-Length-bounded JSON body held to end-of-stream
is not a violation, it is the one place a whole body is legitimately held.

**The decision.** Adopt is not in question — the open choice is the buffer
bound for the JSON terminal parse and the posture when it is exceeded
(meter-degraded and logged, never silently zero).

**Publish.** Doc 02 gains the non-streaming path as a first-class half of
Q2's answer, with the buffer bound stated next to the metering error bound.

**Status: implemented.** `upstream_response_filter` dispatches per response
via `decide_tap` (pure, unit-tested table); `application/json` swaps in the
bounded terminal-parse tap (`JsonBodyTap`, all four dialects' usage field
names, including the Vertex `streamGenerateContent` JSON-array shape and the
input-only embeddings shape). The stated bound is **256 MiB** — sized to
hold the largest legal non-streaming body (a max-batch float embeddings
response reaches ~130 MB) whole, worst-case tap memory a boring
concurrency × cap — and past it the tap degrades loudly (`json_body_overflow`
Error event that stamps `meter_degraded` on the span), never a silent zero.
The billed number is the authoritative **input + output** total (an
embeddings body has input and no output), charged through the SAME
enforcement `meter` path a stream uses — so GB-6 alerts fire, the cut bound
is checked, and partition is honored on non-streaming traffic, closing the
`stream:false` bypass end to end. A 1xx informational head (100 Continue,
103 Early Hints) is skipped, not treated as an unmetered head — without that
guard it would drop the streaming adapter and the following 2xx would meter
zero. A parsed 2xx JSON body on a capped route that carries no authoritative
usage stamps the span; non-2xx responses are the unmetered error path;
unrecognized heads pass through with metering stated absent.

These edges (the 1xx head, the input-only bill, the Vertex array shape, the
loudness contract on every `JsonBodyTap` failure path, and the GB-6/partition
enforcement routing) were each found by an adversarial review of the first
implementation and fixed before commit — the review's provenance is the same
as this document's. A second review of the fix surfaced and closed five
follow-ons: the span now records the billed input+output total (not
output-only) so span-vs-ledger reconciliation agrees; the token sum
saturates against a hostile upstream; a provider error in a 2xx envelope is
distinguished from a gateway metering hole (only the latter stamps
`meter_degraded`); unmeterable content types (audio, text) are stated
`NotApplicable` at info level rather than flooding the degradation alert; and
the D2 exception is narrowed to a caller `AWS4-HMAC-SHA256` signature (a
caller Bedrock API key signs nothing, so its `Accept-Encoding` is safely
stripped and metered).

**Known gap — closed by D4 (3 August 2026).** A JSON-terminal response
whose connection drops before the body completes now settles through the
always-runs path with `disposition=client-abort` (or `upstream-cut`) and a
loud span stamp on capped routes. The billed number for a truncated JSON
body remains zero — the tokens are unknowable from half a document — but it
is now a stamped, queryable population instead of a skipped accounting
path. See D4's published invariants below.

The stated bound for the terminal parse is **256 MiB** — sized to hold the
largest legal non-streaming body (a max-batch float embeddings response
reaches ~130 MB) whole, since the usage object cannot be reached without the
whole body. A body past even that degrades loudly (`json_body_overflow`,
span-stamped), never a silent zero.

## Decision 2 — content coding: don't offer what the tap can't see through

**The miss.** Neither the docs nor the code mention `Accept-Encoding` or
`Content-Encoding` anywhere, yet the entire tap — adapters, meter, GB-5
mid-stream enforcement, the GB-4 cut — assumes plaintext bytes. Official
SDKs negotiate compression unprompted (httpx: `gzip, deflate`; fetch:
`gzip, deflate, br`). Any upstream or fronting CDN that honors the offer
feeds the adapters compressed bytes: zero events, zero usage, traffic
flowing fine — the failure is silent, and a client can *induce* it with a
header. A mid-stream cut then splices a plaintext terminal event into a
coded stream and corrupts it. The request side has the mirror image: a
`Content-Encoding: gzip` request body flows undecoded into the model gate
and the injection merge, producing wrong rejections for well-formed
requests.

**The decision.** Overwrite `Accept-Encoding` to `identity` on the upstream
leg of every metered route. Content negotiation is hop-scoped by design —
the provider still chooses within the offer, so dialect fidelity holds; the
gateway invents nothing, it just declines to offer what it cannot read
through. This is the one deliberate, documented exception to header
pass-through (the nginx `proxy_set_header Accept-Encoding ""` move). The
alternative — decompress-for-tap while passing coded bytes through — is
heavier and only worth reaching for if lost transfer compression on large
non-streaming JSON measurably matters.

**The exception, stated honestly.** SigV4 pass-through routes, where a
header inside the caller's `SignedHeaders` cannot be mutated: there, either
inflate inside the tap (client bytes stay untouched) or publish the
metering blind spot as a stated edge.

**Defense in depth.** A response filter checks `Content-Encoding` anyway:
if it is ever non-identity, do not feed the adapter — log meter-degraded
loudly instead of billing zero silently. Request side: a coded request body
currently fails closed by accident; make that a stated behavior.

**Status: implemented.** `upstream_request_filter` overwrites
`Accept-Encoding: identity` on every routed request except genuine
caller-signed Bedrock pass-through — a Bedrock route where the gateway
supplies **no** credential of its own (no STS chain, no minted bearer, no
operator-injected `Authorization`), so a caller-computed SigV4 signature that
may cover the header must reach AWS unmodified. Keying on the presence of a
gateway credential rather than on `sts.is_none()` is deliberate: a bearer-key
Bedrock route is gateway-controlled, not caller-signed, so its
`Accept-Encoding` is safely stripped (a hole the review found in the first
cut). Anywhere a provider ignores the negotiation, the response-head dispatch
drops the tap, logs UNMETERED, and stamps `meter_degraded` on the span. Coded
bytes never reach a dialect parser.

## Decision 3 — the OpenAI usage frame is opt-in: inject, or publish estimate-only (DECIDED: never inject)

**The miss.** On the chat-completions dialect the terminal usage frame
exists only if the caller sent `stream_options: {"include_usage": true}`.
The dominant client path — default `openai-python`/`node` streaming, most
agent frameworks, older pinned SDKs — does not send it. For exactly the
most common dialect, the authoritative number never arrives and the ledger
settles on the chars/4 estimate (±50% measured, per the spike's own
numbers). No doc acknowledges this. It is the one genuine collision between
the billing thesis and the proxy-not-a-format philosophy: they cannot both
hold on this dialect, and holding neither in writing is the only wrong
answer.

**The decision: REJECTED, on purpose (3 August 2026).** The gateway does
NOT inject `include_usage`. Pass-through purity wins: the caller's request
body crosses this proxy byte-identical, always — the gateway never rewrites
a request to harvest a number, not even the number its own thesis runs on.
The industry went the other way (agentgateway, Envoy AI Gateway, and
Bifrost all force `stream_options.include_usage` today), which makes the
refusal a distinguishing stance rather than a lag: theirs is a defensible
engineering choice; ours is the stricter trust posture, and it is now a
brand property. What made the alternative honest is what makes this one
honest — the invariant is PUBLISHED, not hidden:

> **Stated invariant.** An OpenAI-dialect stream whose caller did not send
> `stream_options.include_usage` settles at the estimate, error bound ±50%
> as measured on real transcripts (spike A). The meter record and the span
> carry `disposition=completed-no-frame`, so estimate-settled traffic is a
> query, not a surprise. Callers who want the authoritative number send the
> flag; the gateway will never send it for them.

Operators who need authoritative numbers on this dialect have two clean
paths that keep the purity intact: configure clients/SDK defaults to send
the flag (a one-line SDK setting), or route through a provider whose
dialect always ships the terminal frame. The rejection is revisitable like
any decision in this repo — by a dated entry here, never silently.

**The sibling gap.** The OpenAI Responses API (`/v1/responses`) is a
distinct dialect — semantic `response.*` events, usage nested inside
`response.completed`, no `[DONE]` sentinel — and it is increasingly the
default for real OpenAI clients. Fed to the chat-completions adapter it
meters approximately zero. Two moves, both on-philosophy: a
ResponsesAdapter (greenfield adapters are the declared product, doc 01 Q5),
selected by path within the openai kind; and a stated fail-closed posture
for unrecognized dialects on metered routes — a stream that ends with zero
`UsageDelta` and zero `ContentDelta` is flagged, never silently billed as
zero. Silent under-metering is the worst failure mode this product has.

## Decision 4 — the lifecycle edges are the invoice

**The miss, in four parts.** (1) When the downstream client disconnects
mid-stream — every stop button, every SDK timeout — the end-of-stream
accounting path is skipped and no doc states what that stream's invoice is.
Q3's promise (estimate reconciled against the terminal frame) is silent on
the population of streams where the frame structurally cannot exist. (2)
The GB-5/GB-4 mid-stream cut suppresses chunks downstream while the
provider generates and bills to `max_tokens` — then settlement reconciles
the ledger *up* to the full authoritative count. The overspend bound is
real and published nowhere. (3) The cut renders its terminal event as SSE
on every dialect; on a Bedrock route that splices ASCII into CRC-checked
binary framing, and the client's AWS SDK gets a decode error instead of
the operator's message — the flagship streaming-rejection feature destroys
the stream on one of the three launch providers. (4) After a cut,
subsequent upstream chunks are dropped without being fed to the adapter, so
the provider's terminal usage frame is discarded precisely on the streams
where the provider billed to completion.

**The fix.** One enum, one relocation, one teardown, one dispatch. A
stream-disposition taxonomy — `completed+frame`, `completed-no-frame`,
`client-abort`, `upstream-cut`, `gateway-cut` — stamped on the meter record
and the OTel span. End-of-stream settlement moves to the path that runs on
*every* outcome (guarded to run exactly once) instead of living inside the
body filter's happy path. A gateway cut tears down the upstream connection
— the identical teardown a client abort already triggers — so the provider
stops generating at the cut; until that lands, the interim invariant is
published: overspend after a mid-stream cut ≤ the request's remaining
`max_tokens`, optionally shrunk by clamping `max_tokens` at admission with
the bounded JSON rewrite that already exists. And the terminal event
renders per dialect: SSE block for OpenAI/Anthropic/Vertex; for Bedrock,
one event-stream frame through the existing encoder — with a per-dialect
cut fixture test that decodes the cut bytes with each dialect's own parser.

**Publish.** "An aborted stream is billed at the estimate; its error bound
is stated separately from the completed-stream bound." Platform teams trust
stated error bounds and distrust magic — this is Principle 3 applied to the
worst five minutes of the product's life.

**Status: implemented (3 August 2026).** All four parts landed:

- **The disposition taxonomy is live.** Every proxied request settles with
  one of `completed+frame / completed-no-frame / client-abort /
  upstream-cut / gateway-cut`, stamped on the `[meter]` line and the span
  (`gateway.stream.disposition`). The invoice disposition of every stream
  is a query.
- **Settlement runs exactly once, on every path.** The accounting block
  moved out of the body filter's happy path into a guarded settle that the
  `logging` hook (which pingora runs on every request outcome) fires
  whenever end-of-stream never did. A stop button, a dead upstream, or the
  gateway's own cut all produce a billing record.
- **A cut now stops the provider.** After the operator's terminal event is
  delivered, the next upstream chunk tears the session down; the upstream
  connection closes with it, so generation — and billing — stops at the cut
  instead of running to `max_tokens`. Stated residual: the teardown closes
  both sides rather than finishing the downstream encoding gracefully; the
  clean drop-upstream-only variant still awaits the pingora change named in
  the spike README, filed upstream as cloudflare/pingora#951 (6 Aug 2026). A final chunk that arrives together with end-of-stream
  after a cut is fed to the METER (the usage frame lands if it aligned) and
  suppressed from the client.
- **The terminal event speaks the dialect's framing.** SSE block on SSE
  dialects; on Bedrock, one CRC-framed event-stream EXCEPTION frame
  (`:exception-type` = the operator's event name, default `stream_cut`) —
  decodable by the same parser that reads the provider's frames, surfaced
  by AWS SDKs as a typed error carrying the operator's payload. Per-dialect
  cut tests decode the cut bytes with each dialect's own parser.

**Published invariants (D4).** An aborted or cut stream bills at what was
metered before the end — the estimate for streams (its ±50% bound stated
above), zero for a non-streaming body that never completed (the tokens are
unknowable from a truncated JSON document; the disposition stamp makes the
population visible). A gateway cut's provider-side overspend is bounded by
the chunk in flight at teardown, no longer by `max_tokens`.

## The ledger

Everything the review confirmed, compressed. "Lands in" names the decision
above or the standalone fix. Severity is the review's, kept honest.

| Finding | Sev | Lands in |
|---|---|---|
| No response-head dispatch; non-streaming traffic unmetered; `stream:false` bypasses GB-5 | critical | D1 |
| Content-coding blindness end to end; SDKs negotiate gzip by default | critical | D2 |
| OpenAI usage frame opt-in; thesis degrades to estimate-only for default clients | critical | D3 |
| Client abort: invoice disposition undefined, settlement never runs | critical | D4 |
| Mid-stream cut never cancels upstream; overspend bound unpublished | high | D4 |
| GB-4 terminal event rendered as SSE corrupts Bedrock binary streams | critical | D4 |
| Cut path discards the provider's terminal usage frame; last chunk leaks after the terminal event | high | D4 |
| OpenAI Responses API is an unreadable dialect; meters ~zero; unmentioned anywhere | high | D3 |
| No timeout taxonomy; Pingora peer defaults are all `None`; 504 structurally impossible; dead upstream hangs clients | high | four numbers per provider route (connect / first-byte / inter-chunk / total:none-stated), set on the peer, published |
| Pingora's inherited retry loop re-sends non-idempotent POSTs up to 16× on reused-connection failures, unlogged | high | explicit `max_retries`, retry connect-phase only, every retry logged with attribution tags |
| Host forwarded caller-verbatim, route prefix never stripped — documented setups 404 against real providers | high | request re-origination: strip matched prefix, rewrite Host per provider kind |
| Shape-1 Bedrock pass-through is protocol-contradictory (SigV4 binds Host/path the gateway forwards wrongly) | high | re-scope the recipe in doc 10; signature verification cannot survive re-origination |
| Bedrock `:message-type: exception` frames silently dropped; failed streams look clean; decode errors blind the tap permanently with no meter-loss policy | high | adapter keys on message-type; decode error → meter-degraded, stated |
| Mid-stream upstream failure after 200: no per-dialect terminal answer; OpenAI/Bedrock adapters cannot observe their provider's error frames | high | per-dialect error frames + adapter error visibility |
| Input tokens parsed from the authoritative frame, then dropped — caps and spans count output only, unstated | high | carry `authoritative_input` through cap charge and span; state the unit |
| Multipart endpoints (audio et al.) hard-rejected by the JSON body gate — after buffering the whole upload | high | content-type-aware model gate; multipart `model` field |
| Unbounded request-body buffering, no published limit, no 413 semantics | high | stated cap + 413 in the operator's dialect |
| Compressed request bodies misparse into wrong rejections | medium | D2 (request side) |
| Hop-by-hop header hygiene absent; smuggling/duplicate-header posture unstated | medium | strip the RFC 9110 hop-by-hop set on the upstream leg; state what Pingora already handles |
| Verified fleet JWT forwarded verbatim to third-party providers where no minted credential overwrites it | medium | strip `Authorization` after verification unless the route re-credentials |
| GB-4/GB-5 429s cannot carry `Retry-After`; default SDKs auto-retry every cap refusal ~3× | medium | headers surface on `RejectionTemplate`; emit the known window reset |
| Stale-keepalive 502 race; reuse posture unstated; hits large-context requests specifically | medium | idle-expiry on pooled connections, posture published |
| Process-level drain rests on Pingora's unconfigured 300 s grace; long streams die mid-SSE at deploys with no terminal event | medium | chosen grace period, stated in doc 03 alongside config-swap drain |
| Gateway infra failures are bare bodiless 502s in no dialect | medium | GB-4's own rationale applied to gateway-originated errors |
| Aborted streams: "estimate stands" fallback only runs on the happy-path teardown | medium | D4 (settlement relocation) |
| SSE parser deletes all CR bytes; spec-legal CR-terminated streams buffer unboundedly; BOM unhandled | medium | parser conformance + a hard cap on pending state |
| SSE parser's bounded-state claim unenforced (no cap on the pending buffer) | low | same cap; degrade loudly past it |
| No-coalescing/flush claim proven only on loopback h1 at 80 ms cadence | low | restate as a latency invariant, or measure against a real provider |
| WebSocket dialects (OpenAI Realtime, Gemini Live) invisible to a request/response proxy; scope decision stated nowhere | low | one row in doc 05 with a verdict, per its own integrity rule |

## What this changes in the build plan

Phase 1's acceptance tests grow four cells: a `stream:false` request that
meters, a gzip-negotiating client that meters, a default-SDK OpenAI stream
that settles on an authoritative number, and a Bedrock stream cut mid-flight
whose client receives a decodable terminal frame. None of these are
features. They are what "sits so tightly on HTTP it disappears" turns out
to mean once you list what HTTP actually contains.
