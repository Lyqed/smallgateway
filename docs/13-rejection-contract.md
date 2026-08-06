# The rejection-shape contract

*6 August 2026. What a caller can rely on about every byte this gateway
sends when it refuses, and which shapes the gateway itself owns. Written
for operators of callers that parse rejections with frozen firmware, where
a shape that shifts across upgrades is an outage.*

## The contract

1. **A rejection is the operator's config, verbatim.** Status,
   content-type, and body come from the rejection template in force for
   the request's scope, rendered with exact `{{name}}` substitution and
   nothing else. The gateway never edits, wraps, or annotates an operator
   template.
2. **Rendered from the request's pinned snapshot.** Every request binds
   one config snapshot for its whole life, so a config swap mid-flight
   never changes the shape of a rejection already being served, and every
   rejection log line names the config version that produced it.
3. **Upgrades do not change shapes.** A gateway upgrade never changes a
   route's rejection shape without a config change. This holds by
   construction (the shape lives in config, not code) and is pinned by
   byte-exact conformance tests on the gateway-owned residuals below.
4. **Scope composition applies to every rejection, including mid-stream
   cuts.** Rejection overrides compose fleet → project → route → app,
   lower scope wins, for admission refusals and for the streaming
   terminal event of a mid-stream cut alike. (The cut previously read the
   fleet template; fixed alongside this document, with a conformance
   regression test.)
5. **Graceful denial is a config move, not a feature.** A rejection
   template's status is any value in 100..=599, so a route whose callers
   cannot parse a 4xx can be given a `status: 200` override with a
   completion-shaped body. Rejections never touch the meter, so a 2xx
   denial cannot pollute spend accounting.

## Gateway-owned shapes, frozen

Everything an operator does not configure and a caller can still observe.
Each is pinned byte-exact in tests (`gateway-core/tests/contract_pins.rs`,
`gatewayd/tests/conformance/contract.rs`); changing one is a contract
change and gets a dated entry here.

- **The `model_not_allowed` default.** The one gateway-invented 4xx,
  because the model gate is opt-in per scope: status 403,
  `application/json`,
  `{"error":"model_not_allowed","model":"{{model}}","route":"{{route}}"}`.
- **The default cut payload.** With no operator streaming block in scope,
  a mid-stream cut emits a bare SSE data frame (no event line):
  `{"error":"budget exhausted for <spender>","cap":<cap>,"spend":<spend>}`.
- **The Bedrock exception name.** A cut on the Bedrock dialect is one
  event-stream exception frame; `:exception-type` is the operator's event
  name, or `stream_cut` when unnamed. `:message-type` is `exception`,
  `:content-type` is `application/json`.
- **The placeholder fallbacks.** `{{cap}}` and `{{spend}}` render as `-`
  on every non-budget rejection, so an operator body using them never
  leaks a literal placeholder.
- **SSE terminal-event framing.** An operator event name produces an
  `event:` line; multi-line payloads get one `data:` prefix per line; the
  block ends with the SSE blank line.

## Declared out of contract

- **Infrastructure 502s.** A failed cloud credential exchange (STS, GCP
  token mint) answers 502 with a gateway body. The status is stable; the
  body is diagnostic and MAY change between versions. Callers should key
  on the status, not the body.
- **The wire framing after a mid-stream cut.** The terminal event itself
  is contract; what follows is the stated docs/11 D4 residual: the
  session is torn down on the next upstream chunk rather than finishing
  the downstream encoding cleanly. The clean variant is filed upstream as
  cloudflare/pingora#951; until it lands, callers see the terminal event
  and then an abrupt close, and firmware should treat the terminal event,
  not the close, as the end-of-stream signal.

## Changes

- 2026-08-06: first published, alongside the scoped-cut fix and the pins.
