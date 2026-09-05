# Deploying smallgateway on Kubernetes

Existing installation? Chart 0.2.0 changes the Kubernetes API group. Read the
[migration notes](../docs/14-naming-migration.md) before upgrading.

Kubernetes is the primary deployment target. This directory makes the k8s path
first-class: a `LLMGateway` CRD, an operator that reconciles it into a running
control plane and data planes, and a production Helm chart. The heterogeneous
fleet story (VMs, edge, multi-cloud over the existing gRPC transport) is
unchanged and remains the additional reach; the operator drives that same gRPC
fleet transport from inside the cluster.

```
deploy/
├── crds/llmgateway.yaml            # the LLMGateway CustomResourceDefinition
├── operator/                       # the Go controller (kubebuilder-style)
├── charts/
│   ├── gateway-operator/           # PRODUCTION chart: CRD + operator + RBAC + sample
│   └── (data-plane chart)          # the standalone file-mode chart (see below)
├── gitops/                         # worked Argo CD + Flux examples (see gitops/README.md)
├── images/                         # Dockerfiles: gatewayctl, operator
├── samples/mock.yaml               # in-cluster mock upstream for demos/verification
└── README.md
```

## Quick start (Helm)

```sh
# 1. Build the release binaries and the three images from the tree.
#    (gatewayd:smoke ships gatewayd + mock_upstream; gatewayctl:smoke ships
#     gatewayctl; gateway-operator:smoke is the controller.)
deploy/images/build.sh

# 2. Import the images into the k3d dev cluster.
k3d image import smallgateway/gatewayd:smoke \
                 smallgateway/gatewayctl:smoke \
                 smallgateway/gateway-operator:smoke -c dev

# 3. Install the operator (CRD + controller + RBAC). Zero overrides.
helm install gwop deploy/charts/gateway-operator -n gateway-system --create-namespace --wait

# 4. (demo) An in-cluster mock upstream the sample points at.
kubectl apply -n gateway-system -f deploy/samples/mock.yaml

# 5. Apply a sample gateway. The operator reconciles it into a gatewayctl
#    control plane + a gatewayd data plane joined over gRPC.
helm upgrade gwop deploy/charts/gateway-operator -n gateway-system \
  --reuse-values --set sampleGateway.enabled=true

# 6. Watch it go Ready.
kubectl get llmgateway -n gateway-system -w
```

A request through the data plane, enforcing GB-1 attribution:

```sh
# Missing the required `team` key -> 428, operator-owned GB-4 body.
kubectl run c --rm -i --restart=Never --image=curlimages/curl -n gateway-system -- \
  -s -o /dev/null -w '%{http_code}\n' http://demo-gatewayd:8080/openai/v1/chat -d '{}'
# -> 428

# With attribution -> the request is proxied and metered.
kubectl run c --rm -i --restart=Never --image=curlimages/curl -n gateway-system -- \
  -s -o /dev/null -w '%{http_code}\n' -H 'x-team: acme' \
  http://demo-gatewayd:8080/openai/v1/chat -d '{}'
# -> 200
```

## The LLMGateway CRD

Group/version: `gateway.smallgateway.vercel.app/v1alpha1`, kind `LLMGateway`
(shortname `llmgw`). This is a **one-way-door public API**, kept conservative:
`v1alpha1`, a status subresource, and a spec that maps 1:1 onto the existing
gateway config model — it does not invent a parallel policy model.

### Spec -> gateway-core config mapping

Every spec field renders into a fragment of the gatewayctl **config-repo
layout** (the same layout `crates/gatewayctl/src/render.rs` reads). The operator
never renders the flat config or reimplements scope composition; it writes repo
fragments and lets the tested control-plane pipeline compose + validate + hash +
distribute.

