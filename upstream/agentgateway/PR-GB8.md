# Draft PR — GB-8

> Status: draft, not submitted. Branch `gb8-vertex-operator-labels`, one
> commit on `66713a6ec3bae2d597bc5639a39bb55da72e871a`. Check CLA/DCO
> requirements at submission time (see README.md).

## Title

```
vertex: support operator-configured billing labels for native Gemini requests
```

## Body

The native Vertex `generateContent` path (#2023) forwards billing `labels`
from the client request only
(`crates/llm/src/conversion/vertex_gemini.rs`, `req.rest.get("labels")`).
That means cost attribution is entirely in the caller's hands: a client can
omit labels or send someone else's, and the gateway operator has no way to
stamp requests with trusted attribution.

The gateway already solved this exact problem on AWS with CEL-valued STS
session tags (#2435/#2447 — `AwsSessionTag { key, value, expression }` in
`crates/agentgateway/src/http/auth/aws.rs`). This PR applies the same pattern
to the second cloud.

### What this does

- Adds an operator-set `labels` field to the Vertex provider config. Each
  label is either a static `value` or a CEL `expression` evaluated against
  the request:

  ```yaml
  backends:
  - ai:
      name: vertex
      provider:
        vertex:
          projectId: my-project
          labels:
          - key: cost_center
            value: platform
          - key: tenant
            expression: jwt.sub
  ```

- Operator labels are merged into the outbound `generateContent` request over
  client-sent labels; on a key conflict the operator's value wins, so callers
  cannot override the gateway's attribution. Client labels without conflicts
  pass through unchanged (no behavior change for configs without `labels`).
- Label keys and static values are validated against Google Cloud's label
  requirements (charset, 63-char limits, max 64 labels) at config load;
  dynamic values are validated per request and fail closed — an expression
  that cannot produce a valid value rejects the request, matching the AWS
  session tag posture.
- Implementation follows existing in-tree shapes:
  - `VertexProvider` wraps `vertex::Provider` the same way `BedrockProvider`
    and `AzureProvider` already wrap theirs in
    `crates/agentgateway/src/llm/mod.rs` (`#[serde(flatten)]` + Deref +
    `AIProvider::vertex()`), keeping the CEL-typed config in the gateway
    crate while `crates/llm` stays CEL-free (its `translate` just takes the
    resolved label map).
  - `VertexLabels` mirrors `AwsSessionTags`: static/dynamic split with
    config-load validation, custom de/ser, `expressions()` for registration.
  - Expressions register in `BackendPolicies::register_cel_expressions`
    alongside the AWS auth expressions, so request-snapshot capture works the
    same way it does for webhook header expressions.
- Not wired through XDS in this PR (the Vertex proto has no labels field);
  the XDS conversion builds the provider via `AIProvider::vertex()`, which
  yields an empty operator label set, so XDS-configured vertex backends
  behave exactly as before. Operator labels are file/local-config only for
  now.
- One known tradeoff, shared with the existing Bedrock/Azure wrappers: serde
  does not enforce `deny_unknown_fields` through `#[serde(flatten)]`, so
  unknown keys in the vertex provider block are no longer rejected at parse
  time.
- One insta snapshot updated (`llm_simple_normalized.snap`): `flatten`
  serializes through a map, which alphabetizes the provider's field order in
  the normalized dump. Cosmetic only.
- `cargo xtask schema` output is included; the `VertexProvider` schema
  definition now carries `labels` (array of `VertexLabel`).

### Tests

- `crates/llm/src/conversion/vertex_gemini_tests.rs`: merge semantics —
  operator labels added without client labels, merged alongside them,
  operator wins on conflict, empty operator set is a no-op.
- `crates/agentgateway/src/llm/tests.rs`: config-time validation (charset,
  length, duplicates, exactly-one-of value/expression), YAML config parsing,
  end-to-end `render_vertex_gemini` with a request snapshot resolving a CEL
  label from a request header, and fail-closed rejection on an unresolvable
  expression.

`cargo test -p agent-llm` (258 tests, including the golden tests over the
changed translate path) and `cargo test -p agentgateway --lib` (1621 tests)
pass; `cargo clippy --all-targets` and the `make lint` fmt check are clean.

---

Context: this change came out of gateway-baseline conformance checks for
operator-assigned (rather than caller-asserted) cost attribution
(https://thegatewaybaseline.com).
