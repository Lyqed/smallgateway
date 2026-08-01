//! gatewayctl: the control plane (Phase 2, milestone 1 — fleet distribution).
//!
//! A single binary that turns a config repo (Git truth, a plain directory in
//! M1) into per-node `RenderedSnapshot`s and distributes them to N data planes
//! over one long-lived bidirectional gRPC stream each. It is the ArgoCD-for-
//! gateway-fleets control plane made concrete against
//! docs/07-control-plane.md — the same machine as the single-node reloader,
//! with the file replaced by Git and the single node replaced by a fleet.
//!
//! Modules:
//! - [`render`]: config repo -> canonical flat `Config` -> `RenderedSnapshot`
//!   (reproducible render_hash; the six-month rule made mechanical).
//! - [`fleet`]: the applied render, per-node versions, and the all-or-nothing
//!   wave rollout (single wave in M1).
//! - [`store`]: in-memory runtime state (connected nodes, acked versions,
//!   health) — Postgres replaces it later and is never truth.
//! - [`token`]: single-use, short-TTL join-token bootstrap.
//! - [`server`]: the tonic `FleetService` — auth, push, fan-out, ack/nack.
//!
//! **Deferred beyond M1** (stated, not implied — docs/07's open questions):
//! Git integration (libgit2/webhook/poll of a real repo), Postgres, drift
//! detection + self-heal, config-PR admission checks, and multi-wave rollouts
//! grouped by failure domain. See crates/gatewayctl/README.md.

pub mod fleet;
pub mod render;
pub mod server;
pub mod store;
pub mod token;
