# Current scope

Smallgateway focuses on the path between an application and its model provider.

## Core

- Forward requests and preserve supported response formats.
- Resolve attribution from operator policy, verified claims, or explicitly configured caller headers.
- Attach Bedrock session tags and Vertex billing labels where supported.
- Meter token usage from supported streaming and JSON responses, with missing or estimated usage identified.
- Apply token limits and configured rejection responses.
- Load and validate a local configuration file.

## Optional existing code

The repository also contains Git-based configuration distribution, fleet rollout code, a Kubernetes operator, and WASM extension experiments. These are optional. Work on them should support a concrete use of the core gateway rather than expand into a separate platform.

## Outside the current scope

Invoice import and reconciliation, chargeback dashboards, dollar pricing and budgets, shared GPU cost allocation, and a general API management suite.

Provider recipes can reuse existing adapters. Supporting a provider's wire format does not establish support for every API or billing mechanism it offers.

## What gets worked on

Reproducible bugs, attribution correctness, streaming edge cases, clearer examples, and tests against real provider behavior.

The [original build plan](04-build-plan.md) records earlier experiments. Its phase numbers and feature lists are not the current roadmap.
