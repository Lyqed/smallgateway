# gateway-proto

The fleet-distribution gRPC wire contract shared by `gatewayctl` (server) and
`gatewayd` (client). Library only — it stays inside the two-binaries budget
(docs/07): the two binaries are `gatewayctl` and `gatewayd`; this crate and
`gateway-core` are the libraries.

Defined in [`proto/fleet.proto`](proto/fleet.proto) and generated at build time
with tonic + prost. protoc is supplied by the `protoc-bin-vendored` crate, so
the build is hermetic — no system protoc dependency, and the wire format
docs/07 freezes never varies by host.

## The contract (docs/07-control-plane.md, "The snapshot distribution protocol")

One long-lived bidirectional stream, `FleetService.Session`, xDS-shaped and
dial-out (the data plane dials the control plane and holds the stream open, so a
DMZ box or edge node needs no inbound path).

- **`RenderedSnapshot`** — the delivered unit: `node_id`, `source_commit`,
  `render_hash` (SHA-256 of the *rendered* bytes), `fleet_version` (monotonic
  per node), `config` (canonically-serialized flat `Config` the node re-parses),
  `compiled_at`.
- **Data plane → control plane** (`ClientMessage`): `Hello` (join token + node
  identity + last-known version), `Ack` (version + echoed render_hash), `Nack`
  (version + render_hash + reason), `Status` (observed render_hash + health +
  in-flight streams).
- **Control plane → data plane** (`ServerMessage`): `Push { RenderedSnapshot }`,
  `AckOfStatus {}` (liveness only).

There is no `Patch` message and there will not be one: the only way to change a
running node is a new rendered snapshot, and the only source of a rendered
snapshot is a commit (docs/07 enforced at the protocol level).

## Compatibility surface

docs/07 lists the rendered-snapshot wire format and the render-hash
canonicalization on the "irreversible once public" list. Field numbers in
`fleet.proto` are therefore append-only, and the canonical serialization that
feeds `render_hash` is frozen once nodes compare hashes. Proto round-trip tests
live in `src/lib.rs`.
