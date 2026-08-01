# Draft PR: GB-8

> Status: draft, not submitted. Branch `gb8-vertex-operator-labels`, one
> commit on `66713a6ec3bae2d597bc5639a39bb55da72e871a`, with a DCO
> `Signed-off-by` line matching the author email (see README.md submission
> notes).

## Title

```
vertex: operator-configured billing labels for invoice-grade attribution
```

## Body

#2023 shipped the native Vertex `generateContent`/`streamGenerateContent`
path and a top-level `labels` field on the request. That field is
caller pass-through: the label values come from the client request
(`crates/llm/src/conversion/vertex_gemini.rs`, `req.rest.get("labels")`),
so the operator cannot decide how Vertex spend is attributed. A caller can
omit labels or send another team's value, and the value that reaches Google
Cloud billing is caller-asserted, not operator-set.

This change makes those labels operator-configurable, so the value reaching
Google Cloud billing on the request is operator-decided. That is what turns
Vertex cost attribution from caller-asserted into invoice-grade: the
operator-chosen App/Team/tenant value reaches billing on the
`generateContent` request labels, and finance can slice the Vertex bill by
those dimensions with the value set by the operator rather than trusted from
the caller.

The gateway already provides invoice-grade attribution on AWS via CEL-valued
STS session tags (#2435/#2447, `AwsSessionTag { key, value, expression }` in
`crates/agentgateway/src/http/auth/aws.rs`), which surface in the AWS Cost &
Usage Report. This applies the same pattern to the second cloud, so operators
get operator-set attribution on both major clouds: Bedrock via session tags,
Vertex via these labels.

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
  cannot override the gateway's attribution (the merge site is
  `from_completions::merge_labels` in
  `crates/llm/src/conversion/vertex_gemini.rs`). Client labels without
  conflicts pass through unchanged, and configs without `labels` are
  unchanged.
- Label keys and static values are validated against Google Cloud's label
  requirements (charset, 63-char key/value limits, max 64 labels) at config
  load; dynamic values are validated per request and fail closed. An
  expression that cannot produce a valid value rejects the request
  (`AIError::LabelResolution` → `ProxyError::Processing`), so no request is
  forwarded to Vertex unlabeled. This matches the AWS session tag posture.
- Implementation follows existing in-tree shapes:
  - `VertexProvider` wraps `vertex::Provider` the same way `BedrockProvider`
    and `AzureProvider` already wrap theirs in
    `crates/agentgateway/src/llm/mod.rs` (`#[serde(flatten)]` + Deref +
    `AIProvider::vertex()`), keeping the CEL-typed config in the gateway
    crate while `crates/llm` stays CEL-free (its `translate` takes the
    already-resolved label map, `Option<&serde_json::Map<String, Value>>`).
  - `VertexLabels` mirrors `AwsSessionTags`: static/dynamic split with
    config-load validation, the same `try_new`/`is_empty`/`expressions`
    surface, the same custom de/ser helpers, and it reuses
    `aws::cel_value_to_string` for fail-closed CEL coercion.
  - Expressions register in `BackendPolicies::register_cel_expressions`
    (`crates/agentgateway/src/store/binds.rs`) alongside the AWS auth
    expressions, so the request-snapshot capture works the same way it does
    for the session tag expressions. Resolution uses the existing
    `Executor::new_request_snapshot`; no new CEL machinery is added.
- `cargo xtask schema` output is included; the `VertexProvider` schema
  definition now carries `labels` (array of `VertexLabel`).

### Tests

- `crates/llm/src/conversion/vertex_gemini_tests.rs`: merge semantics
  (operator labels added without client labels, merged alongside them,
  operator wins on conflict, empty operator set is a no-op).
- `crates/agentgateway/src/llm/tests.rs`: config-time validation (charset,
  length, duplicates, exactly-one-of value/expression), YAML config parsing
  including rejection of an invalid key, end-to-end `render_vertex_gemini`
  with a request snapshot resolving a CEL label from a request header (the
  test sends a spoofed client `labels.team` and asserts the operator value
  wins), and fail-closed rejection on an unresolvable expression.

`cargo test -p agent-llm` (258 tests, including the golden tests over the
changed translate path) and `cargo test -p agentgateway --lib llm::tests`
(59 tests, 4 new) pass against the patch applied to a pristine base;
`cargo clippy -p agent-llm -p agentgateway --all-targets` and the `make lint`
fmt check are clean.

### Tradeoffs

Stated plainly so they are not hidden:

- XDS-configured Vertex backends are unchanged: the Vertex XDS proto has no
  labels field, so the XDS conversion builds the provider via
  `AIProvider::vertex()`, which yields an empty operator label set. Operator
  labels are file/local-config only for now. This is a deliberate scope
  limit, not a silent behavior shift; XDS-configured vertex backends behave
  exactly as before.
- `deny_unknown_fields` no longer applies to the vertex provider block. serde
  cannot enforce it through `#[serde(flatten)]`, the same tradeoff the
  existing Bedrock/Azure wrappers already carry, and the same one called out
  explicitly for the flattened `Claims` block in
  `crates/agentgateway/src/http/apikey.rs`. `VertexLabel` itself keeps
  `deny_unknown_fields` (it uses the `schema!` alias), so unknown keys inside
  a label are still rejected.
- Dynamic labels cost one CEL evaluation per request. Static labels are
  pre-resolved at config load and cost nothing per request; only labels
  configured with an `expression` evaluate per request, and the lazy
  request-snapshot context only captures what the expressions reference.
- Fail-closed rejects the request on a bad expression rather than dropping
  the label or forwarding unlabeled. A silently-unlabeled request is an
  attribution hole, so rejection is the intended behavior, matching the AWS
  session tag posture. Static-only label configs cannot hit this path.
- One insta snapshot updated (`llm_simple_normalized.snap`): `flatten`
  serializes the provider through a map, which alphabetizes the provider's
  field order in the normalized-config dump. Cosmetic only; the wire format
  (JSON/YAML object) is order-insensitive.
