---
title: "zwave-bridge (crate)"
description: "The bridge as a library: the same code as the binary."
---

The bridge as a library: the same code as the binary.
[crates.io](https://crates.io/crates/zwave-bridge), [docs.rs](https://docs.rs/zwave-bridge), [API docs for `main`](https://oss.pulzr.net/zwave-bridge/api/zwave_bridge/index.html).

- `run(config, port, shutdown)` runs the bridge on an open port until
  `shutdown` completes.
- `load_or_create_keys(dir)` reads `keys.json`, or creates it with new keys.
- `config::Config` is the configuration file; `Config::parse` parses one.
- `discovery` builds the Home Assistant discovery messages.

The [MQTT API](/zwave-bridge/mqtt.html) and [configuration reference](/zwave-bridge/config-reference.html)
document the behavior.
