# Device files

Since 0.2.0.

Z-Wave devices are configured with numbered parameters: "parameter 3, size
1, value 20". What each number means is in the device's manual, and in
[Z-Wave JS](https://github.com/zwave-js/zwave-js)'s device database, the
largest collection of Z-Wave device knowledge there is. zwave-bridge reads
that database's device files as published, in their own format, rather than
keeping a copy or a database of its own.

With device files configured:

- each node's record names the device (`device` in `zwave/nodes/<id>`), and
  Home Assistant shows that name;
- its parameters, with labels, descriptions, ranges, defaults, units and
  allowed options, are published on `zwave/nodes/<id>/parameters`;
- the `configure` request sets parameters by their key, partial parameters
  included.

Without them, everything else works the same; parameters just have no
names.

## Setup

Download a release of the `@zwave-js/config` package from npm (it is a
plain tarball) and point `[devices] dir` at its `config/devices` directory:

```sh
v=15.32.4
sudo mkdir -p /var/lib/zwave-bridge/zwave-js-config
curl -fsSL https://registry.npmjs.org/@zwave-js/config/-/config-$v.tgz \
    | sudo tar -xz -C /var/lib/zwave-bridge/zwave-js-config
```

```toml
[devices]
dir = "/var/lib/zwave-bridge/zwave-js-config/package/config/devices"
```

Any release works; newer releases know more devices. To update, download a
newer one and restart the bridge. zwave-bridge's CI loads every device file
of the latest `@zwave-js/config` release, so a change in the format is
caught early.

The files are read as Z-Wave JS writes them: JSON with comments, `$import`
templates (transitive), `$if` conditions on the firmware version, partial
parameters, compat flags and associations.

## Parameters

`zwave/nodes/<id>/parameters` holds one entry per parameter:

```json
[
  {
    "key": "3",
    "label": "LED Indicator",
    "description": "Choose when the LED is on.",
    "size": 1,
    "min": 0,
    "max": 2,
    "default": 0,
    "unsigned": true,
    "read_only": false,
    "unit": null,
    "options": [ { "value": 0, "label": "On when load is off" }, { "value": 1, "label": "On when load is on" } ]
  }
]
```

`key` is the parameter number, or for a partial parameter the number and a
bit mask, such as `"41[0xf00]"`.

## Setting parameters

```sh
mosquitto_pub -t zwave/bridge/request/configure -m '{"node": 5, "settings": {"3": 1, "41[0xf00]": 2}}'
```

The bridge looks each key up in the device file, reads the current value of
a partial parameter and merges the new bits into it, writes each parameter
with Configuration Set, and reads it back so its new value is reported. The
response is `{"node": 5, "ok": true}`, or an error.

For a sleeping device, the request waits in its wake-up queue until the
device next wakes; press its button to wake it sooner.
