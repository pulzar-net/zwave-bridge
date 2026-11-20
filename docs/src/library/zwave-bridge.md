# zwave-bridge (crate)

The bridge as a library: the same code as the binary.
[API docs](https://pulzar-net.github.io/zwave-bridge/api/zwave_bridge/index.html).

- `run(config, port, shutdown)` runs the bridge on an open port until
  `shutdown` completes.
- `load_or_create_keys(dir)` reads `keys.json`, or creates it with new keys.
- `config::Config` is the configuration file; `Config::parse` parses one.
- `discovery` builds the Home Assistant discovery messages.

The [MQTT API](../mqtt.md) and [configuration reference](../config-reference.md)
document the behavior.
