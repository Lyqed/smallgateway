# Draft PR — GB-4

> Status: draft, not submitted. Branch `gb4-local-ratelimit-body`, one commit
> on `66713a6ec3bae2d597bc5639a39bb55da72e871a`. Check CLA/DCO requirements at
> submission time (see README.md).

## Title

```
localratelimit: support a custom rate limit exceeded response
```

## Body

When a local rate limit rejects a request today, the client always gets a
plain-text 429 with the error's Display string
(`ProxyError::into_response_with_grpc` falls through to the generic
`text/plain` terminal in `crates/agentgateway/src/proxy/mod.rs`). There is no
way for an operator to return a JSON body, a different content type, or a
different status code — which matters when the callers are SDKs that expect a
provider-shaped error payload.

The remote rate limiter already supports this: it builds its rejection
response from the rate limit service's `raw_body`
(`crates/agentgateway/src/http/remoteratelimit.rs`, `apply`). This PR gives
the local rate limiter the equivalent knob.

### What this does

- Adds an optional `response` field to the local rate limit policy:

  ```yaml
  policies:
    localRateLimit:
    - maxTokens: 10
      tokensPerFill: 1
      fillInterval: 60s
      response:
        body: '{"error":"rate limited"}'
        contentType: application/json
        status: 429   # optional, defaults to 429
  ```

- `ProxyError::RateLimitExceeded` carries the configured response (boxed and
  optional, following the `GuardrailRejected` pattern), and
  `into_response_with_grpc` honors it in the 429 path.
- Behavior is unchanged when `response` is unset: same plain-text 429, same
  `X-RateLimit-*` headers. The headers are also still attached when a custom
  response is configured.
- gRPC requests keep the trailers-only `grpc-status` response; the custom
  body only applies to HTTP clients.
- Not wired through XDS in this PR (the proto has no field for it);
  the XDS conversion passes `response: None`.
- `cargo xtask schema` output (`schema/config.json`, `schema/config.md`) is
  included.

### Tests

New `crates/agentgateway/src/http/localratelimit_tests.rs` (same `#[path]`
wiring as `remoteratelimit_tests.rs`) covering: unchanged default behavior,
custom body/content-type/status, defaulting when only `body` is set, the gRPC
path, the LLM token-limit path, config deserialization, and rejection of
unknown fields under `response`.

`cargo test -p agentgateway --lib` passes (1625 tests), `cargo clippy
--all-targets` and the `make lint` fmt check are clean.

---

Context: this change came out of gateway-baseline conformance checks for
operator-customizable rate limit responses (https://thegatewaybaseline.com).
