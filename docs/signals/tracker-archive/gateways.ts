/**
 * Data model for the Gateway Baseline tracker (/gateways).
 *
 * The Gateway Baseline is the bar we hold AI gateways to, defined below
 * in CRITERIA: nine checks, coded GB-1 through GB-9 and citable from
 * upstream PRs, that make AI spend easy to govern from the layer that
 * already sees every request. Statuses are hand-verified against public
 * docs on the date recorded per gateway; "unknown" means exactly that,
 * not "no". The live half of the page (upstream reference
 * implementations) is fetched from the GitHub API at request time.
 */

export type SupportStatus = "yes" | "partial" | "no" | "unknown";

export type Criterion = {
  id: string;
  /** Stable check code (GB-1 through GB-9), citable from upstream PRs and linkable as #gb-n. */
  code: string;
  title: string;
  /** One-sentence definition of what counts as a pass. */
  detail: string;
  /** Short column label for the matrix. */
  short: string;
  /**
   * Which side of the bar the check belongs to. Drives the matrix
   * super-headers and group dividers; checks of a side must be
   * contiguous in CRITERIA, and CRITERIA stays in code order.
   */
  side: "control" | "invoice" | "operations";
};

export type CellSupport = {
  status: SupportStatus;
  /** Hedged, sourced-from-docs note. Shown as the cell tooltip and in gap lists. */
  note: string;
};

export type Gateway = {
  id: string;
  name: string;
  kind: "open source" | "commercial" | "cloud service";
  url: string;
  /** ISO date the support row was last verified against public docs. */
  lastVerified: string;
  /** Optional row-level caveat rendered on the gateway card. */
  caveat?: string;
  support: Record<string, CellSupport>;
};

export type TrackedRef = {
  repo: string;
  number: number;
  kind: "pr" | "issue";
  title: string;
  /** Which part of the Baseline this moves, in plain words. */
  moves: string;
  /** The check this ref moves (GB-1..GB-9), set only where the mapping is clean. */
  check?: string;
  /** True when the contribution is ours. */
  ours?: boolean;
};

/* ------------------------------------------------------------------ *
 * The standard
 * ------------------------------------------------------------------ */

export const CRITERIA: readonly Criterion[] = [
  {
    id: "enforced-keys",
    code: "GB-1",
    title: "Every request is tagged with who it is for",
    detail:
      "The operator declares the attribution keys every request must be tagged with, as headers or equivalent request metadata, and the gateway MUST reject any request it cannot attribute to a spender.",
    short: "Enforced keys",
    side: "control",
  },
  {
    id: "jwt-values",
    code: "GB-2",
    title: "The tag can be read from a verified login",
    detail:
      "For callers that log in, the tag MUST be derivable from claims on a JWT the gateway itself validates, so it is proven from the login rather than self-reported and taken on trust. This is the proven mode: a different caller yields a different tag, checked every request.",
    short: "From JWT",
    side: "control",
  },
  {
    id: "static-values",
    code: "GB-3",
    title: "The tag can be assigned",
    detail:
      "For callers that do not log in, the operator MUST be able to pin the tag to an app, a route, or the key issued to it, so the value is decided by the operator and nothing the caller sends can change it. This is the assigned mode. Together with GB-2, every tag has exactly two possible origins: proven from a verified login, or assigned by you. The gateway never just believes a tag the caller sent.",
    short: "Assigned",
    side: "control",
  },
  {
    id: "error-bodies",
    code: "GB-4",
    title: "A blocked request says why, in your words",
    detail:
      "When the gateway rejects a request, for missing attribution or for a hit limit, the response MUST carry an operator-defined error body, not a generic 4xx. This is what makes the hard rejections in GB-1 and GB-5 livable: a bare 429 reads as an outage and becomes an incident, while a rejection in your words reads as policy, telling the developer what happened and what to do next.",
    short: "Rejections",
    side: "control",
  },
  {
    id: "default-limit",
    code: "GB-5",
    title: "Every spender gets a cap by default",
    detail:
      "A fleet-wide default spend limit MUST apply to every value of the operator's attribution keys, whether that value is an app, a team, a user, or a customer, from its first request, with overrides for specific values. Denominated in tokens, cost, or a budget, not only requests.",
    short: "Spend limits",
    side: "control",
  },
  {
    id: "alerts",
    code: "GB-6",
    title: "Someone is told when a cap is hit",
    detail:
      "Hitting a limit MUST notify someone, from the same layer that enforced it, without wiring a separate monitoring stack first.",
    short: "Alerts",
    side: "control",
  },
  {
    id: "aws-invoice",
    code: "GB-7",
    title: "The tag reaches the AWS bill",
    detail:
      "For AWS Bedrock, the attribution value MUST ride to the provider's own invoice, as session tags or the role session name landing in the Cost and Usage Report, and it MUST be set or derived by the operator rather than supplied by the caller. Finance reconciles against the bill itself, not the gateway's spend estimate.",
    short: "AWS invoice",
    side: "invoice",
  },
  {
    id: "vertex-invoice",
    code: "GB-8",
    title: "The tag reaches the Vertex bill",
    detail:
      "For Google Vertex AI, the attribution value MUST ride to the provider's own invoice, as billing labels on the native request landing in the GCP billing export, and it MUST be set or derived by the operator rather than supplied by the caller. Finance reconciles against the bill itself, not the gateway's spend estimate.",
    short: "Vertex invoice",
    side: "invoice",
  },
  {
    id: "live-changes",
    code: "GB-9",
    title: "The rules can change while it runs",
    detail:
      "Caps, tags, and rejection messages change weekly in a real fleet, so a policy change MUST apply to a running gateway without a redeploy and without dropping in-flight requests. Streams already running MAY finish under the old rules, and the gateway MUST state its bound on how long old rules can linger, because instant enforcement over streaming traffic is a promise nobody can keep. This check is aspirational today: it is published as an invitation to build toward, together, not as a verdict.",
    short: "Live changes",
    side: "operations",
  },

] as const;

