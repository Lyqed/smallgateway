# Features — the Baseline, and the optional catalog

*Two lists, kept deliberately separate. The first is the product; the second
is triage. The bar for the second list is LiteLLM's entire surface — if the
most feature-rich gateway on the matrix does it, it appears here with a
verdict, so nobody has to wonder whether we "forgot" it.*

**Procurement note, up front: you don't need to buy anything, yet.** Every
row below is either open source we build, open source we host, or a deferral.
When a vendor demo maps their sales deck onto this table, the verdicts don't
change — the Baseline cells are verified against documentation, not against
demos. Defer, defer, defer.

## Required: the Gateway Baseline

These are acceptance tests, not aspirations — see
[04-build-plan.md](04-build-plan.md) for which phase delivers each.

| Check | Behavior | Phase |
|---|---|---|
| GB-1 | Every request is tagged with who it is for | 1 |
| GB-2 | The tag can be read from a verified login | 1 |
| GB-3 | The tag can be assigned (operator-pinned, never believed) | 1 |
| GB-4 | A blocked request says why, in your words — including a streaming terminal event | 1 |
| GB-5 | Every spender gets a cap by default (budget shares, bounded overspend) | 3 |
| GB-6 | Someone is told when a cap is hit | 3 |
| GB-7 | The tag reaches the AWS bill | 1 |
| GB-8 | The tag reaches the Vertex bill | 1 |
| GB-9 | The rules can change while it runs (bounded staleness, stated) | 4 |

## Optional: the catalog, triaged

Verdicts: **adopt** (planned, phase noted) · **compose** (the policy chain
and event stream make it a module, not a feature) · **defer** (real value,
not yet, revisit when pulled by users) · **decline** (conflicts with the
principles).

### Routing and resilience

| Feature (LiteLLM et al.) | Verdict | Notes |
|---|---|---|
| Multi-provider routing (100+ providers) | **adopt, incrementally** | Three providers in Phase 1; adapters are the community's easiest first contribution |
| Model fallbacks on error/timeout | **adopt (P2)** | A route-level policy on the chain |
| Retries with backoff | **adopt (P2)** | Same |
| Load-balancing strategies (latency-based, least-busy, weighted) | **compose** | Router strategies as policy modules over the same telemetry the canaries use |
| Headroom-aware routing (send to the deployment with remaining TPM/RPM capacity) | **compose (P3)** | Falls out of budget shares — the share ledger *is* the headroom signal; LiteLLM tracks this per-deployment, we get it fleet-wide |
| Cross-region/cloud failover | **adopt (P5)** | The heterogeneous-fleet story is the differentiator; this is its demo |

### Cost and quota

| Feature | Verdict | Notes |
|---|---|---|
| Virtual keys | **adopt (P1)** | A key is just an assigned GB-3 tag with credentials attached |
| Key/team/org budgets | **adopt (P3)** | GB-5 generalized up the scope chain |
| Budget headroom alerts (soft thresholds before the hard cap) | **adopt (P3)** | GB-6 with a threshold parameter; alert at 80%, enforce at 100% |
| TPM/RPM rate limits per key/model | **adopt (P3)** | Same counters as budgets, different unit |
| Spend tracking + tags, usage export | **adopt (P1/P3)** | Attribution is the core product; export is a view over it |
| Cost map / model pricing table | **adopt (P3)** | Versioned in Git like everything else, not scraped at runtime |

### Caching and performance

| Feature | Verdict | Notes |
|---|---|---|
| Response caching (exact-match) | **defer** | Real savings, but correctness edges (stream replay, cache-key attribution) deserve their own design pass |
| Semantic caching | **defer** | Embedding infra dependency; revisit when exact-match caching has users |
| Prompt caching passthrough (provider-native) | **adopt (P1)** | Adapter concern; don't break what providers already do |

### Safety and governance

| Feature | Verdict | Notes |
|---|---|---|
| Operator-forced guardrail headers/body (Bedrock guardrails, Model-Armor-class attach points) | **implemented** | `inject:` per provider/route: forced headers enter the SigV4 signed set, body fields inject before signing, never caller-steerable |
| Guardrails hooks (moderation, jailbreak, PII) | **compose (P4)** | The canonical event stream makes these per-delta WASM modules; we ship the hook, the community ships the guardrail — versioned with the config snapshot, rolled out in waves like any rule |
| PII redaction on streams | **compose (P4)** | Flagship demo of the event model |
| Model allow-list per scope (which models a client may use) | **adopt (P1)** | Keyed by adjudicated attribution on the scoped chain, never by caller-asserted identity; model from the path (bedrock/vertex) or the buffered body (openai dialects); refused with the operator's GB-4 body |
| Audit logs | **adopt (P2)** | Falls out of Git-as-truth + snapshot history |
| Tamper-evident audit trail (hash-chained adjudication log) | **defer (P4)** | Honest logs exist; tamper-evidence is a real design (chained hashes, external anchor) and is claimed NOWHERE until built |
| OpenTelemetry export (OTLP for adjudications + meter) | **adopt (P2)** | The observability version of the invoice thesis: ship spans/metrics to the collector you already own; no dashboard of ours |
| SSO / RBAC for the control plane | **defer (P5)** | OIDC first, nothing bespoke |

### Interfaces

| Feature | Verdict | Notes |
|---|---|---|
| OpenAI-compatible API surface | **adopt (P1)** | Table stakes; the adapters translate outward from the canonical model |
| Pass-through endpoints (provider-native routes) | **adopt (P2)** | With attribution enforced even on passthrough — that's the point |
| Admin UI | **defer** | Git is the UI until the reconciler is trustworthy; a read-only fleet view ships with P5 canaries |
| Playground / prompt management | **decline** | Other tools do this well; a gateway that grows a prompt IDE is accreting Spinnaker |
| Langfuse/OTel telemetry export | **adopt (P2)** | OTel-native from the start; the canary analysis consumes the same stream |

The triage is a living document. A **defer** moves to **adopt** when users
pull it, never when a competitor ships it — the matrix measures verified
behavior, not feature-list length.
