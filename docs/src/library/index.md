# The library crates

zwave-bridge is built as a stack of library crates. The bridge binary is
the thinnest layer on top; everything else can be used without it, to build
a different bridge, a gateway, a test tool or a protocol analyzer.

| Crate | Layer | Use it to |
|---|---|---|
| [zwave-host](zwave-host.md) | controller application | run a Z-Wave network from Rust |
| [zwave-cc](zwave-cc.md) | command classes | encode and decode Z-Wave application frames |
| [zwave-security](zwave-security.md) | S0 and S2 | encrypt, decrypt and bootstrap keys; DSKs and QR codes |
| [zwave-serial](zwave-serial.md) | Serial API | talk to a controller directly; identify one |
| [zwave-sim](zwave-sim.md) | test support | test Z-Wave software without hardware |
| [zwave-bridge](zwave-bridge.md) | MQTT bridge | run or embed the bridge itself |

The [API documentation](https://pulzar-net.github.io/zwave-bridge/api/zwave_host/index.html) covers every public item.

## Depending on the crates

The crates are not published on crates.io yet. Depend on a release tag:

```toml
[dependencies]
zwave-host = { git = "https://github.com/pulzar-net/zwave-bridge", tag = "v0.2.0" }
```

Every crate shares the workspace version and is released together. While
the project is at 0.x, minor versions may break the API; the
[changelog](../changelog.md) says what changed.

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