/* ------------------------------------------------------------------ *
 * Spec history: every change to the bar, dated, newest first.
 * The whole point of the site is dated artifacts; the bar itself is
 * one, so its changes are recorded the same way article revisions are.
 * ------------------------------------------------------------------ */

export type SpecChange = {
  /** ISO date the change shipped. */
  date: string;
  /** Terse release-note style summary of what changed and why. */
  note: string;
};

export const SPEC_CHANGELOG: readonly SpecChange[] = [
  {
    date: "2026-08-03",
    note: "Second full verification pass: all 81 cells re-checked against current vendor docs, GB-9 resolved for every gateway, and our own reference row added — scored from the code against the same bar, GB-2 marked partial where it is deferred by judgment. Notable movement since July: LiteLLM's AWS invoice check rose from no to partial (PR #32797 merged), and two Cloudflare cells were corrected downward on re-read.",
  },
  {
    date: "2026-07-14",
    note: "GB-9, live policy changes, added as the ninth check. Aspirational by design: all of its cells enter as not verified until the next full documentation pass.",
  },
  {
    date: "2026-07-13",
    note: "GB-7 and GB-8 tightened: the value on the bill must be operator-set; labels a caller merely passes through score partial.",
  },
  {
    date: "2026-07-13",
    note: "GB-7 and GB-8 added, invoice-grade attribution on AWS and Vertex, one check per cloud; all 64 cells verified against public documentation.",
  },
  {
    date: "2026-07-13",
    note: "Version 1: the bar named the Gateway Baseline, checks coded GB-1 through GB-6 and sharpened, every cell verified against live documentation.",
  },
] as const;

/* ------------------------------------------------------------------ *
 * The matrix
 * ------------------------------------------------------------------ */

