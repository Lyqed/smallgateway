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
    lastVerified: "2026-07-13",
    support: {
      "enforced-keys": {
        status: "yes",
        note: "Strict apiKey policy rejects unkeyed requests; per-key metadata carries attribution; CEL require rules enforce headers.",
      },
      "jwt-values": {
        status: "yes",
        note: "Built-in jwtAuth validates issuer, audience, and JWKS; claims feed headers, rate-limit descriptors, and logs via CEL.",
      },
      "static-values": {
        status: "yes",
        note: "requestHeaderModifier sets static request headers per route or backend; transformations add literal values too.",
      },
      "error-bodies": {
        status: "partial",
        note: "Conditional directResponse returns operator-defined bodies for CEL-matched rejections; the rate-limit 429 body is not customizable.",
      },
      "default-limit": {
        status: "partial",
        note: "Token and cost budgets are native; per-app defaults need remoteRateLimit descriptors plus an external rate limit server.",
      },
      alerts: {
        status: "no",
        note: "Docs route budget alerts through Prometheus rules; no built-in Slack, webhook, or email notifier.",
      },
      "live-changes": {
        status: "unknown",
        note: "Not verified yet; GB-9 entered the bar on 14 July 2026 and awaits the next full documentation pass.",
      },
      "aws-invoice": {
        status: "yes",
        note: "Bedrock AssumeRole session tags and RoleSessionName reach the CUR, per request via CEL; PRs #2435 and #2447 merged.",
      },
      "vertex-invoice": {
        status: "no",
        note: "Translated Gemini hits the OpenAI-compat endpoint that drops labels; native generateContent (PR #2023) is unmerged.",
      },
    },
  },
  {
    id: "litellm",
    name: "LiteLLM",
    kind: "open source",
    url: "https://docs.litellm.ai",
    lastVerified: "2026-07-13",
    support: {
      "enforced-keys": {
        status: "partial",
        note: "enforced_params rejects requests missing required params like user; runtime gates it to enterprise.",
      },
      "jwt-values": {
        status: "partial",
        note: "JWT auth validates OIDC tokens and maps claims to user, team, and org spend; enterprise feature.",
      },
      "static-values": {
        status: "yes",
        note: "Tags set on a key or team apply to every request for spend attribution.",
      },
      "error-bodies": {
        status: "partial",
        note: "Custom Python call hooks can reject with operator-defined messages; no config-level error bodies.",
      },
      "default-limit": {
        status: "yes",
        note: "USD max_budget with fleet defaults via max_internal_user_budget; per-key overrides.",
      },
      alerts: {
        status: "yes",
        note: "budget_alerts notify Slack or webhooks at thresholds and when a budget is crossed.",
      },
      "live-changes": {
        status: "unknown",
        note: "Not verified yet; GB-9 entered the bar on 14 July 2026 and awaits the next full documentation pass.",
      },
      "aws-invoice": {
        status: "no",
        note: "Bedrock session tags reach the CUR only via PR #32797, still unmerged, and would be static per-deployment and caller-overridable.",
      },
      "vertex-invoice": {
        status: "partial",
        note: "Config litellm_params can pin labels per deployment, but caller-supplied labels override the pinned value.",
      },
    },
  },
  {
    id: "portkey",
    name: "Portkey",
    kind: "commercial",
    url: "https://portkey.ai/docs",
    lastVerified: "2026-07-13",
    support: {
      "enforced-keys": {
        status: "partial",
        note: "Metadata schemas enforce required keys at API key and workspace creation, not per request.",
      },
      "jwt-values": {
        status: "partial",
        note: "JWKS-validated JWT auth maps claims and default metadata onto requests; enterprise plan only.",
      },
      "static-values": {
        status: "yes",
        note: "Default metadata pinned per API key or workspace; admin defaults override request values.",
      },
      "error-bodies": {
        status: "no",
        note: "Guardrail denials return fixed 446, limit hits 412 or 429; no custom error bodies documented.",
      },
      "default-limit": {
        status: "partial",
        note: "Cost or token budgets per key, workspace, or provider; no fleet-wide default; enterprise and select Pro.",
      },
      alerts: {
        status: "partial",
        note: "Budget alert thresholds send email notifications; underlying limits gated to enterprise and select Pro.",
      },
      "live-changes": {
        status: "unknown",
        note: "Not verified yet; GB-9 entered the bar on 14 July 2026 and awaits the next full documentation pass.",
      },
      "aws-invoice": {
        status: "no",
        note: "Bedrock AssumeRole sends RoleArn and ExternalId only, no TagSession; the RoleSessionName is a static buggy date. PR #1728 unmerged.",
      },
      "vertex-invoice": {
        status: "yes",
        note: "Admin-attached default configs with override_params pin labels into native generateContent; OSS alone stays caller-supplied.",
      },
    },
  },
  {
    id: "kong",
    name: "Kong AI Gateway",
    kind: "commercial",
    url: "https://developer.konghq.com",
    lastVerified: "2026-07-13",
    support: {
      "enforced-keys": {
        status: "partial",
        note: "request-validator rejects requests missing required headers, but enterprise; OSS auth plugins enforce only consumer identity.",
      },
      "jwt-values": {
        status: "partial",
        note: "The jwt plugin validates tokens and sets consumer headers; claim-to-header mapping needs enterprise openid-connect.",
      },
      "static-values": {
        status: "yes",
        note: "request-transformer adds static headers per route, service, or consumer.",
      },
      "error-bodies": {
        status: "yes",
        note: "rate-limiting exposes error_code and error_message; exit-transformer rewrites other gateway errors, enterprise only.",
      },
      "default-limit": {
        status: "partial",
        note: "ai-rate-limiting-advanced sets token or cost limits with fallback defaults and per-consumer overrides; enterprise only.",
      },
      alerts: {
        status: "partial",
        note: "Event hooks send webhooks, Slack included, on rate-limit-exceeded; enterprise feature.",
      },
      "live-changes": {
        status: "unknown",
        note: "Not verified yet; GB-9 entered the bar on 14 July 2026 and awaits the next full documentation pass.",
      },
      "aws-invoice": {
        status: "partial",
        note: "aws_role_session_name sets a static RoleSessionName that reaches the CUR; no STS session tags, and not per-consumer.",
      },
      "vertex-invoice": {
        status: "no",
        note: "No labels parameter for Vertex generateContent; only undocumented request-transformer body mutation.",
      },
    },
  },
  {
    id: "envoy-ai",
    name: "Envoy AI Gateway",
    kind: "open source",
    url: "https://aigateway.envoyproxy.io",
    lastVerified: "2026-07-13",
    support: {
      "enforced-keys": {
        status: "partial",
        note: "JWT authn and claim authorization can gate; header-keyed limits pass unmatched requests unattributed.",
      },
      "jwt-values": {
        status: "yes",
        note: "SecurityPolicy JWT provider claimToHeaders maps validated claims to request headers.",
      },
      "static-values": {
        status: "yes",
        note: "headerMutation pins static headers per AIGatewayRoute rule or AIServiceBackend.",
      },
      "error-bodies": {
        status: "yes",
        note: "BackendTrafficPolicy responseOverride customizes local replies, including 429 rate-limit rejections.",
      },
      "default-limit": {
        status: "partial",
        note: "QuotaPolicy token budgets with defaultBucket and per-tenant bucketRules; per-model only, serviceQuota unenforced.",
      },
      alerts: {
        status: "no",
        note: "Prometheus and OpenTelemetry metrics only; no built-in notifications.",
      },
      "live-changes": {
        status: "unknown",
        note: "Not verified yet; GB-9 entered the bar on 14 July 2026 and awaits the next full documentation pass.",
      },
      "aws-invoice": {
        status: "no",
        note: "AssumeRole session name is hardcoded to the policy name; no session tags or per-request attribution to the CUR.",
      },
      "vertex-invoice": {
        status: "no",
        note: "Only safetySettings, thinking, and google_search extension fields; no billing labels injected into generateContent.",
      },
    },
  },
  {
    id: "cloudflare-ai",
    name: "Cloudflare AI Gateway",
    kind: "cloud service",
    url: "https://developers.cloudflare.com/ai-gateway/",
    lastVerified: "2026-07-13",
    support: {
      "enforced-keys": {
        status: "partial",
        note: "No required-metadata setting; Dynamic Routing conditionals can dead-end requests lacking metadata, assembled per gateway.",
      },
      "jwt-values": {
        status: "no",
        note: "Authenticated Gateway checks Cloudflare API tokens; no JWT claim mapping at the gateway.",
      },
      "static-values": {
        status: "partial",
        note: "cf-aig-metadata is caller-supplied and trusted; per-app gateways give coarse attribution, not operator-pinned values.",
      },
      "error-bodies": {
        status: "no",
        note: "Rate and spend limits return a fixed 429; no operator-defined rejection bodies documented.",
      },
      "default-limit": {
        status: "yes",
        note: "Spend limits split dollar budgets per metadata value automatically; filter rules add per-app caps.",
      },
      alerts: {
        status: "no",
        note: "No AI Gateway notification types; spend limit alerts described as planned, not shipped.",
      },
      "live-changes": {
        status: "unknown",
        note: "Not verified yet; GB-9 entered the bar on 14 July 2026 and awaits the next full documentation pass.",
      },
      "aws-invoice": {
        status: "no",
        note: "A BYOK SigV4 signing proxy; the gateway does no AssumeRole tagging. Cloudflare spend tracking is its own ledger, not the CUR.",
      },
      "vertex-invoice": {
        status: "no",
        note: "Proxies native generateContent but injects no billing labels to GCP export; Cloudflare spend is its own ledger.",
      },
    },
  },
  {
    id: "bifrost",
    name: "Bifrost",
    kind: "open source",
    url: "https://github.com/maximhq/bifrost",
    lastVerified: "2026-07-13",
    support: {
      "enforced-keys": {
        status: "yes",
        note: "Global required_headers rejects untagged requests; enforce_auth_on_inference mandates virtual keys per request.",
      },
      "jwt-values": {
        status: "partial",
        note: "IdP JWT validation with identity stamping on /mcp only, enterprise; no claim-derived inference attribution documented.",
      },
      "static-values": {
        status: "yes",
        note: "Virtual keys pin team and customer attribution per app; x-bf-customer-id scoping is enterprise.",
      },
      "error-bodies": {
        status: "partial",
        note: "Fixed typed JSON rejection bodies; customization only through custom Go plugins, not first-class config.",
      },
      "default-limit": {
        status: "partial",
        note: "Spend budgets per key, team, and customer; no fleet-wide default; enterprise access profiles add role defaults.",
      },
      alerts: {
        status: "partial",
        note: "Enterprise page claims budget alerts to Slack, email, and PagerDuty; absent from open docs.",
      },
      "live-changes": {
        status: "unknown",
        note: "Not verified yet; GB-9 entered the bar on 14 July 2026 and awaits the next full documentation pass.",
      },
      "aws-invoice": {
        status: "partial",
        note: "Bedrock AssumeRole sets a configurable RoleSessionName that reaches the CUR, but no session tags, and static per key.",
      },
      "vertex-invoice": {
        status: "no",
        note: "VertexKeyConfig has no billing-labels field; generateContent gets no GCP labels, only Bifrost's own telemetry.",
      },
    },
  },
  {
    id: "helicone",
    name: "Helicone",
    kind: "commercial",
    url: "https://docs.helicone.ai",
    lastVerified: "2026-07-13",
    support: {
      "enforced-keys": {
        status: "no",
        note: "Helicone-Property headers are caller-optional; no operator requirement, so unattributed requests pass.",
      },
      "jwt-values": {
        status: "no",
        note: "Gateway auth is Helicone API keys only; no JWT claim validation documented.",
      },
      "static-values": {
        status: "partial",
        note: "Fixed Helicone-Property headers per app work only if callers send them; no operator-side pinning.",
      },
      "error-bodies": {
        status: "no",
        note: "Rate-limit rejections return a plain 429 with policy headers; custom bodies are not documented.",
      },
      "default-limit": {
        status: "partial",
        note: "Helicone-RateLimit-Policy supports cents quotas but is declared per request, not as a fleet default.",
      },
      alerts: {
        status: "yes",
        note: "Cost and error alerts to email and Slack are built in; Pro plan and up.",
      },
      "live-changes": {
        status: "unknown",
        note: "Not verified yet; GB-9 entered the bar on 14 July 2026 and awaits the next full documentation pass.",
      },
      "aws-invoice": {
        status: "no",
        note: "Bedrock docs pass AWS credentials via headers; no STS session tags or RoleSessionName to the CUR, only Helicone's own cost.",
      },
      "vertex-invoice": {
        status: "no",
        note: "Vertex integration is a proxy plus Helicone logging; no billing labels on generateContent reaching GCP billing export.",
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
