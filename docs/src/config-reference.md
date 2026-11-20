# Configuration reference

<!-- Generated from the configuration types by crates/zwave-bridge/tests/config_reference.rs. Do not edit by hand. -->

Every key the configuration file accepts. Unknown keys are an error, so a typo stops the bridge at startup instead of being ignored. This page is generated from the configuration types; a [JSON Schema](config.schema.json) generated from the same types lets editors with TOML schema support (for example Taplo or Even Better TOML) complete and check the file.

## `[serial]`

The controller's port.

This table is required.

| Key | Type | Default | Description |
|---|---|---|---|
| `port` | string | required | A serial device (`/dev/ttyUSB0`, `/dev/serial/by-id/...`) or `tcp://host:port`. |

## `[store]`

State on disk.

| Key | Type | Default | Description |
|---|---|---|---|
| `dir` | string | `"/var/lib/zwave-bridge"` | Directory for `nodes.json`, `keys.json` and NVM backups. |

## `[mqtt]`

The MQTT broker.

| Key | Type | Default | Description |
|---|---|---|---|
| `host` | string | `"127.0.0.1"` | Host. |
| `port` | integer (0 to 65535) | `1883` | Port. |
| `client_id` | string | `"zwave-bridge"` | Client id. |
| `username` | string | unset | User name. |
| `password` | string | unset | Password. |
| `password_file` | string | unset | A file holding the password (read at start), instead of `password`. |
| `base_topic` | string | `"zwave"` | Topic prefix. |
| `keep_alive` | integer (from 0) | `30` | Keep-alive, seconds. |

## `[homeassistant]`

Home Assistant MQTT discovery.

| Key | Type | Default | Description |
|---|---|---|---|
| `enabled` | boolean | `true` | Publish discovery configs. |
| `discovery_prefix` | string | `"homeassistant"` | Discovery prefix. |

## `[network]`

Network behavior.

| Key | Type | Default | Description |
|---|---|---|---|
| `wake_up_interval` | integer (from 0) | `3600` | Wake-up interval configured on sleeping nodes, seconds. |
| `inclusion_timeout` | integer (from 0) | `60` | How long inclusion and exclusion wait for a node, seconds. |

## `[devices]`

Device configuration files in Z-Wave JS's format.

| Key | Type | Default | Description |
|---|---|---|---|
| `dir` | string | unset | The `config/devices` directory of an `@zwave-js/config` release. |