export const GATEWAYS: readonly Gateway[] = [
  {
    id: "agentgateway",
    name: "agentgateway",
    kind: "open source",
    url: "https://agentgateway.dev",
    lastVerified: "2026-08-03",
    support: {
      "enforced-keys": {
        status: "yes",
        note: "CEL-based auth/RBAC policies require and evaluate attribution per request; unattributed traffic is refused, not caller-optional.",
      },
      "jwt-values": {
        status: "yes",
        note: "Validated JWT/OIDC claims (jwt.sub, custom claims) map into attribution and downstream CEL descriptors and session tags.",
      },
      "static-values": {
        status: "yes",
        note: "Operator-pinned values are set via CEL on key/route/backend and evaluated server-side, so the caller cannot override the pinned attribution.",
      },
      "error-bodies": {
        status: "partial",
        note: "Rate-limit rejections return a fixed 429 'rate limit exceeded'; only the budget ImmediateResponse status is being made configurable, no operator-worded custom body.",
      },
      "default-limit": {
        status: "partial",
        note: "RateLimit/budget policies attach opt-in per Gateway/HTTPRoute and count per-instance; no fleet-wide default cap auto-applied to every attribution value.",
      },
      alerts: {
        status: "no",
        note: "Budget/rate-limit state surfaces only as Prometheus metrics; no built-in Slack/webhook/email notification when a cap is hit.",
      },
      "live-changes": {
        status: "yes",
        note: "Local config hot-reloads via file-watch into the shared IR and xDS pushes are zero-downtime; kgateway v2.1.0 control plane adds graceful shutdown/zero-downtime proxy.",
      },
      "aws-invoice": {
        status: "yes",
        note: "AwsSessionTag {key,value,expression} sets STS TagSession tags via per-request CEL (jwt.sub etc.) that reach the AWS CUR, caller cannot forge; PRs #2435/#2447 merged.",
      },
      "vertex-invoice": {
        status: "no",
        note: "Native generateContent labels (#2023) are caller pass-through per issue #2490; the operator-set labels knob is PR #2806, still OPEN, so Vertex attribution is not yet.",
      },
    },
  },
  {
    id: "litellm",
    name: "LiteLLM",
    kind: "open source",
    url: "https://docs.litellm.ai",
    lastVerified: "2026-08-03",
    support: {
      "enforced-keys": {
        status: "partial",
        note: "enforced_params (e.g. require user/metadata.generation_name, rejects missing with 'please pass param=user') exists but is an Enterprise-gated feature, not free-tier.",
      },
      "jwt-values": {
        status: "partial",
        note: "OIDC/JWT claims map to attribution via JWT-to-virtual-key mapping, but docs state 'JWT → Virtual Key Mapping is an Enterprise feature.'",
      },
      "static-values": {
        status: "yes",
        note: "Tags/metadata are operator-pinned per virtual key/team, and 'Reject Client-Side Metadata Tags' lets the operator refuse caller-supplied tag overrides.",
      },
      "error-bodies": {
        status: "partial",
        note: "Custom rejection message/status is settable via a ProxyException in a custom_auth/guardrail Python hook, not a pure config-level rejection body.",
      },
      "default-limit": {
        status: "yes",
        note: "Fleet-wide default caps exist via litellm_settings max_internal_user_budget and max_end_user_budget_id, applied to users/end-users by default, not per-key opt-in only.",
      },
      alerts: {
        status: "yes",
        note: "Built-in Slack/webhook/email budget and threshold alerting ships in the proxy, not merely Prometheus metrics you wire yourself.",
      },
      "live-changes": {
        status: "partial",
        note: "With store_model_in_db, pods poll and converge on config changes within proxy_config_reload_interval_seconds (default 30s), but a process restart drops in-flight.",
      },
      "aws-invoice": {
        status: "partial",
        note: "PR #32797 (aws_session_tags on STS AssumeRole) MERGED 2026-07-16, so operator-set tags now reach AWS CUR and caller cannot forge, but tags are config-pinned per.",
      },
      "vertex-invoice": {
        status: "partial",
        note: "LiteLLM forwards a labels field (and converts string metadata to labels) into Vertex generateContent for GCP billing, but docs show no server-side label pinning so.",
      },
    },
  },
  {
    id: "portkey",
    name: "Portkey",
    kind: "commercial",
    url: "https://portkey.ai/docs",
    lastVerified: "2026-08-03",
    support: {
      "enforced-keys": {
        status: "partial",
        note: "Owners can define mandatory metadata fields and requests that omit/mismatch them are rejected before forwarding, but required-metadata enforcement is an Enterprise-only.",
      },
      "jwt-values": {
        status: "partial",
        note: "JWT/OIDC validation (JWKS/introspection, requiredClaims, claim matching) with a claims_header forwarding sub/email/workspace_id exists but is documented on the MCP.",
      },
      "static-values": {
        status: "yes",
        note: "Metadata precedence is Workspace (highest) then API-key then Request (lowest), so operator-pinned workspace/key values outrank and cannot be overridden by the caller.",
      },
      "error-bodies": {
        status: "no",
        note: "Guardrail denials return a hardcoded 446 status with no documented custom status code or operator-defined rejection body.",
      },
      "default-limit": {
        status: "partial",
        note: "Budget limits (USD or token-based) are set per virtual key and expire the key when hit, but must be explicitly configured per key with no fleet-wide default and are.",
      },
      alerts: {
        status: "partial",
        note: "Budget limits support email notifications at configurable alert thresholds, but no Slack or webhook alerting is documented and the feature is Enterprise-gated.",
      },
      "live-changes": {
        status: "yes",
        note: "Gateway configs are referenced by ID and edited in the UI take effect on the next request with no commits or redeploys, and configs are resolved per-request so.",
      },
      "aws-invoice": {
        status: "no",
        note: "Bedrock integration uses an Assumed Role ARN for access only; no documentation of Portkey injecting per-request STS session tags (TagSession) so operator metadata.",
      },
      "vertex-invoice": {
        status: "yes",
        note: "Portkey request metadata is forwarded as Vertex AI resource labels into native calls (enterprise changelog notes the fix for mislabeled request types), reaching GCP.",
      },
    },
  },
  {
    id: "kong",
    name: "Kong AI Gateway",
    kind: "commercial",
    url: "https://developer.konghq.com",
    lastVerified: "2026-08-03",
    support: {
      "enforced-keys": {
        status: "partial",
        note: "OIDC/key-auth can require a consumer and reject anonymous, but attribution enforcement is per-route plugin config, not a gateway-wide refusal of unattributed traffic.",
      },
      "jwt-values": {
        status: "partial",
        note: "OpenID Connect plugin's config.consumer_claim maps validated JWT/OIDC claims to a Kong consumer (id/username/custom_id), but full OIDC is Kong Enterprise-gated.",
      },
      "static-values": {
        status: "yes",
        note: "Operators pin consumer/credential per key and AI Rate Limiting policies match on operator-set consumer identifiers, never a caller-supplied value.",
      },
      "error-bodies": {
        status: "partial",
        note: "AI Rate Limiting Advanced returns a fixed 429 with a canned {\"message\":\"API rate limit exceeded...\"} body; custom rejection body/status needs a separate.",
      },
      "default-limit": {
        status: "partial",
        note: "AI Rate Limiting Advanced 3.14 policy entity supports a matchless fallback policy that caps all requests, but it is opt-in per plugin instance, not a fleet-wide default.",
      },
      alerts: {
        status: "partial",
        note: "No built-in Slack/webhook/email on cap-hit; you get X-AI-RateLimit-* headers plus Prometheus metrics and must wire Alertmanager yourself.",
      },
      "live-changes": {
        status: "yes",
        note: "kong reload rotates nginx workers so new config serves while old workers drain in-flight requests; DB-less polls with declarative_config_hash for hot reload.",
      },
      "aws-invoice": {
        status: "partial",
        note: "ai-proxy-advanced 3.10 added Bedrock AssumeRole auth but only static credentials/role are documented; no per-request STS session tags (TagSession) reaching CUR.",
      },
      "vertex-invoice": {
        status: "no",
        note: "AI Proxy Advanced supports Vertex as a provider but no documented injection of billing labels into generateContent for GCP billing export.",
      },
    },
  },
  {
    id: "envoy-ai",
    name: "Envoy AI Gateway",
    kind: "open source",
    url: "https://aigateway.envoyproxy.io",
    lastVerified: "2026-08-03",
    support: {
      "enforced-keys": {
        status: "partial",
        note: "SecurityPolicy enforces API-key/JWT auth and refuses unauthenticated requests (401), but auth identity is not required to become an attribution tag on every request.",
      },
      "jwt-values": {
        status: "yes",
        note: "Envoy Gateway SecurityPolicy validates JWT/OIDC and can extract claims into headers/dynamic metadata that feed AI Gateway attribution and rate-limit descriptors.",
      },
      "static-values": {
        status: "yes",
        note: "Operator pins attribution via route/policy config (SecurityPolicy + BackendTrafficPolicy descriptors); pinned values come from gateway config, not caller-supplied.",
      },
      "error-bodies": {
        status: "yes",
        note: "BackendTrafficPolicy responseOverride sets custom status/body, and Envoy Gateway v1.8 added source:Local so the override cleanly targets Envoy-generated rate-limit/auth.",
      },
      "default-limit": {
        status: "partial",
        note: "New QuotaPolicy (v1.0) plus token-cost BackendTrafficPolicy give real per-user/per-model token budgets, but every quota is opt-in per backend with no fleet-wide default.",
      },
      alerts: {
        status: "no",
        note: "Quota/rate-limit breaches surface only as 429s and Prometheus token/latency metrics; no built-in Slack/webhook/email notification ships, so alerting must be wired.",
      },
      "live-changes": {
        status: "yes",
        note: "Config changes propagate via xDS/CRD reconcile and apply to new requests while in-flight HTTP requests drain gracefully (drainTimeout 60s default); known issue #8889.",
      },
      "aws-invoice": {
        status: "no",
        note: "Bedrock BackendSecurityPolicy uses static creds or OIDC/IRSA AssumeRole with the session name hardcoded to the policy name; it injects no per-request TagSession.",
      },
      "vertex-invoice": {
        status: "no",
        note: "AI Gateway routes to Vertex/Gemini but does not document injecting operator-set billing labels into native generateContent, so attribution does not reach GCP billing.",
      },
    },
  },
  {
    id: "cloudflare-ai",
    name: "Cloudflare AI Gateway",
    kind: "cloud service",
    url: "https://developers.cloudflare.com/ai-gateway/",
    lastVerified: "2026-08-03",
    support: {
      "enforced-keys": {
        status: "partial",
        note: "Standard spend limits read caller-supplied cf-aig-metadata (trusted, attribution optional); enforced verified identity exists only in the Cloudflare Access.",
      },
      "jwt-values": {
        status: "partial",
        note: "Identity-driven budgets (closed beta) derive attribution from a Cloudflare Access verified login (OAuth device-code flow, IdP groups), not caller metadata, but the.",
      },
      "static-values": {
        status: "partial",
        note: "cf-aig-metadata is caller-supplied and trusted so pinned values can be overridden, though the Access closed beta lets the operator bind verified identity the caller.",
      },
      "error-bodies": {
        status: "no",
        note: "Over-budget requests return a fixed 429 (or dynamic-route fallback to a cheaper model); no config-level custom rejection body or status is documented.",
      },
      "default-limit": {
        status: "partial",
        note: "Spend limits are opt-in rules (up to 20 per gateway) scoped by model/provider/custom attribute; a gateway-wide cap must be created, there is no fleet-wide default.",
      },
      alerts: {
        status: "no",
        note: "The spend-limits blog states Cloudflare is 'working to add' alerts when a limit is reached; no built-in Slack/webhook/email notification ships today.",
      },
      "live-changes": {
        status: "yes",
        note: "Config changes (routes, spend limits, provider keys, guardrails) apply instantly from dashboard/API across Cloudflare's edge with no redeploys or downtime, and.",
      },
      "aws-invoice": {
        status: "no",
        note: "Unified Billing routes through Cloudflare-managed credentials against a Cloudflare credit balance, and BYOK forwards without STS AssumeRole session tags, so no operator.",
      },
      "vertex-invoice": {
        status: "no",
        note: "No operator-set billing labels are injected into native Vertex generateContent; usage is settled via Cloudflare's own account/credits, not GCP billing export.",
      },
    },
  },
  {
    id: "bifrost",
    name: "Bifrost",
    kind: "open source",
    url: "https://github.com/maximhq/bifrost",
    lastVerified: "2026-08-03",
    support: {
      "enforced-keys": {
        status: "yes",
        note: "enforce_governance_header flag makes the gateway reject unauthenticated/unattributed calls so every request maps to a virtual key, though the flag is opt-in not on by.",
      },
      "jwt-values": {
        status: "partial",
        note: "OIDC via Okta/Entra with user-level governance exists but is enterprise-gated and claim-to-attribution mapping is not spelled out in public docs.",
      },
      "static-values": {
        status: "yes",
        note: "Virtual keys are operator-issued credentials carrying pinned budgets, limits, and hierarchy (customer/team) that the caller supplies by header but cannot redefine.",
      },
      "error-bodies": {
        status: "partial",
        note: "Blocked requests return structured 402 (budget) / 429 (rate) bodies with reason and reset window, but the shape is Bifrost-authored, not an operator-defined custom.",
      },
      "default-limit": {
        status: "partial",
        note: "Hierarchical budgets at customer/team/VK level are opt-in per entity; no documented fleet-wide default cap auto-applied to every spender.",
      },
      alerts: {
        status: "yes",
        note: "Built-in alerting sends budget/failure notifications to Slack, Teams, PagerDuty, email and webhooks via CEL-based governance-scoped rules, not just self-wired Prometheus.",
      },
      "live-changes": {
        status: "partial",
        note: "Config applies at runtime with no restart (add provider / revoke key take effect on next request) plus gossip-sync cluster mode and zero-downtime deploys, but in-flight.",
      },
      "aws-invoice": {
        status: "partial",
        note: "Bedrock integration allows a configurable RoleSessionName but no STS TagSession session tags reaching AWS CUR per request are documented.",
      },
      "vertex-invoice": {
        status: "no",
        note: "Vertex provider docs cover auth and request conversion but document no operator-set billing labels injected into generateContent reaching GCP billing export.",
      },
    },
  },
  {
    id: "helicone",
    name: "Helicone",
    kind: "commercial",
    url: "https://docs.helicone.ai",
    lastVerified: "2026-08-03",
    support: {
      "enforced-keys": {
        status: "no",
        note: "Helicone-Property-* headers are caller-optional metadata; the gateway never requires attribution nor refuses unattributed requests.",
      },
      "jwt-values": {
        status: "no",
        note: "No JWT/OIDC validation or claim-to-attribution mapping; property values are simply whatever the caller puts in headers.",
      },
      "static-values": {
        status: "no",
        note: "Custom properties are caller-supplied headers with no operator-pinned per-key/route value the caller cannot override, so nothing is server-assigned.",
      },
      "error-bodies": {
        status: "no",
        note: "A blocked request returns a fixed 429 rate-limit error; no config-level custom rejection body or status is documented.",
      },
      "default-limit": {
        status: "partial",
        note: "Rate limits are opt-in per request via the Helicone-RateLimit-Policy header (segmentable by property), not a fleet-wide default cap applied to every spender.",
      },
      alerts: {
        status: "yes",
        note: "Built-in cost/error/latency/token threshold alerts route to Slack, email, and webhook, but alerting is gated to the Pro plan ($79/mo) and above.",
      },
      "live-changes": {
        status: "no",
        note: "Config is loaded from --config config.yaml at startup with no documented file-watch/SIGHUP/hot-reload, and the product has been in maintenance mode since Mintlify's.",
      },
      "aws-invoice": {
        status: "no",
        note: "No propagation of an operator tag into AWS CUR and no STS AssumeRole/TagSession session-tag support; attribution lives only in Helicone's own store.",
      },
      "vertex-invoice": {
        status: "no",
        note: "No injection of operator-set billing labels into native Vertex generateContent reaching GCP billing export.",
      },
    },
  },
  {
    id: "the-gateway-baseline",
    name: "The Gateway Baseline",
    kind: "open source",
    url: "https://thegatewaybaseline.com",
    lastVerified: "2026-08-03",
    caveat:
      "Our own reference implementation, scored from the code against the same bar. GB-2 is built but deferred by judgment and may never ship as a promised capability; it is marked partial, not inflated.",
    support: {
      "enforced-keys": {
        status: "yes",
        note: "fleet.attribution.required_keys refuses unattributed requests at the door with the operator template; proven by gb1_missing_required_key_rejected against the spawned.",
      },
      "jwt-values": {
        status: "partial",
        note: "HS256 claim->key mapping is built and tested (gb2_claim_mapped_key_proven_from_verified_jwt) but deferred by judgment as lowest-priority; RS256/JWKS unbuilt, may never.",
      },
      "static-values": {
        status: "yes",
        note: "Operator-pinned attribution: caller headers for pinned keys are stripped then re-inserted with adjudicated values, never believed (gb3_pinned_key_overwrites_caller_value).",
      },
      "error-bodies": {
        status: "yes",
        note: "Per-provider/route operator rejection templates including streaming terminal events and a dedicated cap-exceeded voice, scoped down the chain (gb4 conformance tests).",
      },
      "default-limit": {
        status: "yes",
        note: "Fleet-default spend_caps per attribution value (budget.rs composed fleet default), budget shares with bounded overspend under partition, per-value overrides.",
      },
      alerts: {
        status: "yes",
        note: "GB-6 soft-80% and hard-cap alerts fire from the enforcement layer to a validated webhook sink at operator alert_at (alerts_fire_from_the_meter_at_soft_then_hard).",
      },
      "aws-invoice": {
        status: "yes",
        note: "SigV4 AssumeRole with attribution-derived STS session tags reaches CUR, operator-set and caller-stripped; raw caller session tag rejected at config load (gb7 tests).",
      },
      "vertex-invoice": {
        status: "yes",
        note: "Operator billing labels merged into the native generateContent body before signing, operator wins on conflict so callers cannot override (labels.rs, gb8 tests).",
      },
      "live-changes": {
        status: "yes",
        note: "Phase-4 versioned snapshot hot-swap: each request pins its Arc<Snapshot> for life, old versions drain with last in-flight stream, stated bounded staleness.",
      },
    },
  },
] as const;

