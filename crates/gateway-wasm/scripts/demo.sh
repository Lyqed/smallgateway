#!/usr/bin/env bash
# Phase 4 committed-log proof: runs the wasm_demo example against the REAL
# wasmtime host and captures every deliverable into ../demo.log.
#
# Proofs (see examples/wasm_demo.rs):
#   (1) a signed module loaded and ENFORCING (header transform + custom reject)
#   (2) an UNSIGNED / TAMPERED module REJECTED at the signature gate
#   (3) a fuel-burning / looping module TERMINATED, failing CLOSED to GB-4
#   (4) the MEASURED per-event hot-path number (the named risk) + honest call
#   (5) an ATOMIC module+config swap; the in-flight stream keeps its OLD
#       module version while a new request binds the NEW one (drain)
#   (6) a STATEFUL-module MIGRATION across a swap (inherit / migrate / reset)
#   (7) break-glass with TTL: pin the empty set, then auto-revert
#
# Deterministic, self-contained (.wat fixtures, no network, no toolchain).
set -euo pipefail
cd "$(dirname "$0")/.."

OUT=demo.log
echo "building + running the Phase 4 WASM demo (release, for the measured number)..."
# --release so the hot-path measurement reflects an optimized build.
cargo run -p gateway-wasm --example wasm_demo --release 2>/dev/null | tee "$OUT"

echo
echo "wrote $(pwd)/$OUT"
