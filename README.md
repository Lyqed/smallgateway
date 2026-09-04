# Open Source Gateway

[Website](https://opensourcegateway.com) · [Gateway Baseline](https://thegatewaybaseline.com) · [Documentation](docs/10-getting-started.md) · [Contributing](CONTRIBUTING.md)

Open Source Gateway is an experimental LLM gateway and fleet control plane for platform teams that need attribution, spending controls, provider-native traffic, and Git-managed configuration.

The repository contains two runnable components:

- `gatewayd`, the data plane. It proxies provider-native requests, applies policy, meters streaming responses, and exports telemetry.
- `gatewayctl`, the control plane. It renders versioned configuration from Git and reconciles that configuration across a gateway fleet.

The data plane can run by itself from a local file. The control plane is optional.

> **Project status:** active development, pre-1.0. The implementation and test suite are public, but there is no published stable release or production support commitment yet. Interfaces may change.

## What is implemented

| Area | Current implementation |
| --- | --- |
| Provider traffic | OpenAI, Anthropic, Bedrock, and Vertex adapters |
| Attribution | Required labels, operator-pinned values, and optional verified JWT claims |
| Cost controls | Token budgets, alerts, and deliberate mid-stream termination |
| Configuration | Static files, atomic reloads, versioned snapshots, and ACK/NACK handling |
| Fleet operations | Git-backed reconciliation, drift reporting, waves, and canary evaluation |
| Extensions | Signed WASM modules with execution bounds |
| Deployment | Containers, Helm, an `LLMGateway` CRD, and Argo CD and Flux examples |

These are implementation claims, not a production-readiness claim. The tests exercise the contracts in this repository. They do not establish suitability for a particular environment.

## Quick start

The shortest complete path is the [getting-started guide](docs/10-getting-started.md). For local development:

```sh
git clone https://github.com/Lyqed/smallgateway.git
cd opensourcegateway
cargo test --workspace
```

Run the standalone data plane with a configuration file:

```sh
cargo run -p gatewayd -- --config crates/gatewayd/demo/gateway.yaml
```

The Kubernetes path, including local image builds and the Helm chart, is in [deploy/README.md](deploy/README.md).

## Architecture

```text
applications
    |
    v
gatewayd  ---> provider APIs
    ^
    | versioned snapshots
    |
gatewayctl <--- Git
```

`gatewayd` stays in the request path. `gatewayctl` stays outside it. A control plane outage therefore does not require a data-plane outage, although stale configuration remains an operational risk and is reported explicitly.

The design is documented in [principles](docs/00-principles.md), [architecture](docs/02-architecture.md), [the build plan](docs/04-build-plan.md), [feature status](docs/05-features.md), [the control plane](docs/07-control-plane.md), [live cloud integration](docs/09-live-cloud.md), [HTTP fidelity](docs/11-http-fidelity.md), and the [rejection contract](docs/13-rejection-contract.md).

## Repository layout

| Path | Purpose |
| --- | --- |
| `crates/gatewayd` | Data-plane binary |
| `crates/gatewayctl` | Fleet control-plane binary |
| `crates/gateway-core` | Configuration, policy, adapters, metering, and attribution |
| `crates/gateway-proto` | Fleet distribution protocol |
| `crates/gateway-wasm` | Sandboxed extension host |
| `deploy` | Operator, chart, images, and GitOps examples |
| `spikes` | Frozen experiments that informed the current design |
| `upstream` | Contributions prepared for related open-source projects |

## Verification

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
(cd deploy/operator && go test ./...)
```

CI runs the same Rust and Go checks on every pull request and push to `main`.

## Contributing and security

Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request. For usage questions, see [SUPPORT.md](SUPPORT.md). Please report security issues through GitHub private vulnerability reporting as described in [SECURITY.md](SECURITY.md).

## License

Apache-2.0. See [LICENSE](LICENSE).
