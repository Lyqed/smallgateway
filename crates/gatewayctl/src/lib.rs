//! gatewayctl: the control plane (Phase 2 — fleet distribution + Git truth).
//!
//! A single binary that turns a config repo (Git truth) into per-node
//! `RenderedSnapshot`s and distributes them to N data planes over one
//! long-lived bidirectional gRPC stream each. It is the ArgoCD-for-gateway-
//! fleets control plane made concrete against docs/07-control-plane.md — the
//! same machine as the single-node reloader, with the file replaced by Git and
//! the single node replaced by a fleet.
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
//! - [`fleet`]: the applied render, per-node versions, and the all-or-nothing
//!   wave rollout (single wave in this milestone).
//! - [`reconcile`]: drift detection + self-heal — the desired/delivered/
//!   observed truth table, a periodic tick, and break-glass with TTL
//!   (docs/07, "Drift detection and self-heal").
//! - [`store`]: in-memory runtime state (connected nodes, acked versions,
//!   health, break-glass windows) — Postgres replaces it later and is never
//!   truth.
//! - [`token`]: single-use, short-TTL join-token bootstrap.
//! - [`server`]: the tonic `FleetService` — auth, push, fan-out, ack/nack.
//!
//! **Deferred beyond this milestone** (stated, not implied — docs/07's open
//! questions): Postgres, multi-wave rollouts grouped by failure domain,
//! per-node latching, GatewaySets / label-generators, config-repo webhook (poll
//! is the floor). See crates/gatewayctl/README.md.

pub mod admission;
pub mod fleet;
pub mod reconcile;
pub mod render;
pub mod server;
pub mod source;
pub mod store;
pub mod token;
