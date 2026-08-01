# Architecture

*Two binaries plus Git. The data plane makes streaming a first-class citizen;
the control plane is ArgoCD for gateway fleets.*

## The thesis, stated honestly

Every gateway on the verified matrix is either a single instance with a config
surface (LiteLLM, Bifrost, Portkey) or a k8s-CRD system that outsources fleet
management to ArgoCD itself (Envoy AI Gateway, agentgateway via kgateway). The
honest competitive framing: for a k8s-only shop, CRDs plus ArgoCD already
approximate GitOps gateway management. Our bet is the two things that
combination cannot do:

1. **Heterogeneous fleets** — VMs, DMZ boxes, multiple clouds, edge; not just
   clusters.
2. **Domain-aware reconciliation** — the reconciler understands routes, spend
   limits, attribution, and token-aware canary analysis rather than diffing
   opaque YAML.

Kong's decK is the closest prior art for "gateway config in Git," and it is an
imperative sync CLI, not a reconciler. That gap is real.

## Data plane

### Canonical event stream — the streaming answer

The APIM policy had `!isStreaming` guards around token metrics and payload
logging because APIM can only transform what it buffers. Fix this at the
foundation: provider adapters normalize every provider's wire format (OpenAI
SSE deltas, Anthropic events, Bedrock event-stream) into one internal event
model —

```
MessageStart / ContentDelta / ToolCallDelta / UsageDelta / MessageEnd / Error
```

— flowing through the response path with backpressure, never buffered whole.
Policies get `on_request`, `on_response_event`, `on_response_end` hooks. Every
capability that made streaming a second-class citizen becomes uniform:

- **Token metering on streams** — incremental tally, reconciled against the
  provider's terminal usage frame (design question Q3).
- **PII redaction on deltas.**
- **Format rewriting** between provider dialects.
- **Mid-stream enforcement** — a budget exhausted mid-generation cuts the
  stream with an operator-defined terminal event. This is GB-4 extended to
  streaming; nothing in the matrix does it.

### Policy chain with scoped inheritance — APIM's best idea

The single best thing in APIM's policy XML is not the C#, it is `<base/>`:
scoped policies (global → product → API → operation) that compose. Real-world
policy blobs go flat precisely because APIM's scoping isn't granular enough.
Do it properly: chains compose **fleet → project → route → app**, each level
prepends/appends around an explicit base marker, and each level maps to a Git
directory. The STS credential chain becomes a fleet-level policy, attribution
enforcement a project-level one, a per-app TPM override a route-level values
file.

### Two-tier extensibility — the extend answer

- **Tier 1: CEL expressions** for conditions, derivations, header logic.
  Sandboxed, no I/O, microseconds. agentgateway validated this in this exact
  domain.
- **Tier 2: WASM policy modules** for real programs — custom protocol
  adapters, bespoke redaction, org-specific enforcement. Signed modules only
  (admission-checked at the PR), epoch-based preemption and per-event fuel
  budgets on the hot path. Performance validation is a named risk in doc 04 —
  we do not promise per-event WASM hooks until the spike proves them.

## Control plane

The ArgoCD analogy maps almost one to one, which is a good sign it is
structural, not cosmetic:

| ArgoCD concept | Gateway Project equivalent |
|---|---|
| Hub-and-spoke, management cluster | Control plane manages N data planes over an xDS-style gRPC stream (versioned snapshots, ACK/NACK) |
| Application / desired state in Git | Routes, policy chains, limits, attribution rules, provider refs in a config repo |
| Rendered-manifest pattern | Control plane compiles scoped chains + templates into per-data-plane rendered snapshots; reviewable diffs, no in-gateway templating |
| ApplicationSets + generators | **GatewaySets**: label selectors (region, env, tenant, cloud) × generators stamp config across the fleet |
| App-of-apps bootstrap | Join token + Git path; a new data plane self-populates its full bundle — `argocd cluster add` ergonomics |
| Drift detection / self-heal | Reconciler converges divergent data planes; divergence is surfaced, never silent |
| Argo Rollouts + AnalysisTemplates | **Config canaries**: wave rollouts by failure domain, analyzed on error rate, p99, and token-spend anomaly from the gateway's own telemetry, auto-rollback |
| Admission policies (OPA/Kyverno) | CEL validations on config PRs: "no route without attribution keys," "no unsigned WASM module," "no override >5x default without label" |
| Controller sharding, hierarchical Argo | Same story, v2: regional control planes consuming a root's rendered fleet config |

The break-glass-with-TTL detail matters more than it looks: gateways get
emergency-edited at 3am in ways ArgoCD-managed clusters don't tolerate, and
"visible, temporary, auto-reverting" is the honest middle between forbidding
it and losing Git as truth.

### GB-5 at fleet scale — budget shares

The hard distributed-systems problem: a spend limit per attribution value
enforced across N data planes. Central counters add a hop and a SPOF;
pure-local buckets overspend unboundedly. The defensible design is **budget
shares**: the control plane allocates per-data-plane shares from observed
spend telemetry, rebalances continuously, and data planes escalate to
synchronous checks only above ~90% consumption. Document the bounded-overspend
semantics plainly. Platform teams trust stated error bounds and distrust
magic.

## The Baseline connection

This product is the reference implementation of the Gateway Baseline:

- **GB-1** required attribution keys as a route field
- **GB-2** claim mappings from verified logins
- **GB-3** operator-pinned values for callers that don't log in
- **GB-4** rejection templates — including streaming terminal events
- **GB-5** fleet defaults per attribution value, Git-reviewed overrides; the
  100k-token scenario as five lines of YAML
- **GB-6** alert rules firing from the enforcement layer itself
- **GB-7 / GB-8** invoice-grade attribution on AWS and Vertex as core
  features — the agentgateway PRs (session tags, per-request credential
  values) upstreamed and native here
- **GB-9** hot-swappable config with stated bounds ([03-hot-swap.md](03-hot-swap.md))

The tracker becomes the public conformance scoreboard, with the integrity
rule: our own row gets verified identically to everyone else's.
