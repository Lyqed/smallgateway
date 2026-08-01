# Draft PR — GB-4

> Status: ready to submit. Branch `gb4-local-ratelimit-body`, one signed-off
> commit `bb5d1772907db0568ed3966b6ebd6fb0ff6a85f3` on
> `66713a6ec3bae2d597bc5639a39bb55da72e871a`. Upstream runs the DCO check on
> every PR (verified on #2796's check runs); the commit carries
> `Signed-off-by: Lyqed <antondoeswonders@gmail.com>` matching the author.

## Title

```
localratelimit: support a custom rate limit exceeded response
```

## Body

When a local rate limit rejects a request, the client always gets a
plain-text 429 with the error's Display string. There is no way to return a
JSON body, a different content type, or a different status code — which
matters when the callers are SDKs that expect a provider-shaped error
payload. The remote rate limiter already supports this: it builds its
rejection response from the rate limit service's `raw_body`
(`remoteratelimit.rs`). This gives the local rate limiter the equivalent
knob.

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

The new `response` field follows the `filters::DirectResponse` field shapes
(`body`/`status` serde helpers), and `ProxyError::RateLimitExceeded` carries
it boxed like `GuardrailRejected`.

Behavior is unchanged when `response` is unset. The `X-RateLimit-*` headers
are still attached when a custom response is configured, and gRPC requests
keep the trailers-only `grpc-status` response. Not wired through XDS in this
PR (the proto has no field for it); the XDS conversion passes `response:
None`.

Tests cover the unchanged default, custom body/content-type/status,
defaulting, the gRPC path, the LLM token-limit path, and config
deserialization (including rejection of unknown fields under `response`).

## Verification (re-run on the refined commit)

```
cargo test -p agentgateway --lib http::localratelimit   # ok. 20 passed
cargo test -p agentgateway --lib                        # ok. 1625 passed; 0 failed; 1 ignored
cargo xtask schema                                      # regenerated, diff committed
cargo fmt --check -- --config imports_granularity=Module,group_imports=StdExternalCrate,normalize_comments=true
                                                        # clean (what upstream `make lint` runs)
git apply --check gb4-local-ratelimit-body/0001-*.patch # clean against pristine 66713a6e
```

## Style calibration notes (not part of the PR body)

- Title matches the dominant merged-commit convention: `module: lowercase
  imperative description` (howardjohn's own PRs #2793, #2776, #2772, #2733;
  externals that merge fastest use the same shape, e.g. #2740, #2705).
- Body kept short and motivation-first, like his #2772/#2754/#2728 bodies;
  the earlier draft's external project link was dropped as noise.
- The change argues from in-tree precedent (remoteratelimit `raw_body`,
  `DirectResponse` field idioms, `GuardrailRejected` boxing) because his
  reviews consistently push PRs toward existing patterns (#2515, #2781) and
  he asks for rationale on partial config surfaces (#2781) — hence the
  explicit XDS note.
