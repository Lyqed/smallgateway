# GitOps: managing the gateway from Git

The gateway's control plane is already Git-native — rendered-manifest
compilation is a pure function of a commit, and the six-month rule ("if
something breaks because of a change you made six months ago, you are
responsible") is mechanical because `git blame` names every attribution key and
every spend cap. GitOps is that same thesis extended one layer up: the
*desired cluster state* also lives in Git, and a controller (Argo CD or Flux)
continuously reconciles the cluster toward it.

There is no impedance mismatch to bridge. The `LLMGateway` CR is plain
declarative YAML, the operator reconciles it level-triggered, and every child
it creates carries an owner reference. So a GitOps controller on top gets clean
semantics for free:

- **The merge is the deploy.** Edit `config/gateway.yaml`, open a PR, merge.
  The controller syncs; the operator reconciles; the fleet converges.
- **`git revert` is the rollback.** No console clicking, no imperative
  `kubectl`. The previous commit is the previous gateway.
- **The PR review is the change-control gate.** A reviewer sees
  `team=acme, $500/day` in the diff and approves or pushes back *before* it is
  live. This is the Git-native judgment gate (Phase 5) at the deploy layer.
- **Drift heals both ways.** The GitOps controller reverts manual edits to the
  CR back to Git; the operator reverts manual edits to the CR's children back
  to the CR. Two level-triggered loops, same direction: toward Git.
- **Delete is delete.** Remove `gateway.yaml` from Git and the controller
  prunes the CR; owner references then garbage-collect the entire control
  plane and data-plane fleet under it.

## What is where

```
deploy/gitops/
├── config/                 # the DESIRED GATEWAY — this is what you edit
│   ├── gateway.yaml            # the LLMGateway CR (providers, GB-1/3/4/5, topology)
│   ├── mock-upstream.yaml      # demo upstream, so the loop runs on k3d; delete for real use
│   └── kustomization.yaml
├── argocd/                 # Argo CD Applications
│   ├── application.yaml         # syncs config/ (the gateway)
│   └── application-operator.yaml  # OPTIONAL: also GitOps-manage the operator itself
└── flux/                   # Flux
    ├── bootstrap.yaml          # GitRepository + both Kustomizations (apply this one file)
    └── operator/               # the operator HelmRelease, wrapped for dependsOn ordering
```

## The one ordering constraint

The `LLMGateway` CRD must exist before any `LLMGateway` object is applied. That
is the *only* hard ordering rule. The two tools do NOT enforce it the same way,
and it is worth being precise about the difference:

- **Flux — a real ordering gate.** The gateway Kustomization `dependsOn` the
  operator Kustomization, and the operator Kustomization health-checks the
  operator HelmRelease. Flux blocks the `LLMGateway` until the operator (and its
  CRD) is applied *and healthy*. `dependsOn` is a genuine cross-Kustomization
  wait.

- **Argo CD — two honest options, because a bare `sync-wave` does NOT gate.** A
  sync-wave orders resources only *within a single sync operation*. Two
  independently-applied `Application` objects reconcile concurrently, so a wave
  on one does **not** sequence it relative to the other. There is no equivalent
  of Flux `dependsOn` between two loose Applications. So pick one:
  1. **Install the operator once by hand (Helm), GitOps-manage only the
     gateway** (`application.yaml`). Simple, and the CRD is guaranteed present
     before Argo ever applies the CR.
  2. **App-of-Apps** (`app-of-apps.yaml`): a parent Application syncs both
     children in one sync, which is what makes the operator child's
     `sync-wave: "-1"` actually order it before the gateway child. This is the
     only way to get a true CRD-first guarantee with Argo owning the operator.

  Applying `application.yaml` + `application-operator.yaml` directly (no parent)
  still *converges* — the gateway Application tolerates the missing CRD via
  `SkipDryRunOnMissingResource` + retry and heals once the operator lands — but
  that is eventual convergence by retry, **not** a hard ordering guarantee. Use
  option 1 or 2 when you want the guarantee.

The operator is the *platform* layer (like cert-manager or an ingress
controller): install it once as a prerequisite, or GitOps-manage it in its own
sync scope. The *gateway* is the *application* layer. Keeping them in separate
scopes keeps their blast radii separate — a bad gateway edit can never roll the
operator, and vice versa.

## Argo CD — quick start

```sh
# Option 1 — operator by hand, GitOps the gateway (guaranteed CRD-first):
helm install gwop deploy/charts/gateway-operator \
  -n gateway-system --create-namespace --wait
# Point the Application's repoURL at your fork, then:
kubectl apply -f deploy/gitops/argocd/application.yaml

# Option 2 — Argo owns the whole stack, ordering enforced by App-of-Apps:
#   (point the repoURLs at your fork first)
kubectl apply -f deploy/gitops/argocd/app-of-apps.yaml

# Either way, watch Argo sync, then the gateway go Ready.
argocd app get gateway-demo
kubectl get llmgateway -n gateway-system -w
```

## Flux — quick start

```sh
# Point bootstrap.yaml's GitRepository url at your fork, then apply it once.
kubectl apply -f deploy/gitops/flux/bootstrap.yaml

# Flux installs the operator first (dependsOn), then the gateway.
flux get kustomizations
kubectl get llmgateway -n gateway-system -w
```

## Verifying the loop is live

Once Ready, GB-1 enforcement is served by the data plane exactly as in the Helm
quick start (see `../README.md`). The GitOps proof is the *round trip*:

```sh
# Change the fleet token cap in Git — e.g. limitTokens 500000 -> 250000 —
# commit, push. The controller syncs, the operator re-renders, the fleet
# converges. No kubectl touched the cluster. `git log config/gateway.yaml`
# is now the authoritative deploy history for the gateway's spend policy.
```

## Notes

- **Point the repoURL at your own fork/clone.** The examples reference
  `github.com/thegatewayproject/thegatewayproject`; change it to wherever your
  config actually lives. In real use you would keep `config/` in a
  deploy repo separate from the source tree.
- **`config/mock-upstream.yaml` is demo-only.** It exists so the loop is
  demonstrable on k3d with no external provider. Delete it and point
  `spec.providers[].upstream` at your real endpoint; nothing else changes.
- **Secrets stay out of Git.** No secret is referenced by these examples (GB-2
  JWT auth is deferred and exposes no `spec.auth`). When a config genuinely
  needs a secret, use your GitOps controller's sealed-secret / SOPS / external-
  secrets path — never a plaintext secret in the config dir.
