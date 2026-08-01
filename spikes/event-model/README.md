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

## What is deliberately not proven here

The **measured error bound** — the spike's real exit criterion — needs replays
of real transcripts, not authored fixtures: the fixtures' token counts were
written by hand, so the ~+11% they show is plumbing verification, not a
measurement. Next step: capture live transcripts from the three providers and
publish the observed bound per provider. (Needs API credentials; per the
principles, nothing gets bought — existing keys or a teammate's captures are
enough.)

Also out of scope, per the spike definition: the async `Stream` wrapper
(mechanical over the push-parser core), provider request translation, and
anything resembling a proxy.

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
