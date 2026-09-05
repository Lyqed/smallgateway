# smallgateway

A small gateway for knowing who used the models and carrying that identity into provider billing where supported. An experimental project by Anton Braverman.

[Website](https://smallgateway.vercel.app) · [Getting started](docs/10-getting-started.md) · [Contributing](CONTRIBUTING.md)

## What it does

`gatewayd` sits between an application and its model provider. It forwards requests, tracks token usage, resolves attribution from policy and verified claims, and applies token limits. It can attach Bedrock session tags and Vertex labels on supported paths.

`gatewayctl` reads configuration from Git and rolls it out to gateway instances. You can also run `gatewayd` on its own with a local configuration file.

The repository includes provider adapters, streaming tests, a Kubernetes operator, and deployment examples. It is still experimental. There is no stable release or production support commitment yet.

## Why I started this

The question was simple enough: several teams were calling models, and someone needed to split the cloud bill between them.

I was using Azure API Management to put a common entry point in front of model providers across AWS, Google Cloud, and Azure. That part made sense. Applications had somewhere to send requests, and the platform team had somewhere to put identity checks and policy. APIM gave me a useful starting point for thinking about the problem.

The billing work was less uniform. On Bedrock, the attribution travelled with the credentials. On supported Vertex calls, it went in the request body. The same team name had to survive very different paths before it could mean anything in a cost report. Recording tokens was useful, but it did not finish that job.

Smallgateway grew out of wanting to work on that part in a codebase I could own and understand: establish who is calling, carry the attribution as far as the provider allows, and make the remaining gaps visible. A few instances should be manageable from Git, whether they run in a cluster or elsewhere.

There are already open-source API management platforms, including [WSO2 API Manager](https://apim.docs.wso2.com/en/latest/get-started/overview/). APIM itself supports [self-hosted gateways across clouds](https://learn.microsoft.com/en-us/azure/api-management/self-hosted-gateway-overview). Smallgateway borrows from that experience and concentrates on a narrower problem. Building it is a way to explore that problem, not evidence that everyone else should replace their gateway.

## What to focus on next

The next useful milestone is a small, reproducible billing exercise: requests from two teams, a provider export, and a report that explains which charges can be matched and which cannot.

- Preserve the origin of each identity value: operator policy, verified claim, or caller assertion.
- Keep provider charges, token-based estimates, and allocations of shared compute costs distinguishable.
- Retain the billing period, currency, source record, and allocation rule so a disputed number can be explained.
- Make missing usage, idle capacity, and unmatched charges visible instead of assigning them a convenient zero.

The request path and attribution mechanisms exist today. Automated invoice import and reconciliation, dollar budgets, and allocation of GPU costs are follow-up work. Current limits are measured in tokens. Provider-reported token counts are not billed dollars.

## Nebius and CoreWeave

The existing `openai` wire adapter can be configured for Nebius Token Factory and CoreWeave's OpenAI-compatible inference endpoints. See the [configuration examples and billing notes](docs/14-neoclouds.md). The recipes are tested locally against mocks; live provider behavior and invoice joins remain unverified.

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
