# Getting started

From zero to a running, enforcing gateway in about ten minutes, then the
part that actually decides whether this works in your organization: who
owns what.

This guide assumes a grounding in the basics: docs/00-principles.md (what
the gateway refuses to become), docs/02-architecture.md (two binaries and
Git), and docs/05-features.md (the Gateway Baseline, GB-1 through GB-9).

## Requirements

- `kubectl` and `helm`, with a kubeconfig pointing at a cluster (k3d/kind
  are fine for a first run).
- The gateway images in a registry the cluster can pull from. For a local
  k3d cluster, build and import them from the tree:

```sh
deploy/images/build.sh
k3d image import thegatewayproject/gatewayd:smoke \
                 thegatewayproject/gatewayctl:smoke \
                 thegatewayproject/gateway-operator:smoke -c <cluster>
```

No cluster? The single-binary file mode needs only a Linux box:
`gatewayd --config gateway.yaml` — see deploy/README.md "The standalone
data-plane chart". Everything below is the Kubernetes path.

## 1. Install the operator

```sh
helm install gwop deploy/charts/gateway-operator \
  -n gateway-system --create-namespace --wait
```

This installs the `LLMGateway` CRD, the controller, and least-privilege
RBAC. Nothing else: no database, no UI, no SaaS dependency, no telemetry.

## 2. Create your first gateway

An `LLMGateway` is the whole desired gateway as one reviewable object:
providers, the attribution contract, routes, caps, rejection bodies,
topology.

```yaml
# gateway.yaml
apiVersion: gateway.thegatewayproject.io/v1alpha1
kind: LLMGateway
metadata:
  name: demo
  namespace: gateway-system
spec:
  providers:
    openai-demo:
      kind: openai
      upstream: { host: gateway-mock, port: 6190 }
  fleet:
    attribution:
      requiredKeys: [team]          # GB-1: your key names, not ours
      models: [gpt-4o, claude-3*]   # which models these clients may use
      pinned: { env: prod }         # GB-3: assigned, never believed
  routes:
    - name: openai
      prefix: /openai
      provider: openai-demo
  spendCaps:
    caps:
      - key: team
        value: acme
        limitTokens: 500000
        window: day
        alertAt: 80
  rejections:
    missingAttribution:
      status: 428
      contentType: application/json
      body: '{"error":"attribution_required","missing":"{{key}}","route":"{{route}}"}'
    unknownRoute:
      status: 404
      contentType: application/json
      body: '{"error":"unknown_route","path":"{{route}}"}'
  dataPlanes:
    replicas: 2
    labels: { region: local }       # rides the join tokens into the fleet
```

```sh
kubectl apply -n gateway-system -f deploy/samples/mock.yaml   # demo upstream
kubectl apply -f gateway.yaml
```

The operator reconciles this into a gatewayctl control plane and two
gatewayd data planes, joined over gRPC with single-use label tokens.

## 3. Watch it go Ready — and know what Ready means

```sh
kubectl get llmgateway -n gateway-system -w
```

`Ready` here is a strong claim: not "the pods look healthy" but **every
data plane joined the fleet and acked the exact rendered config**. While a
change is still propagating you will see `AwaitingFleetCommit` with a
who-is-missing message. `status.nodes` shows each node's acked
version/hash.

## 4. Send your first request — it should be refused

```sh
kubectl run c --rm -i --restart=Never --image=curlimages/curl -n gateway-system -- \
  -s -o /dev/null -w '%{http_code}\n' http://demo-gatewayd:8080/openai/v1/chat -d '{}'
# -> 428
```

That 428, with YOUR body verbatim, is the product working: an
unattributed request never reaches a provider and never spends a token.
Now with attribution:

```sh
kubectl run c --rm -i --restart=Never --image=curlimages/curl -n gateway-system -- \
  -s -o /dev/null -w '%{http_code}\n' -H 'x-attr-team: acme' \
  http://demo-gatewayd:8080/openai/v1/chat -d '{}'
# -> 200, attributed, metered, counted against acme's 500k/day
```

## 5. Look at the fleet directly (optional)

The control plane serves a read-only status surface — the same one the
operator's Ready gate uses:

```sh
kubectl port-forward -n gateway-system svc/demo-gatewayctl 6186:6186 &
curl -s localhost:6186/status | jq .
```

## 6. Put it in Git

