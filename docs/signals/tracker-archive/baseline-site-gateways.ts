/**
 * The Gateway Baseline conformance tracker.
 *
 * Every gateway measured against the same six requirements, coded GB-1
 * through GB-9. Statuses are hand-verified against public documentation on
 * the date recorded per gateway; our own row is scored from the code and
 * held to the same bar, reds and all. The integrity rule is the whole
 * point: our row is verified identically to everyone else’s.
 *
 * Semantic colors follow the design tokens: teal = verified (yes), gold =
 * partial, blossom = missing (no). Never swap these roles.
 */

export type CheckStatus = "yes" | "partial" | "no";

export type Check = {
  /** Stable key, matches the cell keys below. */
  key: string;
  /** Coded check, GB-1..GB-9. */
  code: string;
  /** Short column label for the matrix. */
  short: string;
  /** One-line definition of the check. */
  title: string;
};

export type GatewayRow = {
  id: string;
  name: string;
  kind: "reference" | "open source" | "commercial" | "cloud service";
  url: string;
  /** True for our own reference implementation. */
  ours: boolean;
  /** ISO date this row was last verified. */
  lastVerified: string;
  /** key -> { status, note }. */
  cells: Record<string, { status: CheckStatus; note: string }>;
};

export const LAST_VERIFIED = "2026-08-03";

export const CHECKS: readonly Check[] = [
  { key: "enforced-keys", code: "GB-1", short: "Tagged", title: "Every request is tagged with who it is for" },
  { key: "jwt-values", code: "GB-2", short: "From login", title: "The tag can be read from a verified login" },
  { key: "static-values", code: "GB-3", short: "Operator-set", title: "The tag is assigned, never trusted from the caller" },
  { key: "error-bodies", code: "GB-4", short: "Refusal", title: "A blocked request says why, in the operator’s words" },
  { key: "default-limit", code: "GB-5", short: "Capped", title: "Every spender has a cap by default" },
  { key: "alerts", code: "GB-6", short: "Alerted", title: "Someone is told when a cap is hit" },
  { key: "aws-invoice", code: "GB-7", short: "AWS bill", title: "The tag reaches the AWS invoice" },
  { key: "vertex-invoice", code: "GB-8", short: "Vertex bill", title: "The tag reaches the Vertex invoice" },
  { key: "live-changes", code: "GB-9", short: "Live change", title: "The rules and config can change while it runs, with no dropped requests and stated staleness" },
  { key: "fleet-gitops", code: "GB-10", short: "Fleet as Git", title: "The fleet is managed the way teams already manage clusters: desired state in Git, a reconciler that converges it" },
  { key: "metered-shapes", code: "GB-11", short: "All shapes metered", title: "Every traffic shape the gateway carries is metered and attributed — nothing escapes the meter, and inexactness is stated, never silent" },
  { key: "invoice-true", code: "GB-12", short: "Invoice-true", title: "The spend figure is the bill’s, not a guess — provider-authoritative usage, no fabricated dollars, estimates only with a stated error bound" },
];

