//! gateway-proto: the fleet-distribution gRPC wire contract.
//!
//! The generated prost types (`RenderedSnapshot`, `Hello`, `Ack`, `Nack`,
//! `Status`, `Push`, the `ClientMessage`/`ServerMessage` envelopes) and the
//! tonic `FleetService` client + server live under [`fleet`], generated from
//! `proto/fleet.proto` at build time. See docs/07-control-plane.md, "The
//! snapshot distribution protocol", for the binding shape this encodes.
//!
//! This crate is a library, not a binary: it stays inside the two-binaries
//! budget (docs/07, "The two-binaries budget") — `gatewayctl` and `gatewayd`
//! are the two binaries; `gateway-proto` and `gateway-core` are the libraries
//! they share.

/// The generated module (package `gateway.fleet.v1`).
pub mod fleet {
    tonic::include_proto!("gateway.fleet.v1");
}

// Re-export the common surface at the crate root so downstream code writes
// `gateway_proto::RenderedSnapshot` instead of the fully-qualified path.
pub use fleet::{
    client_message, server_message, Ack, AckOfStatus, ClientMessage, Hello, Nack, RenderedSnapshot,
    ServerMessage, Status,
};
pub use fleet::fleet_service_client::FleetServiceClient;
pub use fleet::fleet_service_server::{FleetService, FleetServiceServer};

impl ClientMessage {
    /// Wrap a `Hello` in the client envelope.
    pub fn hello(hello: Hello) -> ClientMessage {
        ClientMessage {
            kind: Some(client_message::Kind::Hello(hello)),
        }
    }

    /// Wrap an `Ack` in the client envelope.
    pub fn ack(ack: Ack) -> ClientMessage {
        ClientMessage {
            kind: Some(client_message::Kind::Ack(ack)),
        }
    }

    /// Wrap a `Nack` in the client envelope.
    pub fn nack(nack: Nack) -> ClientMessage {
        ClientMessage {
            kind: Some(client_message::Kind::Nack(nack)),
        }
    }

    /// Wrap a `Status` in the client envelope.
    pub fn status(status: Status) -> ClientMessage {
        ClientMessage {
            kind: Some(client_message::Kind::Status(status)),
        }
    }
}

impl ServerMessage {
    /// Wrap a `RenderedSnapshot` push in the server envelope.
    pub fn push(snapshot: RenderedSnapshot) -> ServerMessage {
        ServerMessage {
            kind: Some(server_message::Kind::Push(snapshot)),
        }
    }

    /// A bare liveness ack.
    pub fn ack_of_status() -> ServerMessage {
        ServerMessage {
            kind: Some(server_message::Kind::AckOfStatus(AckOfStatus {})),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    /// A RenderedSnapshot survives an encode/decode round-trip byte-for-byte —
    /// the wire type carries exactly what docs/07 specifies.
    #[test]
    fn rendered_snapshot_round_trips() {
        let snap = RenderedSnapshot {
            node_id: "edge-fra-2".to_string(),
            source_commit: "abc123".to_string(),
            render_hash: "deadbeef".to_string(),
            fleet_version: 42,
            config: b"providers: {}\nroutes: []\n".to_vec(),
            compiled_at: 1_700_000_000,
        };
        let bytes = snap.encode_to_vec();
        let back = RenderedSnapshot::decode(bytes.as_slice()).unwrap();
        assert_eq!(back, snap);
    }

    /// The client envelope carries each variant intact through the wire.
    #[test]
    fn client_envelope_round_trips_every_variant() {
        let msgs = vec![
            ClientMessage::hello(Hello {
                node_id: "n1".to_string(),
                join_token: "tok".to_string(),
                labels: std::collections::HashMap::from([("region".to_string(), "fra".to_string())]),
                current_fleet_version: 7,
            }),
            ClientMessage::ack(Ack {
                fleet_version: 8,
                render_hash: "hash".to_string(),
            }),
            ClientMessage::nack(Nack {
                fleet_version: 9,
                render_hash: "hash".to_string(),
                reason: "unknown provider foo".to_string(),
            }),
            ClientMessage::status(Status {
                observed_render_hash: "hash".to_string(),
                health: "ok".to_string(),
                in_flight_streams: 3,
            }),
        ];
        for msg in msgs {
            let bytes = msg.encode_to_vec();
            let back = ClientMessage::decode(bytes.as_slice()).unwrap();
            assert_eq!(back, msg);
        }
    }

    /// The server envelope carries a push and a liveness ack intact.
    #[test]
    fn server_envelope_round_trips() {
        let push = ServerMessage::push(RenderedSnapshot {
            node_id: "n1".to_string(),
            source_commit: "c".to_string(),
            render_hash: "h".to_string(),
            fleet_version: 1,
            config: b"x".to_vec(),
            compiled_at: 0,
        });
        let bytes = push.encode_to_vec();
        assert_eq!(ServerMessage::decode(bytes.as_slice()).unwrap(), push);

        let live = ServerMessage::ack_of_status();
        let bytes = live.encode_to_vec();
        assert_eq!(ServerMessage::decode(bytes.as_slice()).unwrap(), live);
    }
}