| CRD spec field                    | Baseline | Repo fragment / config key                    |
|-----------------------------------|----------|-----------------------------------------------|
| `spec.providers`                  | —        | `providers.yaml`                              |
| `spec.fleet.attribution.requiredKeys` | GB-1 | `fleet/base.chain.yaml` → `attribution.required_keys` |
| `spec.fleet.attribution.headers`  | caller header names (no default) | `fleet/base.chain.yaml` → `attribution.headers` |
| `spec.fleet.attribution.pinned`   | GB-3     | `fleet/base.chain.yaml` → `attribution.pinned` |
| `spec.projects.<p>.attribution`   | —        | `projects/<p>/base.chain.yaml`                |
| `spec.routes[]`                   | —        | `routes/<name>.route.yaml` (one per route)    |
| `spec.rejections.defaultResponse` / `unknownRoute` | GB-4 | `rejections.yaml` (operator-owned bodies) |
| `spec.spendCaps.caps[]`           | GB-5     | `fleet/base.chain.yaml` → `attribution.spend_caps` (token ceilings + window + alertAt) |
| `spec.controlPlane`               | —        | gatewayctl Deployment topology                |
| `spec.dataPlanes`                 | —        | gatewayd Deployment topology (replicas, labels, port) |

GB-4 placeholders `{{key}}` and `{{route}}` in rejection bodies are honored by
the **data plane** at rejection time; the operator passes them through verbatim.
If a rejection reason is omitted, the operator supplies a conservative JSON
default so the render always validates.

**GB-2 JWT auth is not in this API.** GB-2 is project-deferred, so v1alpha1
exposes **no** `spec.auth` field and the operator renders no `auth.yaml`. The
CRD is structural with pruning on, so a `spec.auth` a user sets is **dropped**
(pruned) rather than stored or reconciled — it never produces a Ready CR with
JWT silently ignored. This is deliberate: the API no longer advertises a JWT
mapping it would not deliver. See the Follow-ups section.

### Status subresource

Reflects the real reconciled cluster state:

| Field                      | Meaning |
|----------------------------|---------|
| `observedGeneration`       | Generation the status was computed for (staleness guard). |
| `conditions[]`             | `Ready` / `Degraded` with reason + message + `lastTransitionTime`. |
| `renderedConfigHash`       | SHA-256 over the rendered repo fragments — the operator's **config-input identity** (changes iff the desired config changes). See the honesty note below. |
| `dataPlanes`               | Ready/desired data-plane pods, e.g. `2/2`. |
| `controlPlaneReady`        | gatewayctl Deployment Ready. |
| `nodes[]`                  | Per-node ack state from the gatewayctl status surface: acked version/hash; healthy = connected AND acked the applied render. |

**How `Ready` and the two hashes relate.** `renderedConfigHash` remains the
operator's hash over the repo fragments it wrote — the config *input*
identity. gatewayctl computes its own `render_hash` over the composed flat
bytes it distributes, and now EXPOSES it, with every node's ack state, on a
read-only status surface (`GET /status` on port 6186, through the
control-plane Service). The operator reads it each reconcile: `.status.nodes`
carries each node's acked version/hash (healthy = connected AND acked the
currently-applied render), and **`Ready` requires the fleet COMMIT** — child
Deployments ready AND every desired data plane connected and acking the
applied `render_hash`. During a config rollout the CR therefore reports
`Ready: False / AwaitingFleetCommit` (with a who-is-missing message) until
the fleet actually swapped, closing the old window where status showed a new
input hash while nodes still served the previous config.

## Gateway API alignment (what is standard vs native-CRD)

The Kubernetes Gateway API (`gateway.networking.k8s.io`) is the standards
surface for ingress-style routing. This milestone is **honest** about what it
implements:

- **Native-CRD (implemented now):** `LLMGateway` is the full config surface.
  Providers, the four attribution scopes, GB-1/GB-3/GB-4/GB-5, and the
  control/data-plane topology are all expressed and reconciled through it. This
  is the complete, working path.
- **Standard Gateway API (adapter stub, NOT conformant):** `spec.gatewayClassName`
  is reserved for the standards path — an operator that claims a `GatewayClass`
  and translates standard `Gateway` / `HTTPRoute` objects into `LLMGateway`
  routes. In M1 this field is a **documented adapter seam, not a working
  translation**, and the operator does **not** claim Gateway API conformance.

**Why an adapter, not a fork of Gateway API's types:** the gateway's value is
cost-attribution policy (required keys, pins, spend caps) that has *no
representation* in the standard `HTTPRoute`. The standards path can carry the
routing skeleton (hostnames, path matches, backend refs → providers) while the
attribution policy rides alongside as `LLMGateway` fields or route annotations.
Mapping sketch for the follow-up:

