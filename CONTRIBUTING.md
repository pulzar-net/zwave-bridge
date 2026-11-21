# Contributing to zwave-bridge

Thanks for helping. Every kind of contribution is welcome: a report that a
controller or device works, a bug report with a log, a fix to the docs, a
new command class, support for a device.

Please follow the [code of conduct](CODE_OF_CONDUCT.md). Security problems go
through [private reporting](SECURITY.md), never a public issue.

## Before you start

- **Questions** (how do I, is this expected): ask in
  [Discussions](https://github.com/pulzar-net/zwave-bridge/discussions).
- **Hardware reports and device requests:** use the
  [device or hardware form](https://github.com/pulzar-net/zwave-bridge/issues/new?template=device.yml).
- **Bugs:** use the [bug form](https://github.com/pulzar-net/zwave-bridge/issues/new?template=bug.yml),
  with the version, your configuration and a `RUST_LOG=debug` log.
- **Larger changes** (a new command class, a change to the MQTT API or the
  configuration): open an issue first, so we can agree on the approach
  before you write the code.

## Build

You need Rust 1.89 or newer ([rustup.rs](https://rustup.rs)). The build has
no system dependencies.

```sh
git clone https://github.com/pulzar-net/zwave-bridge
cd zwave-bridge
cargo build --workspace
```

## Test

Before you send a change, run what CI runs:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

The MQTT end-to-end test needs `mosquitto` on `PATH` (`apt install
mosquitto` or `brew install mosquitto`). Without it the test is skipped,
unless `ZWAVE_BRIDGE_REQUIRE_MQTT=1` is set, as it is in CI.

The tests drive the real host against the controller simulator, whose nodes
run the real S2 node side, so a change to inclusion, security, interviews or
the bridge is tested end to end without hardware. See
[Testing](https://oss.pulzr.net/zwave-bridge/testing.html) for what
each suite covers.

If you change a configuration type, regenerate the configuration reference
and commit the result:

```sh
UPDATE_DOCS=1 cargo test -p zwave-bridge --features schema --test config_reference
```

## Run the simulator

To try a change by hand, run the simulated controller and point the bridge
at it:

```sh
cargo run -p zwave-sim -- 127.0.0.1:4545
cargo run -p zwave-bridge -- --config my-test.toml   # [serial] port = "tcp://127.0.0.1:4545"
```

[Getting started](https://oss.pulzr.net/zwave-bridge/getting-started.html)
walks through it with an MQTT broker.

## Fuzz

Every parser of untrusted bytes has a fuzz target in `fuzz/`. If you change a
parser, fuzz it for a few minutes (needs a nightly toolchain and
`cargo install cargo-fuzz`):

```sh
cd fuzz
cargo +nightly fuzz list
cargo +nightly fuzz run command_classes -- -max_total_time=300
```

A new parser needs a new fuzz target.

## Code rules

- No `unsafe` code; the workspace lints forbid it.
- No panics on input from a radio, a serial port or MQTT: bounds-check every
  read, return errors, and never `unwrap` a value that came from outside.
- Allocate in proportion to the input, never to a length the input claims.
- Every public item has a doc comment (`missing_docs` is on).
- New behavior comes with a test: a unit test against the specification or
  a test vector, or a simulator test.
- Command classes are data: add or change them in
  `crates/zwave-cc/defs/cc/*.toml`. Device knowledge comes from Z-Wave JS's
  device files, read as published; do not copy them into this repository.
- Cryptographic primitives come from RustCrypto and dalek; do not write
  your own.
- Write from the Z-Wave specifications. Do not copy code from other Z-Wave
  projects, whatever their license.

## Commits and pull requests

- One logical change per commit. The subject line says what the change does,
  in plain words, under 72 characters, for example
  `zwave-host: replace a failed node (keeps its id)`; the body says why.
- Say in the pull request what a user or a library user would notice: it
  goes into the
  [release notes](https://github.com/pulzar-net/zwave-bridge/releases).
  There is no changelog file; a release's notes are its annotated tag's
  message (`git tag -a vX.Y.Z --cleanup=whitespace -F notes.md`), which the
  release workflow publishes.
- Update the README and `docs/` when you change behavior, a configuration
  key or an MQTT topic.
- Sign off every commit (see below).

CI must pass before a pull request is merged: the tests, fuzzing and the
cross builds. Pull requests are squash-merged, so the pull
request's title and description become the commit on `main`: write them
as you would a commit message.

## Developer Certificate of Origin

Contributions are accepted under the [Developer Certificate of Origin
1.1](https://developercertificate.org): by signing off a commit, you certify
that you wrote it, or otherwise have the right to submit it under the
project's MIT license. Sign off with `git commit -s`, which adds a line like:

```text
Signed-off-by: Your Name <you@example.com>
```

using your real name. Commits without a sign-off cannot be merged.

## License

By contributing, you agree that your contributions are licensed under the
[MIT license](LICENSE).