/* ------------------------------------------------------------------ *
 * Upstream signals, resolved live against the GitHub API.
 * ------------------------------------------------------------------ */

export const TRACKED_REFS: readonly TrackedRef[] = [
  {
    repo: "agentgateway/agentgateway",
    number: 2435,
    kind: "pr",
    title: "Session tags on Bedrock routes",
    moves: "Operator-set tags riding to the AWS invoice",
    check: "GB-7",
    ours: true,
  },
  {
    repo: "agentgateway/agentgateway",
    number: 2447,
    kind: "pr",
    title: "Per-request app and team values on cloud credentials",
    moves: "Per-request tags on AWS credentials, fresh for every caller",
    check: "GB-7",
    ours: true,
  },
  {
    repo: "BerriAI/litellm",
    number: 32797,
    kind: "pr",
    title: "Session tags for Bedrock AssumeRole paths",
    moves: "Operator-set tags riding to the AWS invoice",
    check: "GB-7",
    ours: true,
  },
  {
    repo: "Portkey-AI/gateway",
    number: 1728,
    kind: "pr",
    title: "Session tags on Bedrock credentials",
    moves: "Operator-set tags riding to the AWS invoice",
    check: "GB-7",
    ours: true,
  },
  {
    repo: "BerriAI/litellm",
    number: 13692,
    kind: "issue",
    title: "Vertex AI label passthrough",
    moves: "Billing labels riding to the Vertex invoice",
    check: "GB-8",
  },
] as const;

