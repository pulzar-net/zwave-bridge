---
title: "zwave-serial"
description: "Silicon Labs' Z-Wave Serial API: the protocol between a host and a controller."
---

Silicon Labs' Z-Wave Serial API: the protocol between a host and a
controller. [crates.io](https://crates.io/crates/zwave-serial), [docs.rs](https://docs.rs/zwave-serial), [API docs for `main`](https://pulzar-net.github.io/zwave-bridge/api/zwave_serial/index.html).

- `frame`: SOF data frames, ACK, NAK and CAN, the checksum, and a streaming
  decoder.
- `driver`: the link layer: three transmissions with back-off, callback ids,
  response and callback timeouts; `SerialApi` runs it over any port.
- `function` and `message`: Serial API function ids and typed messages
  for initialization, SendData with transmit reports, inclusion, exclusion,
  node information, routing information and NVM operations.
- `port`: open `/dev/...` (feature `serialport`, default) or
  `tcp://host:port`.
- `probe`: identify the controller on a port with read-only requests, in
  under three seconds, and close it: firmware, series, home id, whether it
  is primary, node count, region. `probe_blocking` for programs without an
  async runtime.

```toml
zwave-serial = "0.2"
```
