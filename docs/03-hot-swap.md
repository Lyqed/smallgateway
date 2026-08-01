# Hot-swappable configuration, and what it costs

*14 July 2026. First section of the Gateway Project design doc — the GB-9
treatment. The promise, the three limitations, and the mitigations that follow
from them.*

## The promise

APIM-style: change policy on a live gateway (routes, transformations, limits,
streaming rewrites) with no redeploy, no restart, no dropped request. The
design: config rendered ahead of time into versioned snapshots
(rendered-manifest pattern, no runtime templating, every snapshot reviewable),
pushed to data planes over long-lived gRPC with ACK/NACK, modules bound
atomically per snapshot. New requests bind to the new version; in-flight work
finishes on the old.

## Limitation 1: reconciler vs. imperative sync — where truth lives during the swap

Hot-swap tempts toward imperative mutation: PATCH the running gateway, see the
change now. Every imperative change forks runtime truth away from Git, and the
fork compounds. The reconciler model (desired state in Git, a loop converges
the fleet) keeps truth in one place at the price of honesty about time: there
is always a window where Git says X and some node runs X-1. Versioned
snapshots with ACK/NACK bound the window; they do not eliminate it. A node
that NACKs is deliberately divergent, so partial application needs an explicit
policy: all-or-nothing waves, or per-node latching with divergence surfaced —
never silent.

## Limitation 2: long-lived streams pin old config — drain semantics

LLM responses stream for seconds to minutes; a swap cannot rebind an in-flight
stream without corrupting it. The old config version stays resident until its
last stream drains, so two versions are live simultaneously. That forces a
precise answer to: during the overlap, which version's spend limit meters the
stream? A cap tightened mid-stream does not apply to streams already running.
That is not a bug to fix; it is a bounded-staleness semantic to document.
Publish the error bound; do not promise instant enforcement over streaming
traffic.

## Limitation 3: stateful policies do not hot-swap cleanly — state migration

Swapping a stateless transformation is trivial. Swapping anything that owns
counters (budgets, rate limits, quota shares) is a state-migration problem:
the incoming module either inherits the old counters (counter schemas must be
versioned, with migration hooks) or resets them (a bounded over-spend window
while every spender reads zero). Same trade-off triangle as distributed quota
(central counter vs. local state vs. hybrid budget shares), multiplied by
"during the swap."

## Mitigations that follow from the limitations

- **Admission control on config PRs**: CEL validations before render.
- **Canary configuration the way code is canaried**: wave rollouts scoped by
  failure domain, auto-rollback on error rate and token-spend anomaly.
- **Stated invariants over magic**: publish staleness bounds and overlap
  semantics as part of the spec. Trust comes from declared edges, not from
  claiming there are none.
