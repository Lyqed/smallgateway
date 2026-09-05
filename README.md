# smallgateway

A small gateway for LLM traffic, with an optional control plane for managing several instances from Git. An experimental project by Anton Braverman.

[Website](https://smallgateway.vercel.app) · [Getting started](docs/10-getting-started.md) · [Contributing](CONTRIBUTING.md)

## What it does

`gatewayd` sits between an application and its model provider. It forwards requests, tracks usage, attaches attribution labels, and applies spending limits.

`gatewayctl` reads configuration from Git and rolls it out to gateway instances. You can also run `gatewayd` on its own with a local configuration file.

The repository includes provider adapters, streaming tests, a Kubernetes operator, and deployment examples. It is still experimental. There is no stable release or production support commitment yet.

## Try it locally

```sh
git clone https://github.com/Lyqed/smallgateway.git
cd smallgateway
cargo test --workspace
cargo run -p gatewayd -- --config crates/gatewayd/demo/gateway.yaml
```

The demo needs an upstream service. See the [getting-started guide](docs/10-getting-started.md) for a complete setup and [deployment notes](deploy/README.md) for Kubernetes.

## In this repository

| Path | Contents |
| --- | --- |
| `crates/gatewayd` | Request proxy |
| `crates/gatewayctl` | Configuration and fleet management |
| `crates/gateway-core` | Provider adapters, policy, metering, and attribution |
| `crates/gateway-proto` | Configuration distribution protocol |
| `crates/gateway-wasm` | Extension runtime |
| `deploy` | Operator, Helm chart, containers, and deployment examples |
| `website` | The Next.js site at smallgateway.vercel.app |
| `docs` | Design notes and guides |
| `spikes` | Earlier experiments |
| `upstream` | Work prepared for related projects |

## Website

```sh
cd website
pnpm install --frozen-lockfile
pnpm dev
```

Run `pnpm build` in that directory to build the site. A deployment from the combined repository should use `website/` as its root directory.

## Development checks

Upgrading an existing configuration? See the [API and configuration naming
changes](docs/14-naming-migration.md).

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
(cd deploy/operator && go test ./...)
```

These are local checks. Passing them does not establish how the gateway will behave in your environment.

## Design notes

Start with the [design principles](docs/00-principles.md), [architecture](docs/02-architecture.md), and [control plane](docs/07-control-plane.md). The [build plan](docs/04-build-plan.md) records the original sequence of work; some notes describe plans rather than current behavior.

[The Gateway Baseline](https://thegatewaybaseline.com) is a related comparison of cost-attribution features. It is a useful reference for this work, not a certification.

## Contributions

Small fixes, failing examples, and clearer documentation are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md). For a security concern, contact [hello@itslyqed.com](mailto:hello@itslyqed.com) privately.

## License

Apache-2.0. See [LICENSE](LICENSE).
