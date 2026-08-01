# Principles

*The constraints come first. Architecture is what survives them.*

## 1. Two binaries plus Git

Taken straight from the ArgoCD-vs-Spinnaker lesson: ArgoCD won on operational
weight — 3 components vs 10+ microservices, Git as truth vs Front50's
database. Adopt that as a hard budget:

- **One data plane.** A single binary a team can run on a VM, in a container,
  or in a cluster, configured from a static file with no control plane at all.
- **One control plane.** A single binary with Postgres for *runtime state* —
  never for truth. Truth lives in Git, always.

We steal Spinnaker's two good ideas — Kayenta-style automated canary analysis
and manual judgment gates — but implement them as Git-native mechanisms, not a
pipeline engine. The anti-goal, restated at every phase of the build plan:
**don't accrete Spinnaker.**

## 2. Defer, defer, defer

You don't need to buy anything yet. You may never need to.

- **Defer procurement.** No vendor gateway, no SaaS control plane, no paid
  observability tier until the Baseline matrix — verified cells, not vendor
  claims — shows a gap we cannot close ourselves in reasonable time.
- **Defer framework commitments.** Every dependency is a governance
  relationship, not just a `Cargo.toml` line. We take one only after the
  build-vs-reuse question in doc 01 has been answered for that specific layer.
- **Defer irreversible decisions.** The name, the license, the plugin ABI, the
  wire format of the snapshot protocol — each stays provisional until code
  forces the commitment. Reversible decisions get made fast and cheap;
  irreversible ones get made late and once.

Don't give in to the hype. The vendors are selling urgency; the Baseline is a
list of eight-going-on-nine verifiable behaviors, and most of them are weeks
of work, not platforms.

## 3. Full ownership, six-month horizon

If something breaks because of a change you made six months ago, you are
responsible. This is the community's contract and it shapes the technical
design directly:

- **Every change is attributable and revertible.** Git as truth is not an
  aesthetic — it is what makes the six-month rule survivable. `git log` names
  the change, `git revert` undoes it, and the rendered-snapshot history shows
  exactly what every data plane was running at the moment of the incident.
- **Break-glass exists, with a TTL.** Gateways get emergency-edited at 3am in
  ways ArgoCD-managed clusters don't tolerate. Visible, temporary,
  auto-reverting imperative overrides are the honest middle between forbidding
  3am fixes and losing Git as truth. The override is itself a logged, owned
  change.
- **Stated invariants over magic.** Publish staleness bounds, overlap
  semantics, and bounded-overspend windows as part of the spec. Platform teams
  trust stated error bounds and distrust magic. Trust comes from declared
  edges, not from claiming there are none.

## 4. Creativity has a budget

How creative can we allow ourselves to be within the timeframe allotted to us?
Exactly as creative as the novelty budget allows — and the budget is spent
deliberately, not sprinkled:

- **Spend novelty on:** the canonical event stream, the domain-aware
  reconciler, budget shares for fleet-wide caps, config canaries analyzed on
  token-spend anomaly. These are the things nothing in the matrix does.
- **Do not spend novelty on:** the HTTP engine, TLS, connection pooling, load
  balancing algorithms, or a new general-purpose programming language. These
  are solved; reinventing them converts our timeframe into someone else's
  résumé.

"Maybe even invent a brand new language" gets an honest answer in doc 01: yes
— but a *policy* language, small and total, not a general-purpose one.

## 5. Baseline-conformant from day one

The project is the reference implementation of the Gateway Baseline. GB-1
through GB-9 are not a compliance checklist bolted on at the end; they are the
acceptance tests of the first standalone data plane. The public tracker
becomes our conformance scoreboard, with the obvious integrity rule: our own
row gets verified identically to everyone else's.