| Standard Gateway API              | LLMGateway equivalent |
|-----------------------------------|-----------------------|
| `Gateway` (claimed `gatewayClassName`) | one `LLMGateway` instance |
| `HTTPRoute.spec.rules[].matches[].path.value` | `route.prefix` |
| `HTTPRoute.spec.rules[].backendRefs[]` | `route.provider` (backend → provider) |
| `HTTPRoute` hostname / header matches | `route.match` (CEL) |
| (no standard field) attribution policy | `LLMGateway` fleet/route attribution |

Full upstream Gateway API conformance is an explicit follow-up (below).

## The operator

A `controller-runtime` (kubebuilder-style) controller. It watches `LLMGateway`
and reconciles it into:

1. a **join-token Secret** (generated once, stable across reconciles);
2. a **config-repo ConfigMap** (the rendered repo fragments; keys are the repo
   paths with `/` flattened to `__`, reconstructed into a `--repo` directory by
   an init container);
3. a **gatewayctl Deployment + Service** (the control plane), fed `--repo` and
   the join token; and
4. a **gatewayd StatefulSet + headless Service** (the data planes), each joined
   to the control-plane Service over gRPC with `--control-plane http://…:6187
   --node-id <stable-pod-name> --join-token <per-ordinal token>`.

### Why the data planes are a StatefulSet (not a Deployment)

The fleet join model makes this a hard requirement:

- **Stable node identity.** A join token binds to the FIRST node-id that burns
  it, and only that same node-id may reconnect on it. A Deployment's random pod
  names change on reschedule, so a restarted data plane would present a burned
  token under a new identity and be refused as a replay. A StatefulSet gives
  each pod a stable name (`<name>-gatewayd-<ordinal>`) that survives restart, so
  reconnect works.
- **Distinct single-use LABEL token per node.** The operator passes gatewayctl
  one `--label-token <labels>:<secret>` per replica (secrets derive from the
  one Secret env var: ordinal `0` → the base, ordinal `N` → `<base>-<N+1>`,
  the same scheme the pod-side selection script uses, so the secret never
  appears in the pod spec). Every node burns a different token, a restart
  re-presents the same one, and — because the tokens carry the CR's
  `dataPlanes.labels` — every joined node lands in the fleet WITH its
  failure-domain labels, which wave plans and GatewaySets select on. Any
  replica count works; the old three-token cap is gone. With label tokens
  present, gatewayctl mints no unlabeled base tokens and no dev default.

### Reconcile discipline

- **Level-triggered & idempotent:** every pass reconciles desired-from-observed
  against live cluster state via `CreateOrUpdate`. A missed event, a manual
  child edit, or an operator restart all heal on the next pass. Child names are
  pure functions of the CR name.
- **Owner references:** every child is owned by the CR, so **deleting the CR
  garbage-collects all children** (Deployments, Services, ConfigMap, Secret).
- **Requeue backoff:** errors return with error (controller-runtime applies
  exponential backoff); a healthy-but-converging pass requeues after 10s; a
  Ready gateway re-checks every 60s so status tracks replica/drift changes. No
  hot loop.

### RBAC (least privilege)

`ServiceAccount` + `ClusterRole` + `ClusterRoleBinding` scoped to exactly the
resources the operator manages:

- `llmgateways` (+ `/status`, `/finalizers` subresources) — get/list/watch/update/patch
- `deployments`, `services`, `configmaps`, `secrets` — full CRUD on owned children
- `pods` — read-only, for readiness
- `events` — create/patch, to surface reconcile outcomes
- `leases` — only when `--leader-elect` is enabled

No cluster-admin, no Nodes, no arbitrary cluster-wide Secret read.

## Two-binary budget decision

The product budget is **two binaries: `gatewayd` (data plane) + `gatewayctl`
(control plane)**, both Rust. The operator is **deploy/ops tooling, not part of
that budget**, and is deliberately:

- a **separate binary**, and
- in a **separate toolchain (Go)**, physically outside the Rust workspace
  (`deploy/operator` is not a workspace member),

so it can never bloat the product's dependency tree or blur the product/ops
boundary. Go + controller-runtime is also the idiomatic, standards-following
operator stack, which is what the "clean, standards-following k8s experience"
direction calls for. The operator ships and versions independently of the two
product binaries. It reconciles the CR by driving the **existing** gatewayctl
`--repo`/gRPC pipeline — it adds no new config-semantics authority.

