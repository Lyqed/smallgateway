# Design questions

These questions keep changes within the [current scope](05-features.md).

## Can the gateway still run from a file?

The basic setup is `gatewayd` and a local configuration. The optional control plane reads desired configuration from Git and distributes snapshots to instances.

Runtime state and token counters currently live in memory. They are not a durable accounting ledger.

## Does the change preserve the response?

Supported streams pass through while adapters observe events. Ordinary JSON responses use a bounded parser to read terminal usage. A new path needs tests for its response format and failure cases.

## Who supplied the attribution?

Keep operator assignments, verified claims, derived values, and caller assertions distinct. A value being present does not prove who supplied it.

## Which number is being counted?

Live streaming counts may be estimated until provider usage arrives. Limits are in tokens. A provider's usage count is not a dollar amount.

## What happens during failure?

Check missing usage, interrupted streams, restarts, and unavailable dependencies. For fleet token limits, document the effect of stale shares and lost control-plane connectivity.

## Does this need another subsystem?

Prefer existing libraries and small adapter changes. Invoice reconciliation, billing dashboards, and general API management are outside the current scope.

## What happens when configuration changes?

A request uses the snapshot it started with. Existing rollout and rollback mechanisms are optional ways to manage several instances. See [hot-swap behavior](03-hot-swap.md).
