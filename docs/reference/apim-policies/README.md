# Reference: production APIM policies

These are five real Azure API Management policies running in production in front
of model traffic. They are the source of truth for what this gateway must
reproduce natively. When a design decision is unclear, it is settled here: the
gateway earns its place only if a platform team can retire these policies and
lose nothing.

The product, stated from this evidence: an APIM-style policy engine, with
policies managed as a fleet across Kubernetes clusters. The policy model, the
credential chains, the attribution, and the streaming pain are all visible
below.

## The five policies

Kept verbatim as `.policy.xml` files in this directory:

- `aws-rolechain.policy.xml` — Bedrock via a full STS role chain (managed
  identity to base creds to team creds), session tags, guardrail injection,
  header validation, streaming-aware retry.
- `bedrock.policy.xml` — Bedrock, single AssumeRoleWithWebIdentity hop, the
  simpler attribution baseline.
- `bedrock-rolechain.policy.xml` — the team-per-role variant with a role map
  and the design comment block explaining the CUR and cost-allocation-tag model.
- `vertex.policy.xml` — Vertex (Google, Mistral) via GCP Workload Identity
  Federation (Azure JWT to Google STS to service-account impersonation), label
  injection, the partner-model streaming fix.
- `vertex-graph-lookup.policy.xml` — Vertex with Entra ID token validation and
  a Microsoft Graph lookup (appId to displayName) cached 24h.
- `foundry.policy.xml` — Azure AI Foundry: semantic caching, token-and-request
  rate limits, content safety, event-hub logging.

## What they establish the gateway must do

### 1. The policy model (matches our scoped chain)

APIM's `<inbound> / <backend> / <outbound> / <on-error>` with `<base/>`
composition is the model we already chose: the scoped chain fleet to project to
route to app (docs/02). These policies confirm the primitive set the engine
needs:

| APIM primitive | Our status |
|---|---|
| `set-variable` + C# expressions | CEL tier-1 covers derivations and conditions |
| `choose / when / otherwise` | route conditions (CEL) and rejection branches (GB-4) |
| `return-response` (operator body) | GB-4 rejection templates |
| `set-header` (override/delete) | GB-3 assignment; header stripping at the boundary |
| `set-body` (JSON mutation) | GB-8 label merge; **general body rewrite is not yet a primitive** |
| `send-request` (mid-request side call) | **not yet a primitive** — the credential chains need it |
| `retry` with backoff, streaming-aware | **not yet** — Phase-level, backend section |
| `authentication-managed-identity` | **not yet** — the cloud-identity source |
| `cache-lookup-value / cache-store-value` | **not yet** — the Graph lookup and semantic cache |

The imperative side-effect primitives (`send-request`, general `set-body`,
`retry`, managed-identity) are the gap. CEL is the right tool for the pure
expression parts; the side-effecting steps belong in the WASM policy tier
(Phase 4) or as first-class named policy steps.

### 2. GB-7 is a role CHAIN, not one AssumeRole

Our GB-7 milestone did the session-tag AssumeRole plus SigV4. The production
policy does more, and the extra hops are load-bearing:

1. Managed identity to Azure AD JWT.
2. `AssumeRoleWithWebIdentity(JWT)` to BASE creds (federation, not a static key).
3. `AssumeRole` signed with the base creds, carrying session `Tags` (App, User,
   Subscription) and a `RoleSessionName` of `team-app-user`, to TEAM creds. Tags
   are transitive; the RoleSessionName lands in the CUR line-item / IAM-principal
   column.
4. SigV4-sign Bedrock with the team creds.

The attribution dimensions that result: Team (the team role's own IAM tag,
activated as a cost-allocation tag of type "IAM principal"), App and User
(session tags to Cost Explorer), and User again in the CUR via RoleSessionName.
GB-7 in the gateway should model the whole chain, with the web-identity
federation hop and the base-to-team step, not just the final tagged AssumeRole.

### 3. GB-8 for Vertex needs Workload Identity Federation

The Vertex policy is the GCP parallel of the AWS chain: Azure JWT to Google STS
token-exchange, to `generateAccessToken` service-account impersonation, to a
`Bearer` token on the Vertex call, with `labels` merged into the body. Our GB-8
stamps labels but does not yet do the credential exchange. Both clouds need the
"federate an Azure managed identity into the cloud, then attribute" flow.

### 4. The `!isStreaming` guard is exactly what the canonical event stream fixes

Every policy skips token metrics on streaming, with the same comment: buffering
the response to count tokens breaks real-time streaming. This is the APIM
limitation the canonical event model (Spike A, `gateway-core`) removes. These
policies are the direct evidence for that architectural bet: streaming is a
first-class citizen here, so metering, safety, and rewriting apply to it
uniformly, and the `!isStreaming` guard disappears.

### 5. Policy types beyond attribution the gateway will eventually need

Named from the policies, not promised on any date: guardrail-config injection
into the body before signing; forced guardrail headers overriding client values
(SCP compliance); content-safety / prompt-shield; semantic caching with a
per-subscription vary-by; token-and-request rate limits; event-hub / structured
request-response logging; and identity resolution with a cached directory lookup
(appId to display name, 24h). These map to future policy steps and to the WASM
tier.

## How to use this reference

Any milestone that touches attribution, credentials, body rewriting, or the
policy primitive set reconciles against these files first. A capability the
gateway claims is only real when the matching policy above could be deleted and
replaced by gateway config with no loss. The conformance suite is the proof; this
directory is the specification behind it.

## A note on GB-2 (identity from a login), consistent with the deferral

Two of these policies do validate an Entra ID token and even resolve identity via
Graph. That is real, and it is exactly the capability GB-2 covers. It stays
deferred by judgment anyway (see `crates/gatewayd/README.md`): where these
policies validate a token they do it for authorization (only callers with a role
may pass), while attribution still comes from the `X-Team` / `X-App` headers the
operator trusts at the boundary. The gateway assigns and validates the tag; it
does not need to prove the caller's directory identity to attribute spend, and
the cost of doing so on every request is the open question the essay leaves open.
