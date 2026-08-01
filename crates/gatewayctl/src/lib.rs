//! gatewayctl: the control plane (Phase 2 — fleet distribution + Git truth).
//!
//! A single binary that turns a config repo (Git truth) into per-node
//! `RenderedSnapshot`s and distributes them to N data planes over one
//! long-lived bidirectional gRPC stream each. It is GitOps for gateway fleets
//! made concrete against docs/07-control-plane.md: desired state in Git, a
//! reconciler that converges the fleet. The same machine as the single-node
//! reloader, with the file replaced by Git and the single node replaced by a
//! fleet.
//!
//! Modules:
//! - [`source`]: the config-source abstraction — a loose directory or a Git
//!   repo at a ref/commit — resolving to a source-agnostic byte set
//!   (docs/07, "Truth in Git").
//! - [`render`]: resolved repo -> canonical flat `Config` -> `RenderedSnapshot`
//!   (reproducible render_hash carrying the source commit; the six-month rule
//!   made mechanical).
//! - [`admission`]: config-PR admission — CEL-expressed + built-in rules gating
//!   a candidate config before it can become desired (docs/07 admission).
//! - [`fleet`]: the applied render, per-node versions, per-wave committed state,
//!   and the all-or-nothing wave outcome adjudication.
//! - [`waves`]: the ordered wave plan — label selectors over node labels,
//!   node-to-wave assignment (first-match, implicit final wave), and the
//!   `waves.yaml` parser (docs/07, "Partial application").
//! - [`gatewayset`]: GatewaySets — a label selector plus a config overlay that
//!   stamps config across every matching node at render time (docs/02
//!   "ApplicationSets + generators").
//! - [`rollout`]: the wave rollout orchestration — the single-wave path, the
//!   ordered MULTI-wave walk grouped by failure domain, and per-node self-heal.
//! - [`canary_rollout`]: the Phase-5 analysis-gated superset of the multi-wave
//!   walk — the config-canary analysis window + auto-rollback between waves and
//!   the Git-native judgment gate ([`canary_rollout::RepoGateSignal`]). A second
//!   `impl ControlPlane` block reusing the wave/telemetry machinery; no pipeline
//!   engine, no separate analysis service (docs/07 anti-goal).
//! - [`canary`]: the PURE config-canary substrate (Phase 5) — the canary policy
//!   (metrics, thresholds, window, manual gates), a per-wave telemetry snapshot,
//!   and the plain-Rust analysis (error rate, p99, token-spend anomaly) that
//!   compares a canary wave against a baseline. No metrics service, no new
//!   dependency (docs/07 anti-goal).
//! - [`telemetry`]: the observed-telemetry sink the analysis reads — per-node
//!   request/error/latency windows folded from the SAME `Status`/NACK stream the
//!   fleet already ingests (spend comes from [`budget`]).
//! - [`reconcile`]: drift detection + self-heal — the desired/delivered/
//!   observed truth table, a periodic tick, and break-glass with TTL
//!   (docs/07, "Drift detection and self-heal").
//! - [`store`]: in-memory runtime state (connected nodes, acked versions,
//!   health, break-glass windows) — Postgres replaces it later and is never
//!   truth.
//! - [`token`]: single-use, short-TTL join-token bootstrap.
//! - [`budget`]: GB-5 fleet-wide budget-share allocation — observed-spend
//!   telemetry, continuous per-node share rebalancing, the ~90% synchronous
//!   escalation reply, and fleet-wide GB-6 alerts from the ingest.
//! - [`server`]: the tonic `FleetService` — auth, push, fan-out, ack/nack,
//!   GB-5 usage/escalation handling, and the session/push-ack correlation the
//!   rollout builds on.
//!
//! **Deferred beyond this milestone** (stated, not implied — docs/07's open
//! questions): projects/tenancy scoping (Phase 5 item 2 — NOT built; the honest
//! remaining item), Postgres (runtime state stays in-memory, never truth),
//! per-node latching, config-repo webhook (poll is the floor). Config-canary
//! analysis between waves, auto-rollback, and the Git-native judgment gate
//! (Phase 5 item 3) ARE built here — see [`canary`], [`telemetry`], and the
//! analysis-gated walk in [`rollout`]. See crates/gatewayctl/README.md.

pub mod admission;
pub mod budget;
pub mod canary;
pub mod canary_rollout;
pub mod fleet;
pub mod gatewayset;
pub mod reconcile;
pub mod render;
pub mod rollout;
pub mod server;
pub mod source;
pub mod store;
pub mod telemetry;
pub mod token;
pub mod waves;
