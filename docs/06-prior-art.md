# Prior art — what we steal, and from whom

*Best of all worlds means naming the worlds. Every borrowed idea is listed
with its source and the form it takes here; every rejected idea is listed with
the reason.*

## The steal list

| Source | Idea | Form it takes here |
|---|---|---|
| APIM | `<base/>` scoped policy composition | The scoped chain: fleet → project → route → app, each level a Git directory ([02-architecture.md](02-architecture.md)) |
| APIM (negative lesson) | `!isStreaming` guards — buffering-only transforms | The canonical event stream; streaming is first-class or the design is wrong |
| ArgoCD | Git as truth, rendered manifests, app-of-apps, drift detection, ApplicationSets | The whole control plane shape, mapped one-to-one in doc 02 |
| Spinnaker | Automated canary analysis (Kayenta), manual judgment gates | Config canaries + PR approvals — as Git-native mechanisms, not a pipeline engine |
| Spinnaker (negative lesson) | 10+ microservices, database as truth | The two-binaries-plus-Git budget in doc 00 |
| Envoy | xDS: versioned snapshots over long-lived gRPC, ACK/NACK | The fleet transport, without adopting Envoy itself |
| agentgateway | CEL for gateway conditions; LLM-aware routing; the credential paths | Tier-1 extensibility; possibly the whole proxy foundation (Phase 0 spike B) |
| Pingora | A proxy *library* with streaming body filters, not a proxy product | The other candidate foundation in spike B |
| Kong decK | Gateway config in Git (closest prior art) | The gap it leaves — imperative sync CLI, no reconciler — is our opening |
| LiteLLM | The feature catalog: virtual keys, budgets, headroom, fallbacks, spend tags, the lot | The triage table in [05-features.md](05-features.md) — adopted, composed, or deferred, never silently ignored |
| Kubernetes | Reconciliation as the control loop; admission control | Domain-aware: the reconciler understands tokens and spend, not opaque YAML |

## agentgateway's path to 6/8 — the extend-don't-fork proof

*Reads straight off the verified Baseline matrix. This analysis is why
"extend" is a real strategy and not a slogan: two config-knob-sized upstream
changes take agentgateway from 4/8 to sole leader.*

**1. Custom rate-limit rejection bodies (GB-4, partial → conforms).** The
machinery exists in-tree: conditional `directResponse` returns
operator-defined bodies for CEL-matched rejections. The one verified gap: the
local rate-limit 429 body is not customizable. Wiring the existing response
machinery into the rate-limit filter's local reply is a config-knob-sized
change; Envoy AI Gateway's `responseOverride` is prior art in the same family.
Days, not quarters.

**2. Native Vertex `generateContent` with billing labels (GB-8, missing →
conforms).** Already open as PR #2023. Today translated Gemini traffic hits
the OpenAI-compat endpoint, which drops labels. Land the native path plus an
operator-set labels field and GB-8 closes; the CEL-values-on-cloud-credentials
pattern is already proven in the same codebase by #2435/#2447 on Bedrock. Same
shape, second cloud. First gateway with invoice-grade attribution on both
major clouds.

Result: 4/8 → 6/8, sole leader by two points. The remaining pair (GB-5 native
default budgets, GB-6 built-in alerts) is the structurally hard half: both
need durable shared state across data planes — which is exactly the layer this
project greenfields, and exactly where the hot-swap limitations in
[03-hot-swap.md](03-hot-swap.md) live.

The strategic point: those PRs advance the ecosystem whether or not this
project's spike chooses agentgateway as its foundation. Extending upstream and
building the community gateway are not competing bets — the matrix rises
either way, and the Baseline stays the neutral yardstick that makes both
legible.

## The decline list

- **A pipeline engine.** Every borrowed Spinnaker idea arrives as a Git
  mechanism; the moment we have "stages," we have accreted Spinnaker.
- **A new general-purpose language.** Doc 01, Q6: the invented language is a
  small, total policy DSL. General-purpose language design is deferred
  creativity — indefinitely.
- **Runtime templating in the gateway.** Rendering happens in the control
  plane, ahead of time, reviewably. A gateway that templates at request time
  has an unauditable config.
- **Trusting caller-sent tags.** GB-2/GB-3's whole point: proven or assigned,
  never believed. No feature is worth reopening that door.
