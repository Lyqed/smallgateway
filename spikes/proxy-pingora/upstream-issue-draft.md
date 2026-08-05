# Upstream issue draft: cloudflare/pingora

*Draft for filing, written 6 August 2026 against pingora-proxy 0.8.1. This
is the change docs/11 (D4) and this spike's README refer to as "the pingora
change". File as a feature-request issue first; offer the implementation
once the API shape has maintainer agreement. Nothing below mentions our
product or LLMs: the framing is deliberately generic proxy behavior.*

---

**Title: Allow `response_body_filter` to end the downstream response cleanly after the current chunk**

## Problem

`ProxyHttp::response_body_filter` receives `end_of_stream: bool` as an
input and can rewrite the chunk bytes, but it has no way to declare the
response finished. In `proxy_h1.rs` and `proxy_h2.rs`, the task pipeline
maps `HttpTask::Body(data, end)` through the filter and re-emits
`HttpTask::Body(data, end)` with the original `end` flag, so a filter that
decides mid-stream that the response must stop has only two options:

1. Return `Err(...)`. The session is torn down: the client sees a
   truncated chunked body or an H2 reset, not a well-formed end of
   response, and the downstream connection is lost.
2. Replace the current chunk with a final message and swallow every
   subsequent chunk (`*body = None`). The client eventually sees a clean
   end, but the upstream transfer runs to completion, which can be
   arbitrarily long and, for metered upstreams, costly.

Neither produces the behavior a policy-enforcing proxy needs: deliver one
final application-level message (for example a terminating SSE event that
says why), finish the downstream encoding correctly, and stop consuming
the upstream.

This shows up in any filter that makes mid-stream decisions: content
policy and DLP filters that must stop a response on a match, quota and
metering enforcement that cuts at a limit, or timeout policies that end a
stalled stream with an explanatory final chunk instead of a reset.

## Proposed API

A method on `Session` (name entirely up to you):

```rust
/// After the current `response_body_filter` call returns, treat the
/// filtered chunk as the last chunk of the response: finish the
/// downstream body encoding cleanly and stop reading from upstream.
pub fn finish_response_body_after_current_chunk(&mut self) -> Result<()>;
```

Semantics:

- The proxy loop checks the flag after each `response_body_filter`
  return. When set, the current task is forwarded as
  `HttpTask::Body(data, true)` (or `Body` followed by `Done`), so the
  downstream write path runs its normal end-of-body logic: final chunk
  plus `0\r\n\r\n` terminator on H1 chunked, `END_STREAM` on H2. The
  downstream connection stays reusable.
- The upstream read loop stops. On H1 the upstream connection is closed
  (mid-body, it is not reusable anyway); on H2 the stream is reset. This
  is the point of the feature for metered upstreams: the transfer stops
  at the cut.
- Fixed Content-Length responses cannot be ended early without violating
  the framing, so the method returns an error when the downstream
  response is neither chunked nor H2 (the response header has already
  been sent by filter time, so eligibility is known). The filter author
  chooses how to handle that; nothing changes silently.
- Expected trailers are simply not sent; the response ended at the body.
- Calling it when `end_of_stream` is already true is a no-op.

## Alternatives considered

- Changing the filter signature to `end_of_stream: &mut bool` expresses
  the same thing more directly but breaks every existing `ProxyHttp`
  implementation. The session-method shape is purely additive.
- Doing this outside pingora-proxy does not work: the `end` flag is
  decided before the filter runs, and the upstream read loop is not
  reachable from filter code.

## Offer

Happy to implement this behind whatever API shape you prefer, with tests
for the H1 chunked, H2, and Content-Length-refusal paths, if the design
direction is acceptable.

---

*Filing notes (not part of the issue): CLA required. If review stalls,
the interim is a `[patch.crates-io]` overlay of a branch carrying exactly
this change, referenced from docs/11 D4 by PR number; the current
teardown-on-next-chunk behavior remains the stated fallback. When filed,
record the issue number in docs/11 D4 and in the spike README, and add
the ref to the ledger with expectedState open.*
