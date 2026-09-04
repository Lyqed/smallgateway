# Contributing

Open Source Gateway is a community solution. It has no owner-vendor and no
roadmap you have to lobby to change. The way in is to make a change and stand
behind it.

## The ownership contract

Read [docs/00-principles.md](docs/00-principles.md) before anything else, then
take its third principle literally: **if something breaks because of a change
you made six months ago, you are responsible.** Not blamed. Responsible. You
show up, you diagnose, you fix or revert, and the postmortem names the
mechanism, not the person.

You own your change for its whole lifetime, so the design is built to let you.
Every change is attributable and revertible: `git log` names it, `git revert`
undoes it. That is the deal. Contribute what you are willing to carry.

## Defer by default

The same principle applies to what you add. A dependency is a governance
relationship, not a `Cargo.toml` line. A wire format is a commitment. Before
you reach for a new crate, a new abstraction, or a new config knob, check that
the build-vs-reuse question is actually answered for that layer. Reversible
decisions get made fast; irreversible ones get made late and once. When in
doubt, defer, and say in the PR what you deferred and why.

## Where to start

In order:

1. **The design docs.** Read them start to finish. The reading-order table in
   [README.md](README.md) is the fast path;
   [docs/00-principles.md](docs/00-principles.md) is non-optional.
2. **The spikes.** `spikes/event-model/` and `spikes/proxy-pingora/` are the
   frozen Phase 0 evidence. They are where the canonical event model and the
   Pingora foundation were proven, and the conformance corpus (17 real
   transcripts) lives under `spikes/event-model/fixtures/real/`.
3. **The crates.** `crates/gateway-core` is the library (adapters, event
   model, metering, scopes, JWT, snapshots); `crates/gatewayd` is the
   standalone data-plane binary. Read `crates/gateway-core/src/lib.rs` to get
   the shape.

## The most accessible first contribution: a provider adapter

A provider adapter takes raw wire bytes from one LLM API and emits the
canonical event stream. It is self-contained, it has a clear correctness bar,
and it needs no control plane to test. That makes it the best place to start.

The adapters live in
[`crates/gateway-core/src/adapters/`](crates/gateway-core/src/adapters/). Three
already exist as reference: `openai.rs`, `anthropic.rs`, `bedrock.rs`. Each
implements the one-method `Adapter` trait in `mod.rs`:

```rust
pub trait Adapter {
    fn feed(&mut self, bytes: &[u8]) -> Vec<Event>;
}
```

An adapter is a synchronous push-parser with bounded internal state: it holds
only the current partial frame, never the whole response, so backpressure falls
out of the shape. The canonical events it emits are defined in
`crates/gateway-core/src/event.rs`:

```
MessageStart / ContentDelta / ToolCallDelta / UsageDelta / MessageEnd / Error
```

The invariants matter: `MessageStart` first, `MessageEnd` last, and the
terminal usage frame (the last `UsageDelta`) always precedes `MessageEnd`. Read
one of the three reference adapters end to end before writing a fourth, and
match their structure.

## The conformance suite is the bar

`crates/gateway-core/tests/conformance.rs` is the acceptance test, not an
afterthought. The same message streamed over every wire format must normalize
to the same canonical shape at any chunk boundary. A new adapter is done when
it passes the conformance suite against real fixtures at every chunk size, with
metering reconciled against the provider's terminal usage frame.

The Gateway Baseline (GB-1..GB-9) from
[antonbraverman.com/gateways](https://antonbraverman.com/gateways) is the
neutral yardstick the whole project measures itself against. The project's own
Baseline row gets verified by the same public-documentation standard as every
other gateway on the matrix. When a change touches a Baseline behavior, say
which GB item and how it is verified.

## Build and test

The toolchain is pinned in `rust-toolchain.toml` to **Rust 1.97** (the
Pingora MSRV cost accepted in Phase 0); the pin selects it for you.

```bash
cargo test              # unit + conformance
cargo clippy            # lint; keep it clean
cargo fmt               # format before you push
```

Run `cargo test` and `cargo clippy` before opening a PR, and again after any
review changes.

## Pull requests

- Keep the change focused. One adapter, one Baseline item, one fix.
- State what you deferred and why.
- If it touches a Baseline behavior, name the GB item and how it is verified.
- Write the commit and PR the way you would want to read them at 3am during an
  incident six months from now. That reader might be you.

## License

By contributing, you agree that your contributions are licensed under the
Apache License 2.0, the same license that covers the project. See
[LICENSE](LICENSE).

Participation is governed by our
[Code of Conduct](CODE_OF_CONDUCT.md).