The CR is plain YAML; the deployment loop belongs in Git from day one:
edit, merge request, merge. The merge is the deploy; `git revert` is the
rollback; the diff (`team=acme, 500k tokens/day, alert at 80`) is the
change-control gate. Worked Argo CD and Flux setups, with the
CRD-before-CR ordering handled, are in deploy/gitops/.

---

# Real-world gateways, complete

Two production-shaped configs — the full policy surface, nothing elided.
These are NATIVE gateway configs (file mode, or the flat config a control
plane renders from a fleet repo): the advanced provider blocks below (the
STS role chain, the Vertex WIF auth, injection, locations, telemetry,
alerts) are native surfaces; the Kubernetes CRD carries the common core
today and grows a field only when its operator path is verified end to
end. Key names (`cost_center`, `workload`, `study`) are the operator's
own — nothing below is a built-in name.

One seam, stated: the STS/Google token clients are verified against
contract-enforcing mocks over plaintext; live endpoints are TLS — see
docs/09-live-cloud.md before the first real run.

## Bedrock, with the full role chain

What this buys: every request carries invoice-grade identity into the AWS
CUR via session tags, the assumed IAM role is picked per request from an
operator-closed set, guardrails are forced and SIGNED, and a team's token
budget alerts its own webhook at 80%.

```yaml
# gateway.yaml — a hospital research fleet, Bedrock
providers:
  bedrock-prod:
    kind: bedrock
    upstream: { host: bedrock-runtime.us-east-1.amazonaws.com, port: 443, tls: true }
    sts:
      endpoint: { host: sts.us-east-1.amazonaws.com, port: 443, tls: true }
      region: us-east-1
      # The assumed role is TEMPLATED on adjudicated attribution: the
      # caller picks WHICH operator-built role, never a new one, because
      # cost_center is gated by the allow list below.
      role_arn: arn:aws:iam::111122223333:role/bedrock-{{cost_center}}
      session_name: '{{cost_center}}-{{workload}}'   # -> CloudTrail identity
      allow:
        key: cost_center
        values: [research, radiology, pharmacy]
      # The two-hop chain: platform OIDC token -> base role -> SigV4-SIGNED
      # AssumeRole into the templated role. Live-STS semantics.
      base:
        web_identity_token: { file: /var/run/secrets/tokens/gateway-token }
        role_arn: arn:aws:iam::111122223333:role/gatewayd-base
        sts_region: us-east-1
      tags:                            # -> CUR columns, once activated
        - { key: cost_center, from_attribution: cost_center }
        - { key: workload, from_attribution: workload }
    inject:                            # forced guardrails, in the SIGNED set
      headers:
        - { name: x-amzn-bedrock-guardrailidentifier, value: gr-phi-default }
        - { name: x-amzn-bedrock-guardrailversion, value: '3' }
      body:
        - { path: guardrailConfig.guardrailIdentifier, value: gr-phi-default, if_absent: true }
        - { path: guardrailConfig.guardrailVersion, value: '3', if_absent: true }

fleet:
  attribution:
    required_keys: [cost_center, workload]
    # PROVEN from the verified login (GB-2): role material and session
    # tags may only reference gateway-established keys, so the identity
    # that reaches CloudTrail and the CUR is claim-proven, never a bare
    # caller header. The allow list above additionally closes the value
    # set for role selection.
    from_claims: { cost_center: cost_center, workload: workload }
    pinned: { env: prod }
    models: ['anthropic.claude*']      # this fleet runs Claude only
    spend_caps:
      cost_center:
        default: 2000000               # tokens
        window: day
        alert_at: 80
        overrides:
          research: 8000000            # the heavy user, knowingly

routes:
  - prefix: /bedrock
    provider: bedrock-prod

auth:
  jwt:
    # Fleet-minted tokens (HS256). For a real IdP, replace with the inline
    # JWKS form — RS256, parsed at load, rotation via config hot-swap:
    #   jwks: '<the IdP's JWKS document, pasted or CI-synced inline>'
    hs256_secret: replace-with-the-fleet-signing-secret

rejections:
  missing_attribution:
    status: 428
    content_type: application/json
    body: '{"error":"attribution_required","missing":"{{key}}","route":"{{route}}","help":"Set x-attr-cost_center and x-attr-workload. Research IT: ext 4571."}'
  unknown_route:
    status: 404
    content_type: application/json
    body: '{"error":"unknown_route","path":"{{route}}"}'
  model_not_allowed:
    status: 403
    content_type: application/json
    body: '{"error":"model_not_allowed","asked":"{{model}}","route":"{{route}}","help":"This fleet runs Claude models only."}'

alerts:                                # WHO is told: this fleet's receiver
  webhook:
    endpoint: { host: alertmanager.monitoring.svc.cluster.local, port: 9093 }
    path: /api/v2/alerts

telemetry:                             # spans to the collector you own
  otlp:
    endpoint: { host: otel-collector.monitoring.svc.cluster.local, port: 4318 }
    service_name: bedrock-research-fleet
```

