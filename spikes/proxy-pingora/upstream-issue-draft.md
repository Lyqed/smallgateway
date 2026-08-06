# Upstream issue draft: cloudflare/pingora

*Draft for filing, shaped to Pingora's feature-request template, written
6 August 2026 against pingora-proxy 0.8.1. This is the change docs/11 (D4)
and this spike's README refer to as "the pingora change". Nothing below
mentions our product or LLMs: the framing is deliberately generic proxy
behavior.*

---

**Title: Allow `response_body_filter` to end the downstream response cleanly after the current chunk**

## What is the problem your feature solves, or the need it fulfills?

`ProxyHttp::response_body_filter` receives `end_of_stream: bool` as an
input and can rewrite the chunk bytes, but it has no way to declare the
response finished. In `proxy_h1.rs` and `proxy_h2.rs`, the task pipeline
maps `HttpTask::Body(data, end)` through the filter and re-emits
`HttpTask::Body(data, end)` with the original `end` flag.

So a filter that decides mid-stream that a response must stop has only
two options, and both are wrong on the wire:

1. Return `Err(...)`. The session is torn down: the client sees a
   truncated chunked body or an H2 reset, not a well-formed end of
   response, and the downstream connection is lost.
2. Replace the current chunk with a final message and swallow every
   subsequent chunk (`*body = None`). The client eventually sees a clean
   end, but the upstream transfer runs to completion, which can be
   arbitrarily long and, for metered upstreams, costly.

Who this is for: any proxy that enforces policy on streaming responses
and needs to end one mid-stream with a final well-formed message rather
than a reset. Content-policy and DLP filters that must stop a response on
a match, quota and metering enforcement that cuts at a limit, and timeout
policies that end a stalled stream with an explanatory final chunk (for
example a terminating SSE event that says why) all hit this today.

## Describe the solution you'd like

A method on `Session` (name entirely up to you):

```rust
/// After the current `response_body_filter` call returns, treat the
/// filtered chunk as the last chunk of the response: finish the
/// downstream body encoding cleanly and stop reading from upstream.
pub fn finish_response_body_after_current_chunk(&mut self) -> Result<()>;
```

How it would work:

- The proxy loop checks the flag after each `response_body_filter`
  return. When set, the current task is forwarded as
  `HttpTask::Body(data, true)` (or `Body` followed by `Done`), so the
  downstream write path runs its normal end-of-body logic: final chunk
  plus `0\r\n\r\n` terminator on H1 chunked, `END_STREAM` on H2. The
  downstream connection stays reusable.
- The upstream read loop stops. On H1 the upstream connection is closed
  (mid-body, it is not reusable anyway); on H2 the stream is reset. For
  metered upstreams this is the point: the transfer stops at the cut.
- Fixed Content-Length responses cannot be ended early without violating
  the framing, so the method returns an error when the downstream
  response is neither chunked nor H2 (the response header has already
  been sent by filter time, so eligibility is known at the call). The
  filter author chooses how to handle that; nothing changes silently.
- Expected trailers are simply not sent; the response ended at the body.
- Calling it when `end_of_stream` is already true is a no-op.

## Describe alternatives you've considered

- **Changing the filter signature to `end_of_stream: &mut bool`.** More
  direct, but it breaks every existing `ProxyHttp` implementation. The
  session-method shape is purely additive; the tradeoff is one extra flag
  check in the response loop, which seems the better exchange.
- **Returning `Err` from the filter (what we do today).** Works as an
  interim: the client stops receiving and the upstream stops generating.
  But the client sees a protocol error instead of a well-formed response
  end, and the downstream connection is lost. Acceptable as a fallback,
  hostile as the permanent behavior.
- **Swallowing all subsequent chunks after substituting a final
  message.** Clean for the client eventually, but the proxy keeps
  consuming the upstream to completion: unbounded time and, on metered
  upstreams, unbounded cost.
- **Solving it outside pingora-proxy.** Not possible: the `end` flag is
  decided before the filter runs, and the upstream read loop is not
  reachable from filter code.

## Additional context

- Verified against pingora-proxy 0.8.1: the filter call sites in
  `proxy_h1.rs` (the `HttpTask::Body` and `HttpTask::UpgradedBody` arms)
  and `proxy_h2.rs` re-emit the task with the original end flag; the
  filter can mutate `data` only.
- `HttpTask::Done` ("Signal that the response is already finished")
  already exists in `pingora-core`, which suggests the pipeline has the
  vocabulary for this; what is missing is the filter's ability to say it.
- We searched existing issues and pull requests for prior art on forcing
  end-of-stream from the body filter and found none.
- Happy to implement this behind whatever API shape you prefer, with
  tests for the H1 chunked, H2, and Content-Length-refusal paths, if the
  design direction is acceptable.

---

*Filing notes (not part of the issue): CLA required. If review stalls,
the interim is a `[patch.crates-io]` overlay of a branch carrying exactly
this change, referenced from docs/11 D4 by PR number; the current
teardown-on-next-chunk behavior remains the stated fallback. When filed,
record the issue number in docs/11 D4 and in the spike README, and add
the ref to the ledger with expectedState open.*
