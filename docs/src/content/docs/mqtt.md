---
title: "MQTT API"
description: "The bridge publishes and listens under one base topic, zwave unless mqtt.base_topic says otherwise."
---

The bridge publishes and listens under one base topic, `zwave` unless
`mqtt.base_topic` says otherwise. Payloads are JSON, except the plain-text
state topics noted below. This page uses the default base topic throughout.

This page explains how the topics work together. The
[MQTT reference](/zwave-bridge/mqtt-reference.html) lists every topic with its QoS, retain flag and
payload schema, field by field. It is generated from the bridge's own types,
so it always matches the code.

Bridge topics are `zwave/bridge/state`, `info` and `event`, and requests and
responses under `zwave/bridge/request/<name>` and
`zwave/bridge/response/<name>`. Each node has `zwave/nodes/<id>` for its
record, `/status`, one retained topic per value with `/set` beside it for
the writable ones, `/route`, `/command` for the commands it sends, and
`/parameters` from its device file.

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
  "version": "0.2.1"
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

`event` names what happened; the other fields depend on it. Two events ask
for an answer during an S2 inclusion: `grant_security_classes`, answered
with the `grant` request, and `validate_dsk`, answered with the
`validate_dsk` request. See [Including devices](/zwave-bridge/inclusion.html). The
[reference](/zwave-bridge/mqtt-reference.html) lists every event and its fields.

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

An empty payload is the same as `{}`. A payload that is not JSON, or a field
of the wrong type, is answered with `"ok": false` and an `error` naming the
request. Requests that wait for a device (inclusion, a firmware update)
answer when they finish.

The requests are `include`, `exclude`, `stop`, `grant`, `validate_dsk`,
`provision`, `unprovision`, `provisioning_list`, `reinterview`,
`remove_failed`, `rename`, `topology`, `backup`, `restore`,
`firmware_update` and `configure`. The
[reference](/zwave-bridge/mqtt-reference.html) gives each one's fields and its response; the
sections below cover the ones that need more than that.

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
file back; see [Backup and restore](/zwave-bridge/backup.html). `file` must be a file name
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
[device file](/zwave-bridge/device-files.html). Partial parameters (several settings packed
in one parameter) are merged for you:

```sh
mosquitto_pub -t zwave/bridge/request/configure -m '{"node": 5, "settings": {"3": 20, "41[0xf00]": 2}}'
```

## Node topics

### `nodes/<id>`

The node record, retained: what the interview found and what people named
it, with its `status`. With a device file (since 0.2.0), `device` names the
device and, for a multi-channel device, its channels, as
`{"1": "Relay 1", "2": "Relay 2"}` (`endpoints`, unreleased). The
[reference](/zwave-bridge/mqtt-reference.html) lists every field.

### `nodes/<id>/<value>`

Every value a node reports is a retained topic of its own. The value's
path is `<command class>/<property>`, with `/<endpoint>` added for endpoints
other than 0:

```json
{ "time": 1795137000000, "label": "Air temperature", "value": 23.5, "unit": "°C" }
```

That is `zwave/nodes/3/multilevel_sensor/air_temperature`. A switch's value
is `true` or `false`, a dimmer's a level from 0 to 99, and a notification's
the event's id with its name in `state`, as on
`zwave/nodes/3/notification/home_security_motion_sensor_status`:

```json
{ "time": 1795137000000, "label": "Home Security Motion sensor status", "value": 8, "state": "Motion detection" }
```

`time` is when the bridge received it, in Unix milliseconds.

### `nodes/<id>/<value>/set`

Set a writable value (switches and dimmers). `ON` or `TRUE` turns a
switch on and a dimmer on at its last level; `OFF` or `FALSE` turns either
off; a number turns a switch on unless it is 0 and sets a dimmer to that
level, 0 to 99. The value can also come as `{"value": ...}`.

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
See [Home Assistant](/zwave-bridge/home-assistant.html).
