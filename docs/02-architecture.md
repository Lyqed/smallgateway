# Architecture

The core is `gatewayd`, a request proxy that can run from a local configuration
file. `gatewayctl` is an optional way to distribute configuration from Git.

These notes describe the implementation and earlier extension experiments.
The [current scope](05-features.md) is forwarding, attribution, token metering,
and token limits. Optional fleet and extension mechanisms do not make a broader
platform the project's goal.

## Data plane

### Canonical event stream

Adapters observe supported provider formats, including OpenAI SSE, Anthropic
events, and Bedrock event-stream, through one internal event model:

```
MessageStart / ContentDelta / ToolCallDelta / UsageDelta / MessageEnd / Error
```

Streaming responses pass through while adapters observe events. Non-streaming
JSON responses use a bounded parser to read terminal usage.

- Live token estimates can be compared with the provider's final usage count.
- Token limits can end a stream with an operator-configured terminal event.
- Missing usage and interrupted responses have explicit handling; see
  [metering decisions](11-http-fidelity.md).

### Scoped policy

Policies compose through fleet, project, route, and app scopes. An explicit
base marker lets a lower scope include inherited values. A local configuration
can use the same rules without running the control plane.

### Existing extension mechanisms

These mechanisms are optional implementation experiments. Expanding them is
not a separate roadmap.

- CEL expressions provide conditions, derivations, and header logic.
- Signed WASM modules provide optional hooks with execution limits.
  Per-event hooks are off by default. See the extension crate's tests and
  documentation before using them.

## Control plane

The optional control plane has the following mechanisms:

| Capability | Implementation |
|---|---|
| Hub-and-spoke distribution | Control plane manages N data planes over an xDS-style gRPC stream (versioned snapshots, ACK/NACK) |
| Desired state in Git | Routes, policy chains, limits, attribution rules, provider refs in a config repo |
| Rendered-manifest pattern | Control plane compiles scoped chains + templates into per-data-plane rendered snapshots; reviewable diffs, no in-gateway templating |
| Fleet-wide config generation | **GatewaySets**: label selectors (region, env, tenant, cloud) × generators stamp config across the fleet |
| Zero-config bootstrap | Join token + Git path; a new data plane self-populates its full bundle, the `argocd cluster add` ergonomic |
| Drift detection / self-heal | Reconciler converges divergent data planes; divergence is surfaced, never silent |
| Progressive delivery with analysis | **Config canaries**: wave rollouts by failure domain, analyzed on error rate, p99, and token-spend anomaly from the gateway's own telemetry, auto-rollback |
| Admission policies | CEL validations on config PRs: "no route without attribution keys," "no unsigned WASM module," "no override >5x default without label" |

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

Some implementation tests use Gateway Baseline check names. The comparison
does not define a roadmap or certify this project:

- **GB-1** required attribution keys as a route field
- **GB-2** claim mappings from verified logins (built, but deferred by
  judgment and lowest priority; see the note in `crates/gatewayd/README.md`.
  It may never ship as a promised capability)
- **GB-3** operator-pinned values, the primary attribution mode
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
