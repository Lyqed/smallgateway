# Spike A — the canonical event model

Phase 0 of [the build plan](../../docs/04-build-plan.md). Throwaway code with
non-throwaway conclusions: three provider wire formats normalize into the
canonical event stream, and streaming token metering reconciles against the
terminal usage frame.

## What is proven here

- **One event model over three wire formats.** OpenAI `chat.completion.chunk`
  SSE, the Anthropic Messages event protocol, and Bedrock ConverseStream
  (real AWS event-stream binary framing, CRC-checked) all normalize to
  `MessageStart / ContentDelta / ToolCallDelta / UsageDelta / MessageEnd /
  Error` — for text and for streamed tool calls.
- **A stronger ordering contract than any provider gives natively.** The
  terminal usage frame always precedes `MessageEnd`, and `MessageEnd` is
  always terminal. OpenAI ships usage *after* finish_reason; Bedrock ships it
  after `messageStop`; the adapters hold `MessageEnd` until the stream truly
  ends. Policies get one contract, not three.
- **Chunking invariance.** Replaying any fixture at chunk sizes 1, 7, 64, or
  whole-buffer produces byte-identical event streams. Parsers hold only the
  current partial frame (`pending_bytes()` is bounded and returns to 0), which
  is what makes backpressure trivial: feed only after consuming.
- **Metering reconciliation.** A live chars/4 estimate accumulates per delta
  (this is what mid-stream enforcement meters against), and the report shows
  it against the provider's authoritative count with a signed error
  percentage.
- **A measured metering error bound from real transcripts.** No API
  credentials existed on this machine, so instead of live captures the bound
  is measured from genuinely machine-recorded streaming transcripts harvested
  from public OSS test suites (VCR cassettes and WireMock stubs that recorded
  real provider responses, usage frames included). Provenance and the
  authenticity bar for every transcript:
  [fixtures/real/PROVENANCE.md](fixtures/real/PROVENANCE.md).

## Measured metering error (chars/4 estimate vs provider usage frame)

Signed error = (estimated − authoritative) / authoritative, on output tokens.

| provider | n | per-transcript error | min | max | mean |
|---|---|---|---|---|---|
| OpenAI | 6 | +34.3%, +0.0%, −30.8%, +21.0%, +33.0%, +66.7% | −30.8% | +66.7% | +20.7% |
| Anthropic | 6 | +10.0%, +24.0%, +0.0%, −34.8%, −22.2%, −23.1% | −34.8% | +24.0% | −7.7% |
| Bedrock | 5 | +20.3%, −48.4%, −20.0%, −11.6%, +14.6% | −48.4% | +20.3% | −9.0% |

Per-transcript detail (estimated / authoritative output tokens):

| fixture | est | auth | error |
|---|---|---|---|
| openai/dd-apm-test-agent-gpt-3.5-ae4728c2 | 47 | 35 | +34.3% |
| openai/dd-apm-test-agent-gpt-3.5-d98ce00d | 9 | 9 | +0.0% |
| openai/dd-apm-test-agent-gpt-3.5-toolcalls-b29f1a87 | 18 | 26 | −30.8% |
| openai/dd-apm-test-agent-gpt-4o-66dfc80e | 121 | 100 | +21.0% |
| openai/dd-apm-test-agent-gpt-4o-cached-193ae44a | 133 | 100 | +33.0% |
| openai/otel-java-gpt-4o-mini-include-usage | 5 | 3 | +66.7% |
| anthropic/dd-apm-test-agent-claude-sonnet-4-595f439c | 110 | 100 | +10.0% |
| anthropic/dd-apm-test-agent-claude-sonnet-4-a1af2c12 | 124 | 100 | +24.0% |
| anthropic/dd-trace-py-claude-3-opus-stream | 15 | 15 | +0.0% |
| anthropic/ecologits-claude-haiku-4-5-stream | 15 | 23 | −34.8% |
| anthropic/logfire-claude-sonnet-4-stream | 7 | 9 | −22.2% |
| anthropic/weave-claude-3-haiku-stream | 10 | 13 | −23.1% |
| bedrock/dd-trace-py-claude-3-7-sonnet-prompt-caching | 284 | 236 | +20.3% |
| bedrock/dd-trace-py-claude-3-sonnet-toolcall | 33 | 64 | −48.4% |
| bedrock/otel-python-contrib-titan-lite | 8 | 10 | −20.0% |
| bedrock/pydantic-ai-gpt-oss-reasoning | 38 | 43 | −11.6% |
| bedrock/pydantic-ai-nova-micro-stream | 94 | 82 | +14.6% |

What the numbers say: the deliberately crude chars/4 heuristic lands within
±50% on all but one real stream (the +66.7% outlier is a 3-token response,
where 2 tokens of absolute miss is 66%), and its worst misses are
structural, not random — **tool-call streams undercount** (the tool name and
call scaffolding are billed but never streamed as text: −30.8% OpenAI,
−48.4% Bedrock), tiny responses have huge relative error on a few tokens'
absolute miss, and emoji/multibyte content undercounts (−34.8% is a
wave-emoji greeting). For mid-stream budget enforcement this is workable —
the estimate is a tripwire, the usage frame is the invoice — but a
tokenizer-aware estimator would tighten the bound considerably.

### Honest caveats on the measurement

- **The estimator is chars/4 by design.** The spike measures how crude that
  is; it does not claim it is good.
- **Sample size is small** (17 transcripts, output lengths 3–236 tokens) and
  skewed toward short test-suite responses; long-generation behavior (where
  relative error should shrink) is under-represented.
- **Provenance is second-hand.** These are recordings committed to OSS test
  suites, vetted for machine-recorded artifacts (CRCs, padding fields,
  request ids — see PROVENANCE.md), not captures we performed ourselves.
- **Reasoning/thinking streams are only lightly covered** (one gpt-oss
  Bedrock transcript). Recorded Anthropic extended-thinking streams exist in
  the wild, but the adapter does not yet normalize `thinking_delta`, so they
  were left out rather than half-measured.
- No provider ended up unmeasured, but Bedrock recordings are genuinely
  scarce: binary event-stream bodies survive only in cassettes that store
  raw bytes, and two of the recordings found had guardrail-masked content
  and were excluded (see PROVENANCE.md).

## Out of scope

Per the spike definition: the async `Stream` wrapper (mechanical over the
push-parser core), provider request translation, and anything resembling a
proxy.

## Run it

```bash
cargo test
cargo run -- --provider openai    --file fixtures/openai.sse
cargo run -- --provider anthropic --file fixtures/anthropic.sse
cargo run -- --provider bedrock   --file fixtures/bedrock.jsonl
```

`--chunk-size N` (default 17, deliberately odd) controls replay chunking so
frame boundaries never align with feed boundaries. The Bedrock fixture is
JSONL for reviewability; the CLI and tests encode it into real event-stream
frames in memory and run it through the full binary decoder, CRCs included.

The real recorded transcripts live under `fixtures/real/<provider>/` and
replay the same way, e.g.:

```bash
cargo run -- --provider anthropic --file fixtures/real/anthropic/logfire-claude-sonnet-4-stream.sse
cargo run -- --provider bedrock   --file fixtures/real/bedrock/pydantic-ai-nova-micro-stream.jsonl
```

The conformance suite replays the whole real corpus and asserts the usage
frame + ordering contract and chunking invariance on every transcript.
