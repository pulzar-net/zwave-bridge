---
title: "The library crates"
description: "zwave-bridge is built as a stack of library crates."
---

zwave-bridge is built as a stack of library crates. The bridge binary is
the thinnest layer on top; everything else can be used without it, to build
a different bridge, a gateway, a test tool or a protocol analyzer.

| Crate | Layer | Use it to |
|---|---|---|
| [zwave-host](/zwave-bridge/library/zwave-host.html) | controller application | run a Z-Wave network from Rust |
| [zwave-cc](/zwave-bridge/library/zwave-cc.html) | command classes | encode and decode Z-Wave application frames |
| [zwave-security](/zwave-bridge/library/zwave-security.html) | S0 and S2 | encrypt, decrypt and bootstrap keys; DSKs and QR codes |
| [zwave-serial](/zwave-bridge/library/zwave-serial.html) | Serial API | talk to a controller directly; identify one |
| [zwave-sim](/zwave-bridge/library/zwave-sim.html) | test support | test Z-Wave software without hardware |
| [zwave-bridge](/zwave-bridge/library/zwave-bridge.html) | MQTT bridge | run or embed the bridge itself |

The [API documentation](https://oss.pulzr.net/zwave-bridge/api/zwave_host/index.html) covers every public item.

## Depending on the crates

The crates are on [crates.io](https://crates.io/crates/zwave-host), with their API documentation on [docs.rs](https://docs.rs/zwave-host):

```sh
cargo add zwave-host        # or zwave-serial, zwave-security, zwave-cc
cargo add --dev zwave-sim   # the simulator, for tests without hardware
```

Every crate shares the workspace version and is released together. While
the project is at 0.x, minor versions may break the API; the
[changelog](/zwave-bridge/changelog.html) says what changed.

## Conventions shared by every crate

- **No `unsafe`.** The workspace lints forbid `unsafe` code in every crate.
- **No panics on input.** Every parser of radio or serial bytes is bounds
  checked and fuzzed; a malformed frame is an error value, never a panic.
- **Bounded allocation.** Parsers allocate in proportion to their input.
- **Encoding in both directions.** Frames encode and decode both the host's
  and the controller's or node's side, so the simulator shares the
  definitions the host uses.
- **Cryptography from audited crates.** AES, CMAC, CCM and X25519 come from
  RustCrypto and dalek; the only construction written here is S2's
  CTR_DRBG, checked against NIST's test vectors.
