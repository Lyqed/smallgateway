# smallgateway

A deliberately small gateway for LLM traffic. Experimental, by Anton Braverman.

[Website](https://smallgateway.vercel.app) · [Getting started](docs/10-getting-started.md) · [Contributing](CONTRIBUTING.md)

## Scope

Forward requests, attach attribution, meter tokens, and apply token limits. Bedrock session tags and Vertex labels carry attribution into provider billing on supported paths.

Run `gatewayd` with a local configuration file. The optional `gatewayctl` distributes configuration from Git when you need several instances.

Billing dashboards, invoice reconciliation, GPU cost allocation, and general API management are outside the current scope. The aim is to keep the request path understandable and its behavior testable.

## Try it

```sh
git clone https://github.com/Lyqed/smallgateway.git
cd smallgateway
cargo run -p gatewayd -- --config crates/gatewayd/demo/gateway.yaml
```

The demo needs a local mock upstream. Follow the [getting-started guide](docs/10-getting-started.md) for the complete setup.

## Status and docs

Still experimental. Token limits are not dollar budgets. The [Nebius and CoreWeave examples](docs/14-neoclouds.md) have been tested against local mocks, not live provider accounts.

- [Current scope](docs/05-features.md) and [design principles](docs/00-principles.md)
- [Architecture](docs/02-architecture.md) and [deployment](deploy/README.md)
- [Development checks](CONTRIBUTING.md#checking-a-change)

Gateway code is in `crates/`; the Next.js website is in `website/`. Older build plans record experiments, not a promised roadmap.

Apache-2.0. See [LICENSE](LICENSE).
