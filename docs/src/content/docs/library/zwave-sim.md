---
title: "zwave-sim"
description: "A Serial API controller simulator with simulated nodes, for testing Z-Wave hosts without hardware."
---

A Serial API controller simulator with simulated nodes, for testing Z-Wave
hosts without hardware. [crates.io](https://crates.io/crates/zwave-sim), [docs.rs](https://docs.rs/zwave-sim), [API docs for `main`](https://oss.pulzr.net/zwave-bridge/api/zwave_sim/index.html).

`Simulator` plays a controller over any byte stream, with nodes from
`NodeSpec`: `switch`, `sleeping_sensor` (S0), `s2_sensor` (with the
classes it asks for), `smartstart_sensor`, `long_range_sensor` (on
`main`), or your own. Nodes run the real security crate's node side, so S2
bootstrapping, encryption and nonces are exercised for real. `spawn`
returns a `SimHandle` to act on the nodes (`press`, `wake_up`,
`power_on`, `set_sensor`, `motion`) and to `attach` a reconnecting host.

The `zwave-sim` binary serves it on TCP:

```sh
cargo run -p zwave-sim -- 127.0.0.1:4545
```

and reads commands on standard input: `press <name>`, `wake <name>`,
`power <name>`, `temp <name> <celsius>`, `motion <name>`, `dsk <name>`.
See [Getting started](/zwave-bridge/getting-started.html).

The simulator is a test tool. It models what a host sees, not radio
propagation, and is not a reference implementation of the controller
firmware.
