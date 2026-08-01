# The Gateway Project

A community-built LLM gateway that platform teams can be proud of — designed
from scratch, owned end to end, and conformant with the
[Gateway Baseline](https://antonbraverman.com/gateways) (GB-1..GB-9) from day
one.

Every gateway on the verified matrix is either a single instance with a config
surface or a k8s-CRD system that outsources fleet management to ArgoCD. The
genuinely novel product here is not another gateway binary — it is the control
plane. Nobody has "ArgoCD for gateway fleets": heterogeneous fleets (VMs, DMZ
boxes, multiple clouds, edge — not just clusters) plus domain-aware
reconciliation, where the reconciler understands routes, spend limits,
attribution, and token-aware canary analysis rather than diffing opaque YAML.

## You don't need to buy anything. Yet.

Everything in this design runs on open source and Git. No procurement, no
platform subscription, no per-seat pricing, no "talk to sales." If a vendor
pitch lands in your inbox promising all of this today, hold it against the
Baseline matrix — the verified one, not the marketing page. Defer the purchase.
Defer the framework commitment. Defer the rewrite. Every deferral keeps a
decision reversible, and reversible decisions are the only ones a small team
can afford to make quickly. See [docs/00-principles.md](docs/00-principles.md).

## The ownership rule

This is a community solution, and the community's contract is full ownership:
**if something breaks because of a change you made six months ago — you are
responsible.** Not blamed; responsible. You show up, you diagnose, you fix or
revert, and the postmortem names the mechanism, not the person. Nothing is
impossible anymore — the tooling closes the skill gap — so the differentiator
is knowing how to build, collaborate, and stand behind changes for their whole
lifetime.

## Reading order

| Doc | What it answers |
|-----|-----------------|
| [00-principles.md](docs/00-principles.md) | The operating constraints: two binaries plus Git, defer-by-default, ownership |
| [01-design-questions.md](docs/01-design-questions.md) | The central questions every decision hangs off |
| [02-architecture.md](docs/02-architecture.md) | Data plane and control plane design |
| [03-hot-swap.md](docs/03-hot-swap.md) | Hot-swappable config: the promise, the three limitations, the mitigations |
| [04-build-plan.md](docs/04-build-plan.md) | Step-by-step build from scratch, with the risk-ordered sequencing |
| [05-features.md](docs/05-features.md) | Baseline-required features, plus the optional catalog (everything LiteLLM does, triaged) |
| [06-prior-art.md](docs/06-prior-art.md) | What we steal and from whom, including agentgateway's own path to 6/8 |

## Status

**Phase 0 closed (1 August 2026); Phase 1 begun.** Spike A proved the
canonical event model on three wire formats and measured the metering error
bound from 17 machine-recorded transcripts
([spikes/event-model/](spikes/event-model/)). Spike B chose the foundation:
the data plane is built on **Pingora** ([spikes/proxy-pingora/](spikes/proxy-pingora/)
demonstrated the streaming tap without buffering), with the agentgateway
GB-4/GB-8 changes proceeding as parallel upstream contributions
([docs/spike-b/agentgateway-embed.md](docs/spike-b/agentgateway-embed.md)).
Phase 1 — the standalone, Baseline-conformant-from-a-file data plane — is
under way in `crates/`.

License: to be chosen before the repo goes public (Apache-2.0 is the working
assumption, matching the ecosystem we intend to upstream into). Deferred,
deliberately, like everything else that can be.
