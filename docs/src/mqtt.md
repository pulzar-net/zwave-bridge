# MQTT API

The bridge publishes and listens under one base topic, `zwave` unless
`mqtt.base_topic` says otherwise. Payloads are JSON, except the plain-text
state topics noted below. This page uses the default base topic throughout.

## Topics at a glance

| Topic | Direction | Retained | What |
|---|---|---|---|
| `zwave/bridge/state` | out | yes | `online` or `offline`, plain text |
| `zwave/bridge/info` | out | yes | the controller and the network |
| `zwave/bridge/event` | out | no | inclusion, security, interview and firmware update progress |
| `zwave/bridge/request/<name>` | in | | a request to the bridge |
| `zwave/bridge/response/<name>` | out | no | the answer to a request |
| `zwave/nodes/<id>` | out | yes | the node record |
| `zwave/nodes/<id>/status` | out | yes | `alive`, `awake`, `asleep`, `dead` or `unknown`, plain text |
| `zwave/nodes/<id>/<value>` | out | yes | one value |
| `zwave/nodes/<id>/<value>/set` | in | | set a writable value |
| `zwave/nodes/<id>/route` | out | yes | how the last frame from the node arrived |
| `zwave/nodes/<id>/command` | out | no | commands the node sends, such as button presses |
| `zwave/nodes/<id>/parameters` | out | yes | its configuration parameters, from its device file (since 0.2.0) |

`<id>` is the node id, a number. Network keys are never published.

## Bridge topics

### `bridge/state`

`online` while the bridge runs; `offline` (also its MQTT last will) when it
stops or loses the broker.

### `bridge/info`

```json
{
  "home_id": "c0ffee01",
  "node_id": 1,
  "library_version": "Z-Wave 7.22",
  "library_type": "bridge controller",
  "firmware": "7.22",
  "manufacturer_id": 0,
  "product_type": 4,
  "product_id": 4,
  "chip_type": 8,
  "sis": true,
  "region": "UsaLongRange",
  "dsk": "36982-35213-13151-10130-14261-37667-55588-29374",
  "nodes": [2, 3],
  "version": "0.2.0"
}
```

`dsk` is the controller's own DSK, which some devices ask for when they
include the controller. `sis` is true when this controller is the network's
SIS (it makes itself one on a new network). `region` is the controller's RF
region.

### `bridge/event`

```json
{ "event": "node_added", "node": 5 }
```

| `event` | Other fields |
|---|---|
| `inclusion_started`, `inclusion_stopped` | |
| `exclusion_started`, `exclusion_stopped` | |
| `node_added`, `node_removed` | `node` |
| `grant_security_classes` | `node`, `requested`: the classes it asks for; answer with `grant` |
| `validate_dsk` | `node`, `dsk` with the PIN hidden; answer with `validate_dsk` |
| `security_bootstrapped` | `node`, `classes` granted |
| `security_failed` | `node`, `reason` |
| `interview_stage` | `node`, `stage` |
| `interview_completed` | `node` |
| `interview_failed` | `node`, `reason` |
| `firmware_update_progress` | `node`, `sent`, `total` fragments (since 0.2.0) |
| `firmware_update_finished` | `node`, `status`, `success` (since 0.2.0) |
| `provisioning_changed` | `entries`, the SmartStart list |

## Requests and responses

Publish a request to `zwave/bridge/request/<name>`. The answer comes on
`zwave/bridge/response/<name>`, with `"ok": true` and the result's fields,
or `"ok": false` and an `error`:

```json
{ "node": 5, "ok": true, "transaction": "a1" }
{ "ok": false, "error": "no node 9" }
```

Put a `transaction` value in a request to have it copied into the response
(since 0.2.0), so you can match answers to questions.

An empty payload is the same as `{}`. Requests that wait for a device
(inclusion, a firmware update) answer when they finish.

| Request | Fields | Response |
|---|---|---|
| `include` | `strategy`, `classes`, `pin`, `timeout` (all optional; see [Including devices](inclusion.md)) | `node`, `security`, `security_error` |
| `exclude` | `timeout` (optional) | `node` (`null` for a device from another network) |
| `stop` | | `ok`: whether an inclusion or exclusion was running |
| `grant` | `classes` | `ok` |
| `validate_dsk` | `pin` | `ok` |
| `provision` | `qr`, or `dsk` with optional `classes` and `name` | `dsk` |
| `unprovision` | `dsk` | `removed` |
| `provisioning_list` | | `entries`: `dsk`, `classes`, `node`, `name`, `active` |
| `reinterview` | `node` | |
| `remove_failed` | `node` | `node` |
| `rename` | `node`, `name`, `location` | |
| `topology` | | `controller`, `nodes`: `id`, `neighbors`, `route`, `rssi`, `listening` |
| `backup` | | `file`, `bytes` |
| `restore` | `file` (since 0.2.0) | `file`, `restart` |
| `firmware_update` | `node`, `file`, `target` (since 0.2.0) | `node`, `status`, `success`, `wait_time`, `activated` |
| `configure` | `node`, `settings` (since 0.2.0) | `node` |