## Vertex, with WIF auth and the location gate

What this buys: the gateway MINTS the Google credential itself (callers
hold no GCP keys), spend lands labeled in the BigQuery billing export,
and only EU locations are reachable.

```yaml
# gateway.yaml — an EU-resident clinical fleet, Vertex
providers:
  vertex-eu:
    kind: vertex
    upstream: { host: aiplatform.googleapis.com, port: 443, tls: true }
    locations: [eu, europe-west3, europe-west4]   # anything else: your 404
    auth:                              # WIF: no long-lived keys anywhere
      web_identity_token: { file: /var/run/secrets/tokens/gateway-token }
      wif:
        project_number: "123456789012"
        pool_id: gateway-pool
        provider_id: gateway-provider
      service_account_email: gateway@clinical-eu.iam.gserviceaccount.com
      sts_endpoint: { host: sts.googleapis.com, port: 443, tls: true }
      iam_endpoint: { host: iamcredentials.googleapis.com, port: 443, tls: true }

fleet:
  attribution:
    required_keys: [study]
    pinned: { env: prod, data_residency: eu }
    models: [gemini-2.5-pro, gemini-2.5-flash]
    spend_caps:
      study:
        default: 1000000
        window: day
        alert_at: 80

routes:
  - prefix: /vertex
    provider: vertex-eu
    labels:                            # -> BigQuery billing export
      - { key: study, from_attribution: study }
      - { key: residency, value: eu }

rejections:
  missing_attribution:
    status: 428
    content_type: application/json
    body: '{"error":"attribution_required","missing":"{{key}}","route":"{{route}}"}'
  unknown_route:
    status: 404
    content_type: application/json
    body: '{"error":"unknown_route","path":"{{route}}"}'

alerts:
  webhook:
    endpoint: { host: hooks.internal, port: 443, tls: true }
    path: /clinical-fleet/gb6
```

## Who is told, exactly

`alert_at: 80` fires GB-6 at the ENFORCEMENT point. Two things always
happen, and one more when configured:

1. A structured log line lands at the node that enforced — the guaranteed
   record, grep-able, shipped by whatever log pipeline you already run.
2. The OTLP request spans carry the spend curve that led to it (per-request
   token counts under the same attribution keys), so the collector can
   alert on trajectory too.
3. With `alerts.webhook` set, this JSON POSTs to the receiver THIS
   fleet's repo names — Alertmanager, a Slack webhook, a pager bridge:

```json
{"kind":"soft_threshold","fraction":0.8,"key":"cost_center","value":"research",
 "cap_tokens":8000000,"spend_tokens":6400000,"node":"gatewayd-1"}
```

The addressing is the point: the webhook lives in the fleet's own config,
so the alert reaches the team that owns the rules and the budget — not a
central NOC that has to re-route it.

## A WASM module a client actually asked for: the MRN tripwire

The creative case: a hospital's compliance team wants a hard stop if a
model ever starts GENERATING something shaped like a medical record
number (their format: `MRN` + 8 digits) — regardless of which prompt
caused it. That is a per-token judgment on the RESPONSE stream, which is
exactly what the tier-2 hook exists for; no gateway fork, no vendor
ticket.

```yaml
wasm:
  per_event_hooks: true                # off by default; this fleet opts in
  modules:
    - name: mrn-tripwire
      source: modules/mrn_tripwire.wasm
      signature: 9f2c…                 # HMAC-SHA256; unsigned never loads
      hooks: [on_response_event]
      schema: 1
```

The module itself is ~40 lines against the SDK: it sees each canonical
`ContentDelta`, keeps a small rolling window across chunk boundaries, and
returns `cut` on a match — the stream ends with THIS fleet's GB-4
terminal event, mid-generation, before the number finishes printing.

