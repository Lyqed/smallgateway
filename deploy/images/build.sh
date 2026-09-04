#!/usr/bin/env sh
# Build the three smoke images from the current tree.
#
# The Rust binaries are glibc-linked (wasmtime + pingora), so they are built
# on the host and COPYed into debian:bookworm-slim contexts rather than
# compiled in-image; the operator is a Go multi-stage build with its own
# context (deploy/operator). Copied binaries and fixtures are gitignored —
# this script is the only thing that places them.
#
# Usage, from anywhere:   deploy/images/build.sh
# Then, for a k3d dev cluster:
#   k3d image import smallgateway/gatewayd:smoke \
#                    smallgateway/gatewayctl:smoke \
#                    smallgateway/gateway-operator:smoke -c dev
set -eu

cd "$(dirname "$0")/../.."

cargo build --release --bin gatewayctl --bin gatewayd --bin mock_upstream

# gatewayd: the data plane + the mock upstream + the streaming fixtures the
# mock replays (baked in so the mock needs no volume).
cp target/release/gatewayd target/release/mock_upstream deploy/images/gatewayd/
rm -rf deploy/images/gatewayd/fixtures
cp -r spikes/event-model/fixtures deploy/images/gatewayd/fixtures
docker build -t smallgateway/gatewayd:smoke deploy/images/gatewayd

# gatewayctl: the control plane.
cp target/release/gatewayctl deploy/images/gatewayctl/
docker build -t smallgateway/gatewayctl:smoke deploy/images/gatewayctl

# gateway-operator: Go controller, multi-stage, distroless. The build
# context is deploy/operator; only the Dockerfile lives here.
docker build -f deploy/images/operator/Dockerfile \
  -t smallgateway/gateway-operator:smoke deploy/operator

echo "built: smallgateway/{gatewayd,gatewayctl,gateway-operator}:smoke"