/* ------------------------------------------------------------------ *
 * Scoring
 * ------------------------------------------------------------------ */

export type Tally = {
  total: number;
  green: number;
  partial: number;
  missing: number;
  unknown: number;
  /** Cells that still stand between here and the Baseline. */
  remaining: number;
};

export function tallyMatrix(): Tally {
  let green = 0;
  let partial = 0;
  let missing = 0;
  let unknown = 0;
  for (const gateway of GATEWAYS) {
    for (const criterion of CRITERIA) {
      const cell = gateway.support[criterion.id];
      if (!cell) continue;
      if (cell.status === "yes") green += 1;
      else if (cell.status === "partial") partial += 1;
      else if (cell.status === "no") missing += 1;
      else unknown += 1;
    }
  }
  const total = GATEWAYS.length * CRITERIA.length;
  return { total, green, partial, missing, unknown, remaining: total - green };
}

export function gatewayScore(gateway: Gateway): number {
  return CRITERIA.filter((c) => gateway.support[c.id]?.status === "yes").length;
}

/**
 * Highest score any gateway currently reaches. A gateway at this score is
 * a current leader; when nothing has cleared the full bar, the leaders
 * are simply whoever is closest. Recomputed from the matrix, so the
 * "leading" treatment moves on its own as statuses change.
 */
export function topScore(): number {
  return Math.max(...GATEWAYS.map(gatewayScore));
}

export function criterionAdoption(criterionId: string): number {
  return GATEWAYS.filter((g) => g.support[criterionId]?.status === "yes")
    .length;
}