The properties come from the platform, not the module: it is signed (an
unsigned module never loads), versioned WITH the config snapshot (no
torn module/config reads), rolled out in waves and revertable like any
rule, fuel- and epoch-sandboxed (a buggy regex cannot hang the stream),
and breakable-glass (SIGUSR1 disables modules for a bounded TTL). Honest
cost, measured: ~12.7µs per event with fresh-instance isolation — which
is why per-event hooks are opt-in per fleet rather than a default.

---

# A large organization: two ways to split the responsibility

The setup above is one gateway. The real question at, say, a 40-team
enterprise or a hospital network is organizational: every tool abstracts
something away, and the question is for whom. Both models below are
first-class; they differ in who carries the pager. A fleet is the unit of
ownership either way — one control plane, its data planes, one Git repo,
one owner — and moving between the models later is re-drawing repo
boundaries, not a redesign.

## Model A — platform-operated, team-owned policy

One infrastructure team runs the MACHINERY; every product team owns its
RULES.

| Responsibility | Platform team | Product team (per fleet) |
|---|---|---|
| Operator install/upgrade, images, CRD versions | ✔ | |
| Cloud credentials: STS roles, WIF pools, egress | ✔ (one-time) | |
| Provider endpoints + base rejection bodies | ✔ | |
| The team's `LLMGateway` CR / fleet repo | | ✔ |
| Attribution keys and their names | | ✔ |
| Token caps, windows, `alertAt` | | ✔ |
| Rejection wording, routes, app overrides | | ✔ |
| The 3am page for a fleet | | ✔ |
| Billing-tag activation (once) | finance | |

Mechanics: the platform team's repo holds a fleet TEMPLATE (providers,
role ARNs templated on whatever identity grain the org chooses — org,
team, user — and base bodies); each team's repo vendors it and owns
everything scoped. The platform team reviews nothing inside team repos;
teams touch no cloud credentials.

**End result:** one operating rotation instead of forty; forty policy
owners instead of one bottleneck. A team changing its own cap never files
a platform ticket, and the platform upgrading the operator never touches a
team's rules. Blast radius of a bad policy merge: that team's fleet. The
invoice slices by every team's own keys.

Choose this when a capable platform group already exists and teams want
policy autonomy without operational duty.

## Model B — fully decentralized fleets

Every department runs its own COMPLETE stack: its own `LLMGateway` (or its
own operator install in its own namespace), its own repo, its own
cloud roles, its own pager. There is no central gateway team at all.

| Responsibility | Cloud/IAM admin (one-time) | Each department |
|---|---|---|
| Per-department IAM roles / WIF bindings, scoped | ✔ | |
| Billing-tag/label activation | ✔ (with finance) | |
| Everything else — operator, CR, repo, keys, caps, bodies, upgrades, the page | | ✔ |

Mechanics: a department bootstraps by forking a template repo and running
steps 1-6 above in its own namespace or cluster. Departments share
nothing: not a control plane, not a dashboard, not a rotation, not a
base repo.

**End result:** zero shared operational surface; a department's outage,
bad merge, or weird requirement is entirely its own. Onboarding
department forty-one is a repo fork, not a platform-team project. The
only thing that converges is spend — attributed line by line on the cloud
invoice, which was already centralized before you arrived.

Choose this when autonomy is the point (hospitals, multi-entity groups,
research organizations) or when no platform team exists to volunteer.

## Choosing, honestly

Start with Model A if you have a platform group today: it is the shorter
path and every artifact — fleet repos, CRs, cloud roles — carries over
unchanged if a team later takes full ownership and slides into Model B.
Start with Model B if creating a central gateway team is exactly what you
are trying to avoid. What we recommend against is the third model this
project exists to replace: one team owning everyone's POLICY — the
single pane of glass, where every cap change in the company queues behind
one backlog.

Both models land on the idea this document keeps returning to:
**responsibility is owned, never diluted.** The team that runs a fleet
owns all of it — every request that crosses it, every cap, every
incident, the PII that leaks or does not, the breach post-mortem, the
page. There is no provider governance layer in the middle to absorb
blame, and the absence is the feature: an executive who knows every AI
incident lands on a named team has no incentive to buy tooling whose
main function is to make responsibility harder to locate.
