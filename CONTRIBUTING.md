# Contributing to smallgateway

Thanks for taking a look. A bug report, a clearer example, or a documentation correction is a useful contribution.

For a larger change, open an issue first so we can discuss the scope before you spend much time on it.

## Finding your way around

The gateway code is in `crates/`, deployment examples are in `deploy/`, and the website is in `website/`. The [architecture notes](docs/02-architecture.md) explain how the pieces fit together.

Provider adapters live in `crates/gateway-core/src/adapters/`. If you change an adapter, include a fixture that exercises the behavior and run the conformance tests. Fixtures must not contain credentials, personal data, or internal identifiers.

## Checking a change

For Rust code, use the toolchain in `rust-toolchain.toml`:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

For the Kubernetes operator:

```sh
cd deploy/operator
go test ./...
```

For the website:

```sh
cd website
pnpm install --frozen-lockfile
pnpm lint
pnpm build
```

## Pull requests

Explain the problem, what changes for someone using the project, and how you checked it. Mention any compatibility changes or limits you know about. Keep the scope small enough to review.

Help with follow-up fixes is appreciated. Contributing does not create a lifetime support obligation.

## Reporting problems

Include enough detail to reproduce a bug. Remove secrets and identifying information from logs and configuration before sharing them. For security concerns, contact [hello@itslyqed.com](mailto:hello@itslyqed.com) privately.

## License and conduct

Contributions use the project's [Apache-2.0 license](LICENSE). Please follow the [Code of Conduct](CODE_OF_CONDUCT.md).
