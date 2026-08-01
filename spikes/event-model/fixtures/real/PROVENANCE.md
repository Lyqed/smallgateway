# Provenance — real recorded provider transcripts

Every fixture in this directory is a genuinely machine-recorded provider
streaming response, harvested from public OSS test suites on 2026-08-01 (no
API credentials were available, so nothing here was captured live by us).
The acceptance bar: the transcript must be plausibly machine-recorded
(realistic ids, timestamps, fingerprints, delta patterns) **and** must carry
the provider's authoritative usage frame recorded with it. Hand-authored
mocks were excluded.

Commits listed are the most recent commit touching the file at fetch time;
raw content was fetched from `raw.githubusercontent.com` at the named branch
and matches that commit.

## Extraction / reconstruction (applies per format)

- **`.sse` files (OpenAI, Anthropic)** — the source VCR cassettes
  (vcrpy format) store the decoded SSE response body as a YAML string under
  `interactions[0].response.body.string`; the OTel Java source is a WireMock
  stub mapping storing it under `response.body`. Extraction was a YAML parse
  (PyYAML) followed by writing the body string verbatim — the SSE framing
  (`event:`/`data:` lines, blank-line separators) is exactly what the
  recorder captured off the wire; no re-framing was performed. The DataDog
  test-agent cassettes contain Python-specific YAML tags
  (`!!python/object/new:multidict._multidict.istr`) on *request header* keys
  only; a tag-tolerant loader was used, and the response body is unaffected.
- **`.jsonl` files (Bedrock)** — the source cassettes store the **raw binary
  AWS event-stream response body** (base64 inside YAML). It was decoded with
  a purpose-written decoder that validated both the prelude CRC32 and the
  message CRC32 of **every frame** (all passed), confirmed each frame carries
  exactly the three standard headers (`:event-type`, `:content-type`,
  `:message-type`; no extras were found), and spliced each payload's JSON
  **verbatim** into the spike's JSONL convention
  (`{"event":...,"payload":...}` per line) — including AWS's random-length
  `"p"` padding fields. On replay the CLI re-encodes these into event-stream
  frames with the same three headers; CRCs are recomputed and `serde_json`
  may normalize payload key order, neither of which affects delta text or
  token counts.

## Why these were judged genuinely recorded

- **OpenAI**: realistic `chatcmpl-…` ids and `created` epochs that match the
  recorded `Date` response headers; `service_tier`, `system_fingerprint`,
  and full `prompt_tokens_details` / `completion_tokens_details` objects;
  Cloudflare `CF-RAY` ids and rate-limit headers with non-round remaining
  values (e.g. `3999982`); one transcript shows genuine prompt caching
  (`cached_tokens: 1280` of a 1420-token prompt).
- **Anthropic**: every transcript exhibits Anthropic's anti-buffering
  artifact — random-length whitespace padding *inside* the JSON payload
  before the closing brace (e.g. `"index":0        }`) — which hand-authored
  mocks never reproduce; realistic `msg_…` ids; `ping` events; recorded
  `request-id`, `anthropic-ratelimit-*` and `CF-RAY` headers;
  `cache_creation`, `service_tier`, `inference_geo` fields consistent with
  the API version at the recording date.
- **Bedrock**: raw binary event-stream framing with valid CRC32s on every
  frame is not hand-authorable in practice; every payload carries AWS's
  `"p"` padding field with random length; realistic `latencyMs` metrics;
  requests target real `bedrock-runtime.us-east-1.amazonaws.com` model
  endpoints.

## OpenAI (6 transcripts)