export const GATEWAYS: readonly GatewayRow[] = [
  {
    id: "the-gateway-baseline",
    name: "The Open Source Gateway",
    kind: "reference",
    url: "https://smallgateway.vercel.app",
    ours: true,
    lastVerified: "2026-08-03",
    cells: {
      "enforced-keys": { status: "yes", note: "fleet.attribution.required_keys refuses unattributed requests at the door with the operator template; proven by gb1_missing_required_key_rejected against the." },
      "jwt-values": { status: "partial", note: "HS256 claim->key mapping is built and tested (gb2_claim_mapped_key_proven_from_verified_jwt) but deferred by judgment as lowest-priority; RS256/JWKS unbuilt, may." },
      "static-values": { status: "yes", note: "Operator-pinned attribution: caller headers for pinned keys are stripped then re-inserted with adjudicated values, never believed." },
      "error-bodies": { status: "yes", note: "Per-provider/route operator rejection templates including streaming terminal events and a dedicated cap-exceeded voice, scoped down the chain (gb4 conformance tests)." },
      "default-limit": { status: "yes", note: "Fleet-default spend_caps per attribution value (budget.rs composed fleet default), budget shares with bounded overspend under partition, per-value overrides." },
      alerts: { status: "yes", note: "GB-6 soft-80% and hard-cap alerts fire from the enforcement layer to a validated webhook sink at operator alert_at (alerts_fire_from_the_meter_at_soft_then_hard)." },
      "aws-invoice": { status: "yes", note: "SigV4 AssumeRole with attribution-derived STS session tags reaches CUR, operator-set and caller-stripped; raw caller session tag rejected at config load (gb7 tests)." },
      "vertex-invoice": { status: "yes", note: "Operator billing labels merged into the native generateContent body before signing, operator wins on conflict so callers cannot override (labels.rs, gb8 tests)." },
      "live-changes": { status: "yes", note: "Phase-4 versioned snapshot hot-swap: each request pins its Arc<Snapshot> for life, old versions drain with last in-flight stream, stated bounded staleness." },
      "fleet-gitops": { status: "yes", note: "Desired state in Git, a domain-aware reconciler that converges N data planes across heterogeneous fleets (VMs, DMZ, edge, multi-cloud), drift heals toward Git. The product itself." },
      "metered-shapes": { status: "partial", note: "Every carried shape meters through one event model, degradation loud, never silent; two gaps stated in docs/11 — default OpenAI streams settle on estimate, aborts under-bill." },
      "invoice-true": { status: "yes", note: "Never invents a dollar: usage is the provider’s terminal frame, the live estimate reconciles to it per request with a published bound, and dollars live only on the invoice." },
    },
  },
  {
    id: "agentgateway",
    name: "agentgateway",
    kind: "open source",
    url: "https://agentgateway.dev",
    ours: false,
    lastVerified: "2026-08-03",
    cells: {
      "enforced-keys": { status: "yes", note: "CEL-based auth/RBAC policies require and evaluate attribution per request; unattributed traffic is refused, not caller-optional." },
      "jwt-values": { status: "yes", note: "Validated JWT/OIDC claims (jwt.sub, custom claims) map into attribution and downstream CEL descriptors and session tags." },
      "static-values": { status: "yes", note: "Operator-pinned values are set via CEL on key/route/backend and evaluated server-side, so the caller cannot override the pinned attribution." },
      "error-bodies": { status: "partial", note: "Rate-limit rejections return a fixed 429 'rate limit exceeded'; only the budget ImmediateResponse status is being made configurable, no operator-worded custom body." },
      "default-limit": { status: "partial", note: "RateLimit/budget policies attach opt-in per Gateway/HTTPRoute and count per-instance; no fleet-wide default cap auto-applied to every attribution value." },
      alerts: { status: "no", note: "Budget/rate-limit state surfaces only as Prometheus metrics; no built-in Slack/webhook/email notification when a cap is hit." },
      "aws-invoice": { status: "yes", note: "AwsSessionTag {key,value,expression} sets STS TagSession tags via per-request CEL (jwt.sub etc.) that reach the AWS CUR, caller cannot forge; PRs #2435/#2447 merged." },
      "vertex-invoice": { status: "no", note: "Native generateContent labels (#2023) are caller pass-through per issue #2490; the operator-set labels knob is PR #2806, still OPEN, so Vertex attribution is not." },
      "live-changes": { status: "yes", note: "Local config hot-reloads via file-watch into the shared IR and xDS pushes are zero-downtime; kgateway v2.1.0 control plane adds graceful shutdown/zero-downtime." },
      "fleet-gitops": { status: "partial", note: "Config is Gateway API CRDs; multi-cluster convergence is outsourced to an external GitOps tool (ArgoCD/Flux, both documented), not agentgateway itself. The data plane runs anywhere, but the fleet model needs clusters and diffs opaque YAML, not routes or spend." },
      "metered-shapes": { status: "partial", note: "All LLM shapes hit gen_ai_client_token_usage (SSE via forced include_usage, realtime via response.done); MCP gets only request counts, A2A only access logs." },
      "invoice-true": { status: "partial", note: "Dollars are list price from a models.dev catalog (agctl costs import) shown as realized USD; unpriced models flagged not zeroed, but nothing reconciles to the bill." },
    },
  },
  {
    id: "litellm",
    name: "LiteLLM",
    kind: "open source",
    url: "https://docs.litellm.ai",
    ours: false,
    lastVerified: "2026-08-03",
    cells: {
      "enforced-keys": { status: "partial", note: "enforced_params (e.g. require user/metadata.generation_name, rejects missing with 'please pass param=user') exists but is an Enterprise-gated feature, not free-tier." },
      "jwt-values": { status: "partial", note: "OIDC/JWT claims map to attribution via JWT-to-virtual-key mapping, but docs state 'JWT → Virtual Key Mapping is an Enterprise feature.'" },
      "static-values": { status: "yes", note: "Tags/metadata are operator-pinned per virtual key/team, and 'Reject Client-Side Metadata Tags' lets the operator refuse caller-supplied tag overrides." },
      "error-bodies": { status: "partial", note: "Custom rejection message/status is settable via a ProxyException in a custom_auth/guardrail Python hook, not a pure config-level rejection body." },
      "default-limit": { status: "yes", note: "Fleet-wide default caps exist via litellm_settings max_internal_user_budget and max_end_user_budget_id, applied to users/end-users by default, not per-key opt-in only." },
      alerts: { status: "yes", note: "Built-in Slack/webhook/email budget and threshold alerting ships in the proxy, not merely Prometheus metrics you wire yourself." },
      "aws-invoice": { status: "partial", note: "PR #32797 (aws_session_tags on STS AssumeRole) MERGED 2026-07-16, so operator-set tags now reach AWS CUR and caller cannot forge, but tags are config-pinned per." },
      "vertex-invoice": { status: "partial", note: "LiteLLM forwards a labels field (and converts string metadata to labels) into Vertex generateContent for GCP billing, but docs show no server-side label pinning so." },
      "live-changes": { status: "partial", note: "With store_model_in_db, pods poll and converge on config changes within proxy_config_reload_interval_seconds (default 30s), but a process restart drops in-flight." },
      "fleet-gitops": { status: "no", note: "A single proxy instance with a config surface; no fleet-as-Git model, no reconciler converging many data planes toward a repo." },
      "metered-shapes": { status: "partial", note: "Chat, embeddings, audio and realtime reach spend logs; MCP and A2A meter only via operator-set cost_per_query, and Vertex passthrough tracks /generateContent alone." },
      "invoice-true": { status: "partial", note: "Usage frames are provider-real but dollars are tokens times model_prices json; unmapped models log $0 and nothing reconciles against the invoice." },
    },
  },
  {
    id: "portkey",
    name: "Portkey",
    kind: "commercial",
    url: "https://portkey.ai/docs",
    ours: false,
    lastVerified: "2026-08-03",
    cells: {
      "enforced-keys": { status: "partial", note: "Owners can define mandatory metadata fields and requests that omit/mismatch them are rejected before forwarding, but required-metadata enforcement is an." },
      "jwt-values": { status: "partial", note: "JWT/OIDC validation (JWKS/introspection, requiredClaims, claim matching) with a claims_header forwarding sub/email/workspace_id exists but is documented on the MCP." },
      "static-values": { status: "yes", note: "Metadata precedence is Workspace (highest) then API-key then Request (lowest), so operator-pinned workspace/key values outrank and cannot be overridden by the caller." },
      "error-bodies": { status: "no", note: "Guardrail denials return a hardcoded 446 status with no documented custom status code or operator-defined rejection body." },
      "default-limit": { status: "partial", note: "Budget limits (USD or token-based) are set per virtual key and expire the key when hit, but must be explicitly configured per key with no fleet-wide default and are." },
      alerts: { status: "partial", note: "Budget limits support email notifications at configurable alert thresholds, but no Slack or webhook alerting is documented and the feature is Enterprise-gated." },
      "aws-invoice": { status: "no", note: "Bedrock integration uses an Assumed Role ARN for access only; no documentation of Portkey injecting per-request STS session tags (TagSession) so operator metadata." },
      "vertex-invoice": { status: "yes", note: "Portkey request metadata is forwarded as Vertex AI resource labels into native calls (enterprise changelog notes the fix for mislabeled request types), reaching GCP." },
      "live-changes": { status: "yes", note: "Gateway configs are referenced by ID and edited in the UI take effect on the next request with no commits or redeploys, and configs are resolved per-request so." },
      "fleet-gitops": { status: "no", note: "Managed through a hosted console and API; no desired-state-in-Git reconciler over a fleet of self-owned gateways." },
      "metered-shapes": { status: "partial", note: "MCP calls logged per-user; SSE cost needs stream_options.include_usage opt-in; unpriced models flow at $0.00 outside budget limits." },
      "invoice-true": { status: "partial", note: "Provider usage tokens priced via Portkey’s central pricing JSON (Portkey-AI/models, 24h cache); no invoice reconciliation, unpriced models show $0.00." },
    },
  },
  {
    id: "kong",
    name: "Kong AI Gateway",
    kind: "commercial",
    url: "https://developer.konghq.com",
    ours: false,
    lastVerified: "2026-08-03",
    cells: {
      "enforced-keys": { status: "partial", note: "OIDC/key-auth can require a consumer and reject anonymous, but attribution enforcement is per-route plugin config, not a gateway-wide refusal of unattributed traffic." },
      "jwt-values": { status: "partial", note: "OpenID Connect plugin's config.consumer_claim maps validated JWT/OIDC claims to a Kong consumer (id/username/custom_id), but full OIDC is Kong Enterprise-gated." },
      "static-values": { status: "yes", note: "Operators pin consumer/credential per key and AI Rate Limiting policies match on operator-set consumer identifiers, never a caller-supplied value." },
      "error-bodies": { status: "partial", note: "AI Rate Limiting Advanced returns a fixed 429 with a canned {\"message\":\"API rate limit exceeded...\"} body; custom rejection body/status needs a separate." },
      "default-limit": { status: "partial", note: "AI Rate Limiting Advanced 3.14 policy entity supports a matchless fallback policy that caps all requests, but it is opt-in per plugin instance, not a fleet-wide." },
      alerts: { status: "partial", note: "No built-in Slack/webhook/email on cap-hit; you get X-AI-RateLimit-* headers plus Prometheus metrics and must wire Alertmanager yourself." },
      "aws-invoice": { status: "partial", note: "ai-proxy-advanced 3.10 added Bedrock AssumeRole auth but only static credentials/role are documented; no per-request STS session tags (TagSession) reaching CUR." },
      "vertex-invoice": { status: "no", note: "AI Proxy Advanced supports Vertex as a provider but no documented injection of billing labels into generateContent for GCP billing export." },
      "live-changes": { status: "yes", note: "kong reload rotates nginx workers so new config serves while old workers drain in-flight requests; DB-less polls with declarative_config_hash for hot reload." },
      "fleet-gitops": { status: "partial", note: "decK puts gateway config in Git — the closest prior art — but it is an imperative sync CLI, not a reconciler that continuously converges and self-heals drift." },
      "metered-shapes": { status: "partial", note: "ai-proxy log_statistics meters tokens and cost on LLM routes (with stream estimation), but Agent Gateway MCP/A2A shapes get only request and latency metrics." },
      "invoice-true": { status: "partial", note: "Cost = tokens x operator-entered input_cost/output_cost per 1M in ai-proxy model.options; authoritative tokens but no invoice reconciliation or error bound." },
    },
  },
  {
    id: "envoy-ai",
    name: "Envoy AI Gateway",
    kind: "open source",
    url: "https://aigateway.envoyproxy.io",
    ours: false,
    lastVerified: "2026-08-03",
    cells: {
      "enforced-keys": { status: "partial", note: "SecurityPolicy enforces API-key/JWT auth and refuses unauthenticated requests (401), but auth identity is not required to become an attribution tag on every request." },
      "jwt-values": { status: "yes", note: "Envoy Gateway SecurityPolicy validates JWT/OIDC and can extract claims into headers/dynamic metadata that feed AI Gateway attribution and rate-limit descriptors." },
      "static-values": { status: "yes", note: "Operator pins attribution via route/policy config (SecurityPolicy + BackendTrafficPolicy descriptors); pinned values come from gateway config, not caller-supplied." },
      "error-bodies": { status: "yes", note: "BackendTrafficPolicy responseOverride sets custom status/body, and Envoy Gateway v1.8 added source:Local so the override cleanly targets Envoy-generated." },
      "default-limit": { status: "partial", note: "New QuotaPolicy (v1.0) plus token-cost BackendTrafficPolicy give real per-user/per-model token budgets, but every quota is opt-in per backend with no fleet-wide." },
      alerts: { status: "no", note: "Quota/rate-limit breaches surface only as 429s and Prometheus token/latency metrics; no built-in Slack/webhook/email notification ships, so alerting must be wired." },
      "aws-invoice": { status: "no", note: "Bedrock BackendSecurityPolicy uses static creds or OIDC/IRSA AssumeRole with the session name hardcoded to the policy name; it injects no per-request TagSession." },
      "vertex-invoice": { status: "no", note: "AI Gateway routes to Vertex/Gemini but does not document injecting operator-set billing labels into native generateContent, so attribution does not reach GCP." },
      "live-changes": { status: "yes", note: "Config changes propagate via xDS/CRD reconcile and apply to new requests while in-flight HTTP requests drain gracefully (drainTimeout 60s default); known issue." },
      "fleet-gitops": { status: "partial", note: "GitOps via Kubernetes CRDs and an external ArgoCD; genuine for a k8s-only shop, but clusters only and not domain-aware about routes, spend, or attribution." },
      "metered-shapes": { status: "partial", note: "llmRequestCosts meters all LLM shapes incl. SSE via forced stream_options.include_usage, but MCPRoute, images and audio flow spend-unmetered." },
      "invoice-true": { status: "yes", note: "Usage is read from provider response frames (extproc forces stream_options.include_usage); no price table ships anywhere, so no dollar figure is ever fabricated." },
    },
  },
  {
    id: "cloudflare-ai",
    name: "Cloudflare AI Gateway",
    kind: "cloud service",
    url: "https://developers.cloudflare.com/ai-gateway",
    ours: false,
    lastVerified: "2026-08-03",
    cells: {
      "enforced-keys": { status: "partial", note: "Standard spend limits read caller-supplied cf-aig-metadata (trusted, attribution optional); enforced verified identity exists only in the Cloudflare Access." },
      "jwt-values": { status: "partial", note: "Identity-driven budgets (closed beta) derive attribution from a Cloudflare Access verified login (OAuth device-code flow, IdP groups), not caller metadata, but the." },
      "static-values": { status: "partial", note: "cf-aig-metadata is caller-supplied and trusted so pinned values can be overridden, though the Access closed beta lets the operator bind verified identity the caller." },
      "error-bodies": { status: "no", note: "Over-budget requests return a fixed 429 (or dynamic-route fallback to a cheaper model); no config-level custom rejection body or status is documented." },
      "default-limit": { status: "partial", note: "Spend limits are opt-in rules (up to 20 per gateway) scoped by model/provider/custom attribute; a gateway-wide cap must be created, there is no fleet-wide default." },
      alerts: { status: "no", note: "The spend-limits blog states Cloudflare is 'working to add' alerts when a limit is reached; no built-in Slack/webhook/email notification ships today." },
      "aws-invoice": { status: "no", note: "Unified Billing routes through Cloudflare-managed credentials against a Cloudflare credit balance, and BYOK forwards without STS AssumeRole session tags, so no." },
      "vertex-invoice": { status: "no", note: "No operator-set billing labels are injected into native Vertex generateContent; usage is settled via Cloudflare's own account/credits, not GCP billing export." },
      "live-changes": { status: "yes", note: "Config changes (routes, spend limits, provider keys, guardrails) apply instantly from dashboard/API across Cloudflare's edge with no redeploys or downtime, and." },
      "fleet-gitops": { status: "no", note: "Configured through the Cloudflare dashboard and API on Cloudflare-run infrastructure; no self-hosted fleet converging on an operator Git repo." },
      "metered-shapes": { status: "partial", note: "Costs are metered only for endpoints whose responses return token data and the model name; Realtime WebSocket metering undocumented." },
      "invoice-true": { status: "partial", note: "Dashboard dollars are list-price estimates (‘refer to your provider’s dashboard’); only Unified Billing credits, at pass-through rates plus a 5% fee, are the bill." },
    },
  },
  {
    id: "bifrost",
    name: "Bifrost",
    kind: "open source",
    url: "https://docs.getmaxim.ai/bifrost",
    ours: false,
    lastVerified: "2026-08-03",
    cells: {
      "enforced-keys": { status: "yes", note: "enforce_governance_header flag makes the gateway reject unauthenticated/unattributed calls so every request maps to a virtual key, though the flag is opt-in not on." },
      "jwt-values": { status: "partial", note: "OIDC via Okta/Entra with user-level governance exists but is enterprise-gated and claim-to-attribution mapping is not spelled out in public docs." },
      "static-values": { status: "yes", note: "Virtual keys are operator-issued credentials carrying pinned budgets, limits, and hierarchy (customer/team) that the caller supplies by header but cannot redefine." },
      "error-bodies": { status: "partial", note: "Blocked requests return structured 402 (budget) / 429 (rate) bodies with reason and reset window, but the shape is Bifrost-authored, not an operator-defined custom." },
      "default-limit": { status: "partial", note: "Hierarchical budgets at customer/team/VK level are opt-in per entity; no documented fleet-wide default cap auto-applied to every spender." },
      alerts: { status: "yes", note: "Built-in alerting sends budget/failure notifications to Slack, Teams, PagerDuty, email and webhooks via CEL-based governance-scoped rules, not just self-wired." },
      "aws-invoice": { status: "partial", note: "Bedrock integration allows a configurable RoleSessionName but no STS TagSession session tags reaching AWS CUR per request are documented." },
      "vertex-invoice": { status: "no", note: "Vertex provider docs cover auth and request conversion but document no operator-set billing labels injected into generateContent reaching GCP billing export." },
      "live-changes": { status: "partial", note: "Config applies at runtime with no restart (add provider / revoke key take effect on next request) plus gossip-sync cluster mode and zero-downtime deploys, but." },
      "fleet-gitops": { status: "no", note: "A single self-hosted instance; no control plane reconciling a fleet of gateways from desired state in Git." },
      "metered-shapes": { status: "partial", note: "CalculateCost spans every carried shape incl. forced-usage streams, but MCP meters via operator-entered CostPerExecution and unknown models fall to $0 with a debug log." },
      "invoice-true": { status: "partial", note: "Authoritative provider usage, but dollars are Maxim’s datasheet price map (24h sync), no invoice reconciliation; unknown models price at $0." },
    },
  },
  {
    id: "helicone",
    name: "Helicone",
    kind: "open source",
    url: "https://docs.helicone.ai",
    ours: false,
    lastVerified: "2026-08-03",
    cells: {
      "enforced-keys": { status: "no", note: "Helicone-Property-* headers are caller-optional metadata; the gateway never requires attribution nor refuses unattributed requests." },
      "jwt-values": { status: "no", note: "No JWT/OIDC validation or claim-to-attribution mapping; property values are simply whatever the caller puts in headers." },
      "static-values": { status: "no", note: "Custom properties are caller-supplied headers with no operator-pinned per-key/route value the caller cannot override, so nothing is server-assigned." },
      "error-bodies": { status: "no", note: "A blocked request returns a fixed 429 rate-limit error; no config-level custom rejection body or status is documented." },
      "default-limit": { status: "partial", note: "Rate limits are opt-in per request via the Helicone-RateLimit-Policy header (segmentable by property), not a fleet-wide default cap applied to every spender." },
      alerts: { status: "yes", note: "Built-in cost/error/latency/token threshold alerts route to Slack, email, and webhook, but alerting is gated to the Pro plan ($79/mo) and above." },
      "aws-invoice": { status: "no", note: "No propagation of an operator tag into AWS CUR and no STS AssumeRole/TagSession session-tag support; attribution lives only in Helicone's own store." },
      "vertex-invoice": { status: "no", note: "No injection of operator-set billing labels into native Vertex generateContent reaching GCP billing export." },
      "live-changes": { status: "no", note: "Config is loaded from --config config.yaml at startup with no documented file-watch/SIGHUP/hot-reload, and the product has been in maintenance mode since Mintlify's." },
      "fleet-gitops": { status: "no", note: "Primarily an observability layer with a proxy; no fleet-of-gateways GitOps reconciliation model." },
      "metered-shapes": { status: "partial", note: "Chat and embeddings hit the cost registry, but SSE meters only with include_usage, passthrough and Realtime carry unpriced, unknown models log $0." },
      "invoice-true": { status: "partial", note: "Provider usage frames are authoritative, but dollars come from Helicone’s packages/cost registry (‘Best Effort’); no invoice reconciliation, unknowns $0." },
    },
  },
];
