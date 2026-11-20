# Testing

The whole stack is developed and tested without hardware. CI runs every
test on every commit.

## Running the tests

```sh
cargo test --workspace                                   # everything below except fuzzing
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

The MQTT end-to-end test needs `mosquitto` on `PATH`. Locally it is skipped
when mosquitto is missing; set `ZWAVE_BRIDGE_REQUIRE_MQTT=1` to make that a
failure, as CI does.

## Unit tests and test vectors

- zwave-security: AES-CMAC and AES-CCM against the vectors of RFC 4493 and
  RFC 3610, and the CTR_DRBG against NIST's CAVP vectors
  (`crates/zwave-security/tests/ctr_drbg_cavp.rs`).
- zwave-cc: known frames, as devices send them, parsed and encoded again
  (`crates/zwave-cc/tests/golden.rs`).
- zwave-serial: the link layer's retransmits, timeouts and callbacks
  (`crates/zwave-serial/tests/driver.rs`).

## The simulator

[zwave-sim](library/zwave-sim.md) plays a Serial API controller with
simulated nodes. The nodes run the real S2 node side, so key exchange and
encryption are exercised end to end.

- `crates/zwave-host/tests/sim.rs`: inclusion with S2, S0 and SmartStart,
  interviews, wake-up queues, reports, exclusion, NVM backup, Long Range,
  firmware updates, replacing a failed node, and a restart that resumes from
  the store.
- `crates/zwave-bridge/tests/mqtt.rs`: the bridge with a real broker: an S2
  node included over MQTT, keys granted and the PIN typed, then a sensor
  report and its Home Assistant discovery.
- `ci/quickstart.sh`: the README's quick start as written, with the release
  binary and the current build.

## Recorded data

- `crates/zwave-host/tests/nvm_fixtures.rs` reads and rewrites NVM images
  from real 700 and 800 series controllers.
- `crates/zwave-host/tests/zwave_js_config.rs` loads device files; a CI job
  loads every file of the latest `@zwave-js/config` release.

## Fuzzing

Every parser of untrusted bytes has a
[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) target in `fuzz/`:
Serial API frames and messages, command classes, Transport Service, S0 and
S2 frames, QR codes and DSKs, NVM3 images and device files. CI fuzzes each
for 30 seconds on every commit. Locally (needs a nightly toolchain):

```sh
cargo install cargo-fuzz
cd fuzz && cargo +nightly fuzz list
cargo +nightly fuzz run command_classes -- -max_total_time=600
```

## Budgets

- **Memory:** `crates/zwave-host/tests/budget.rs` runs 200 simulated nodes
  and fails above 16 MB resident.
- **Binary size:** CI builds static `aarch64` and `mipsel` binaries and fails
  if one is over 4 MB.
- **Other architectures:** CI runs the test suite on `aarch64` and `mipsel`
  under qemu.

## Documentation

The `docs` workflow checks the claims this site makes: the
[configuration reference](config-reference.md) is regenerated from the code
and compared, every doctest runs (the README's library example among them),
the README's quick start runs, the workspace builds on the minimum supported
Rust version, and every relative link resolves.

## Real hardware

> Not done yet. Testing on real controllers and devices is the next
> milestone.
