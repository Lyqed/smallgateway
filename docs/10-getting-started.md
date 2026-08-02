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
| Cloud credentials: STS roles, WIF pools, egress | ✔ (one-time per team) | |
| Provider endpoints + base rejection bodies | ✔ (a shared base repo) | |
| The team's `LLMGateway` CR / fleet repo | | ✔ |
| Attribution keys and their names | | ✔ |
| Token caps, windows, `alertAt` | | ✔ |
| Rejection wording, routes, app overrides | | ✔ |
| The 3am page for a fleet | | ✔ |
| Billing-tag activation (once) | finance | |

Mechanics: the platform team's repo holds a fleet TEMPLATE (providers,
role ARNs with `{{team}}`-style identity, base bodies); each team's repo
vendors it and owns everything scoped. The platform team reviews nothing
inside team repos; teams touch no cloud credentials.

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
nothing: not a control plane, not a dashboard, not a rotation. Common
policy travels the way common code travels — a base repo departments merge
from, each reviewing the update themselves.

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
