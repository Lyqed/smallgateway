//! Compile `proto/fleet.proto` into the tonic client/server + prost message
//! types at build time.
//!
//! protoc is provided by the vendored prebuilt binary rather than the system
//! PATH, so `cargo build`/`cargo test`/`clippy` are hermetic — the wire format
//! docs/07 freezes must not depend on which protoc happens to be installed.

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    // prost-build reads $PROTOC; point it at the vendored binary.
    // SAFETY: single-threaded build script, before any codegen runs.
    unsafe {
        std::env::set_var("PROTOC", &protoc);
    }

    let proto = PathBuf::from("proto/fleet.proto");
    println!("cargo:rerun-if-changed={}", proto.display());

    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_protos(&[proto], &[PathBuf::from("proto")])?;
    Ok(())
}
