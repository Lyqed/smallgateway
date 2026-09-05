# Design principles

Smallgateway is deliberately small. These preferences guide current work; earlier plans do not commit the project to additional features.

## One useful request path

Concentrate on forwarding, attribution, token metering, and token limits. Correctness, clear configuration, and useful failure messages come before new features.

Provider billing tags belong here because they must be attached before a request reaches the provider. Importing invoices and allocating their costs are outside the current scope.

## Start with a file

The gateway should work with one local configuration file. Git-based configuration distribution is optional. Existing operator and extension code can be used for experiments without becoming prerequisites for the basic setup.

## Grow when a concrete use needs it

A new feature should solve a reproducible problem for this request path. Prefer a small change to an existing adapter or policy over a new service, protocol, or subsystem.

Another project's feature list is not a backlog.

## Make failures understandable

Document what happens when usage is missing, a stream ends early, a limit is reached, or configuration changes. Tests should exercise those cases.

Keep caller assertions distinguishable from operator assignments and verified claims. Preserve the difference between an estimated token count, provider-reported usage, and billed money.

## Keep claims close to evidence

A mock test demonstrates local behavior. It does not establish live provider compatibility or invoice coverage.

The Gateway Baseline is a related comparison, not a certification or a requirement to expand this project. Link claims to the code, tests, and conditions under which they hold.