## The standalone data-plane chart (file mode)

The simplest case — a single `gatewayd` in **file mode** with an in-pod mock,
no control plane, no operator — is the pre-existing working chart proven on
k3d. It stays supported for the smoke/dev path and for anyone who wants a
data plane without the control-plane topology. Install it directly; it needs
no CRD and no operator.

## Crate changes (noted)

Two crate changes were required for k8s-native operation. Both keep the full
`cargo test` suite and `cargo clippy --workspace --all-targets -D warnings`
green, and neither changes any existing behavior.

1. **`crates/gatewayd/src/bin/mock_upstream.rs` — a `--bind` flag.** The mock
   hardcoded a `127.0.0.1` bind, reachable only in-pod as a sidecar. A `--bind`
   flag was added, **defaulting to `127.0.0.1`** so every existing caller (demo,
   tests, the sidecar smoke chart) is byte-for-byte unchanged; the operator
   sample's mock Deployment passes `--bind 0.0.0.0` to be reachable cross-pod as
   a Service. This is the smoke-test finding, fixed. (Not a product binary.)

2. **`crates/gatewayd/src/client.rs` — control-plane bootstrap-ready signal.**
   A latent bug in the data plane's control-plane mode: `connect_once` bound the
   first pushed snapshot and then fell straight into the steady-state stream
   loop WITHOUT signaling the main thread, so `connect_and_bootstrap` blocked
   forever and pingora never started listening — a control-plane data plane
   joined and ACKed but served no traffic. The fix threads an
   `on_ready` one-shot into `connect_once` that fires the moment the first push
   binds (before the steady-state loop), so the main thread starts pingora
   immediately while the stream stays live for later pushes. Reconnects pass
   `None` and are unchanged. This is a genuine `gatewayd` (product-binary) fix;
   it was necessary because k8s is the first environment that actually asserts
   the data plane listens (via the readiness probe). Verified: the in-cluster
   data plane now logs `gatewayd listening on 0.0.0.0:8080` and serves requests;
   `cargo test -p gatewayd` (25+21 tests) and clippy stay green.

## GitOps (Argo CD / Flux)

The `LLMGateway` CR is plain declarative YAML and reconciles level-triggered, so
it drops into a GitOps repo directly. Worked Argo CD and Flux examples live in
[`gitops/`](gitops/README.md): a self-contained config dir (the gateway CR + a
demo upstream), Argo CD `Application`s (gateway alone, or an App-of-Apps that
truly orders operator-before-gateway), and a one-file Flux bootstrap
(`GitRepository` + Kustomizations with `dependsOn` ordering). The one hard
constraint — the CRD must exist before any `LLMGateway` — is enforced by Flux
`dependsOn` and by the Argo App-of-Apps (a bare cross-Application `sync-wave`
does not gate, and the docs say so). This extends the
control plane's own Git-as-truth model one layer up: the merge is the deploy,
`git revert` is the rollback, and the PR review is the change-control gate.

## Follow-ups (explicitly NOT in this milestone)

- **Full upstream Gateway API conformance:** the `GatewayClass` claim +
  `Gateway`/`HTTPRoute` translation described above is an adapter seam in M1;
  full conformance (and the conformance test suite) is a follow-up.
- **HA control plane:** `controlPlane.replicas` is pinned to 1 in v1alpha1
  (the runtime store — acks, budgets — is in-memory single-authority).
- **GB-2 auth in the CRD (the k8s surface only):** the GATEWAY half is now
  built — HS256 for fleet-minted tokens, RS256 against an inline JWKS (key
  rotation rides the same GB-9 hot-swap as every rule), and verified claims
  reach required keys, pins, session tags, RoleSessionName templates, and
  guardrail values (see crates/gatewayd/README.md "A note on GB-2"). What
  remains is only the Kubernetes field: `spec.auth` with a Secret reference
  the operator resolves into the rendered `auth:` block (the secret lands in
  the config the control plane distributes, never in the CR). v1alpha1 still
  exposes no `spec.auth` — a set one is pruned, not silently ignored — and
  the field appears only together with working Secret resolution, verified
  end to end. Turning GB-2 on remains a per-fleet judgment call either way.
