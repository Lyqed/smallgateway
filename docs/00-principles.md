# Design principles

These are the working preferences behind smallgateway. They can change when the code or operating experience gives us a reason.

## Keep the basic setup small

The data plane should run on its own with a configuration file. The optional control plane manages configuration across instances. Git holds the desired configuration.

This makes it possible to try the gateway without first setting up a fleet-management system.

## Add things when there is a reason

A dependency, service, or protocol adds work for the people maintaining it. Before introducing one, describe the problem it solves and the simpler options considered.

An existing gateway may already fit a team's needs. Building this project does not make it the right choice for every deployment.

## Make changes traceable

Configuration changes should be attributable and reversible. Rollout records should let an operator work out which configuration an instance received.

Emergency overrides need an expiry and an audit trail. A rollback should be a documented operation, not something the operator has to invent during an incident.

## Describe the limits

Streaming, distributed budgets, and configuration updates have failure cases. Document the behavior during partial failures, including stale configuration and possible overspend.

Tests should exercise those limits. If a claim only holds in a local test or a particular setup, say so.

## Reuse the ordinary parts

Use existing libraries for HTTP, TLS, and other routine infrastructure where they fit. Spend project effort on the gateway behavior being explored: attribution, metering, configuration, and fleet management.

## Use comparisons as references

The Gateway Baseline helps describe cost-attribution requirements. Its definitions and comparison rows may change. A claim about this project's behavior should point to code, tests, and the conditions under which it was checked.