### `rename`

Give a node a name and a location. Home Assistant shows the name.

```sh
mosquitto_pub -t zwave/bridge/request/rename -m '{"node": 5, "name": "Hallway sensor", "location": "Hallway"}'
```

### `topology`

Asks every node for its neighbors, then answers with the mesh as the
controller sees it:

```json
{ "controller": 1, "nodes": [ { "id": 2, "neighbors": [1, 3], "route": [], "rssi": -62, "listening": true } ], "ok": true }
```

`route` is the repeaters the last frame from the node came through, and
`rssi` its signal strength in dBm.

### `backup` and `restore`

`backup` reads the controller's NVM into `nvm-<unix time>.bin` in the data
directory and answers with its path. `restore` (since 0.2.0) writes such a
file back; see [Backup and restore](backup.md). `file` must be a file name
in the data directory, not a path.

### `firmware_update`

Since 0.2.0. Updates a device's firmware from a file you put in
`<store dir>/firmware/`. `file` is its name there; `target` is the firmware
target (0, the default, is the device's main firmware). Intel HEX files
(`.hex`) are converted; anything else (`.gbl`, `.bin`, `.ota`) is sent as it
is. Progress comes as `firmware_update_progress` events.

> Needs real hardware: firmware updates are tested against the simulator
> only. A failed update can leave a device unusable; use the image its
> manufacturer published for that exact model.

### `configure`

Since 0.2.0. Sets configuration parameters by their key in the device's
[device file](device-files.md). Partial parameters (several settings packed
in one parameter) are merged for you:

```sh
mosquitto_pub -t zwave/bridge/request/configure -m '{"node": 5, "settings": {"3": 20, "41[0xf00]": 2}}'
```

## Node topics

### `nodes/<id>`

The node record, retained: its protocol information (listening or
sleeping, device classes), interview stage, security classes,
manufacturer and product ids, firmware version, Z-Wave Plus information,
endpoints and their command classes, wake-up interval, neighbors, name,
location, DSK, and `status`. With a device file (since 0.2.0), `device`
names it: `{"file", "manufacturer", "label", "description"}`.

### `nodes/<id>/<value>`

Every value a node reports is a retained topic of its own. The value's
path is `<command class>/<property>`, with `/<endpoint>` added for endpoints
other than 0:

| Topic | Example payload |
|---|---|
| `zwave/nodes/2/binary_switch/current` | `{"value": true, "label": "Switch", "time": 1795137000000}` |
| `zwave/nodes/4/multilevel_switch/current` | `{"value": 42, "label": "Level", "time": 1795137000000}` |
| `zwave/nodes/3/multilevel_sensor/air_temperature` | `{"value": 23.5, "unit": "°C", "label": "Air temperature", "time": 1795137000000}` |
| `zwave/nodes/3/notification/home_security_motion_sensor_status` | `{"value": 8, "state": "Motion detection", "label": "Home Security Motion sensor status", "time": 1795137000000}` |
| `zwave/nodes/3/battery/level` | `{"value": 90, "unit": "%", "label": "Battery", "time": 1795137000000}` |

`time` is when the bridge received it, in Unix milliseconds. `unit` is
present when the value has one; `state` names an enumerated value.

### `nodes/<id>/<value>/set`

Set a writable value (switches and dimmers):

| Payload | Binary Switch | Multilevel Switch |
|---|---|---|
| `ON`, `TRUE` | on | on at the last level (255) |
| `OFF`, `FALSE` | off | off (0) |
| a number | on unless 0 | that level, 0 to 99 |
| `{"value": ...}` | as above | as above |

```sh
mosquitto_pub -t zwave/nodes/2/binary_switch/current/set -m ON
mosquitto_pub -t zwave/nodes/4/multilevel_switch/current/set -m 40
```

The bridge sends the command (supervised when the device supports it) and
reads the value back. Errors are logged; there is no response topic.

### `nodes/<id>/route`

```json
{ "repeaters": [3], "rssi": -71, "time": 1795137000000 }
```

### `nodes/<id>/command`

Commands a node sends that do not become values, such as a Basic Set from
a wall controller, with the command's name and its frame in hex:

```json
{ "endpoint": 0, "cc": 32, "command": "basic.set", "frame": "2001ff" }
```

## Home Assistant topics

With `homeassistant.enabled` (the default), the bridge also publishes a
discovery message under `homeassistant/` the first time each value reports.
See [Home Assistant](home-assistant.md).