| fixture | source | commit | license |
|---|---|---|---|
| `openai/otel-java-gpt-4o-mini-include-usage.sse` | [open-telemetry/opentelemetry-java-instrumentation](https://github.com/open-telemetry/opentelemetry-java-instrumentation) `instrumentation/openai/openai-java-1.1/testing/src/main/resources/mappings/io.opentelemetry.instrumentation.openai.v1_1.abstractchattest.streamincludeusage.yaml` | `6dd06e5` | Apache-2.0 |
| `openai/dd-apm-test-agent-gpt-4o-cached-193ae44a.sse` | [DataDog/dd-apm-test-agent](https://github.com/DataDog/dd-apm-test-agent) `vcr-cassettes/openai/openai_chat_completions_post_193ae44a.yaml` | `6d739bd` | Apache-2.0 OR BSD-3-Clause |
| `openai/dd-apm-test-agent-gpt-4o-66dfc80e.sse` | same repo, `…post_66dfc80e.yaml` | `6d739bd` | Apache-2.0 OR BSD-3-Clause |
| `openai/dd-apm-test-agent-gpt-3.5-d98ce00d.sse` | same repo, `…post_d98ce00d.yaml` | `0e2c71f` | Apache-2.0 OR BSD-3-Clause |
| `openai/dd-apm-test-agent-gpt-3.5-ae4728c2.sse` | same repo, `…post_ae4728c2.yaml` | `9349315` | Apache-2.0 OR BSD-3-Clause |
| `openai/dd-apm-test-agent-gpt-3.5-toolcalls-b29f1a87.sse` | same repo, `…post_b29f1a87.yaml` | `9349315` | Apache-2.0 OR BSD-3-Clause |

Recorded models: `gpt-4o-mini-2024-07-18`, `gpt-4o-2024-08-06`,
`gpt-3.5-turbo-0125`. All six were recorded with
`stream_options.include_usage: true`, so the terminal chunk carries the
authoritative `usage` object. `b29f1a87` is a streamed tool call
(`extract_student_info`), included deliberately to measure metering on
tool-argument streams.

## Anthropic (6 transcripts)

| fixture | source | commit | license |
|---|---|---|---|
| `anthropic/dd-apm-test-agent-claude-sonnet-4-595f439c.sse` | [DataDog/dd-apm-test-agent](https://github.com/DataDog/dd-apm-test-agent) `vcr-cassettes/anthropic/anthropic_v1_messages_post_595f439c.yaml` | `84a343e` | Apache-2.0 OR BSD-3-Clause |
| `anthropic/dd-apm-test-agent-claude-sonnet-4-a1af2c12.sse` | same repo, `…post_a1af2c12.yaml` | `84a343e` | Apache-2.0 OR BSD-3-Clause |
| `anthropic/dd-trace-py-claude-3-opus-stream.sse` | [DataDog/dd-trace-py](https://github.com/DataDog/dd-trace-py) `tests/contrib/anthropic/cassettes/anthropic_completion_stream.yaml` | `d7a5bd7` | Apache-2.0 OR BSD-3-Clause |
| `anthropic/ecologits-claude-haiku-4-5-stream.sse` | [mlco2/ecologits](https://github.com/mlco2/ecologits) `tests/cassettes/test_anthropic/test_anthropic_stream_chat.yaml` | `7047861` | MPL-2.0 |
| `anthropic/weave-claude-3-haiku-stream.sse` | [wandb/weave](https://github.com/wandb/weave) `tests/integrations/anthropic/cassettes/anthropic_test/test_anthropic_stream.yaml` | `ec516f4` | Apache-2.0 |
| `anthropic/logfire-claude-sonnet-4-stream.sse` | [pydantic/logfire](https://github.com/pydantic/logfire) `tests/otel_integrations/cassettes/test_anthropic/test_sync_messages_stream_version_latest.yaml` | `333a8a9` | MIT |

Recorded models: `claude-sonnet-4-20250514` (×3),
`claude-3-opus-20240229`, `claude-haiku-4-5-20251001`,
`claude-3-haiku-20240307`. Each carries `message_start` usage
(`input_tokens`) and the `message_delta` usage frame with cumulative
`output_tokens` — the authoritative count the meter reconciles against.

## Bedrock (5 transcripts)

| fixture | source | commit | license |
|---|---|---|---|
| `bedrock/dd-trace-py-claude-3-sonnet-toolcall.jsonl` | [DataDog/dd-trace-py](https://github.com/DataDog/dd-trace-py) `tests/contrib/botocore/bedrock_cassettes/bedrock_converse_stream.yaml` | `0e3e27d` | Apache-2.0 OR BSD-3-Clause |
| `bedrock/dd-trace-py-claude-3-7-sonnet-prompt-caching.jsonl` | same repo, `…/bedrock_converse_stream_prompt_caching.yaml` | `4258f23` | Apache-2.0 OR BSD-3-Clause |
| `bedrock/pydantic-ai-nova-micro-stream.jsonl` | [pydantic/pydantic-ai](https://github.com/pydantic/pydantic-ai) `tests/models/cassettes/test_bedrock/test_bedrock_model_stream.yaml` | `a932afd` | MIT |
| `bedrock/pydantic-ai-gpt-oss-reasoning.jsonl` | same repo, `…/test_bedrock_model_stream_empty_text_delta.yaml` | `c829449` | MIT |
| `bedrock/otel-python-contrib-titan-lite.jsonl` | [open-telemetry/opentelemetry-python-contrib](https://github.com/open-telemetry/opentelemetry-python-contrib) `instrumentation/opentelemetry-instrumentation-botocore/tests/cassettes/test_converse_stream_with_content.yaml` | `6b3a11b` | Apache-2.0 |

Recorded models: `anthropic.claude-3-sonnet`, `us.anthropic.claude-3-7-sonnet`
(1028 cache-write tokens), `us.amazon.nova-micro`, `openai.gpt-oss-120b`
(streams a `reasoningContent` block — those tokens are billed output, which
is why the adapter now normalizes them into `ContentDelta`), and
`amazon.titan-text-lite`. The authoritative usage arrives in the terminal
`metadata` frame.

## Candidates found and excluded

- **traceloop/openllmetry** `test_nova_converse_stream.yaml` and
  `test_titan_converse_stream.yaml` — genuinely recorded (valid CRCs), but
  both streams were intercepted/masked by Bedrock guardrails (empty or
  `{ADDRESS}`-masked deltas), so the char-stream vs token relationship is a
  guardrail artifact, not a metering measurement.
- **DataDog/dd-trace-py** `bedrock_cassettes/*_invoke_stream.yaml` — real
  recordings of the older InvokeModel wire format (`chunk` events with
  base64 model-specific payloads), which is not the ConverseStream protocol
  this spike normalizes.
- Numerous streaming cassettes recorded **without** a usage frame (OpenAI
  streams predating/omitting `stream_options.include_usage`, e.g.
  dd-apm-test-agent `172294b4`, OTel Java `…stream.yaml`) — real, but they
  cannot produce an error bound and do not count toward the measurement.
- Hand-authored mocks (round-number usage, lorem-ipsum deltas, no padding
  artifacts) were rejected on sight; when in doubt a transcript was
  excluded.
