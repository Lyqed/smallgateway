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
//! - [`reconcile`]: drift detection + self-heal — the desired/delivered/
//!   observed truth table, a periodic tick, and break-glass with TTL
//!   (docs/07, "Drift detection and self-heal").
//! - [`store`]: in-memory runtime state (connected nodes, acked versions,
//!   health, break-glass windows) — Postgres replaces it later and is never
//!   truth.
//! - [`token`]: single-use, short-TTL join-token bootstrap.
//! - [`server`]: the tonic `FleetService` — auth, push, fan-out, ack/nack, and
//!   the session/push-ack correlation the rollout builds on.
//!
//! **Deferred beyond this milestone** (stated, not implied — docs/07's open
//! questions): config canary ANALYSIS between waves (Phase 5 — multi-wave is the
//! substrate it sits on, built here; the analysis is not), Postgres, per-node
//! latching, config-repo webhook (poll is the floor). See
//! crates/gatewayctl/README.md.

pub mod admission;
pub mod budget;
pub mod fleet;
pub mod gatewayset;
pub mod reconcile;
pub mod render;
pub mod rollout;
pub mod server;
pub mod source;
pub mod store;
pub mod token;
pub mod waves;
