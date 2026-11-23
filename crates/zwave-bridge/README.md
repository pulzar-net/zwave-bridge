# zwave-bridge

The Z-Wave to MQTT bridge.

Runs a `zwave_host::Controller` on a serial port and mirrors the network
onto an MQTT broker:

| Topic | Direction | Payload |
|---|---|---|
| `<base>/bridge/state` | out, retained | `online` / `offline` (last will) |
| `<base>/bridge/info` | out, retained | controller identity |
| `<base>/bridge/event` | out | inclusion progress, key grant and PIN prompts |
| `<base>/bridge/request/<action>` | in | JSON arguments |
| `<base>/bridge/response/<action>` | out | `{"ok": true, ...}` or `{"ok": false, "error": ...}` |
| `<base>/nodes/<id>` | out, retained | the node record |
| `<base>/nodes/<id>/status` | out, retained | `alive`, `awake`, `asleep`, `dead` |
| `<base>/nodes/<id>/<class>/<property>[/<endpoint>]` | out, retained | `{"value": .., "unit": .., "time": ..}` |
| `.../set` on a writable value | in | `ON`, `OFF`, a number, or `{"value": ..}` |

With Home Assistant discovery on, each value gets a config under
`<prefix>/<component>/<device>/<object>/config` the first time it reports.

Part of [zwave-bridge](https://github.com/pulzar-net/zwave-bridge), maintained by
[Pulzr](https://pulzr.net). See the [user guide](https://oss.pulzr.net/zwave-bridge/), the
[library guide](https://oss.pulzr.net/zwave-bridge/library/index.html) and the
[API documentation](https://oss.pulzr.net/zwave-bridge/api/zwave_bridge/index.html).

License: MIT.
