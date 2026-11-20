# Getting started

This page runs zwave-bridge end to end without any Z-Wave hardware: a
simulated controller with four simulated devices takes the place of a USB
stick. Every step is the same with a real controller, except the port and
pressing real buttons.

You need:

- Rust 1.89 or newer to build the simulator ([rustup.rs](https://rustup.rs)),
  or the `zwave-sim` binary from a [release](https://github.com/pulzar-net/zwave-bridge/releases);
- an MQTT broker and its command-line clients, for example Mosquitto
  (`apt install mosquitto mosquitto-clients`, or `brew install mosquitto`);
- optionally, Home Assistant with its MQTT integration on the same broker.

## 1. Start the simulator

```sh
git clone https://github.com/pulzar-net/zwave-bridge
cd zwave-bridge
cargo run -p zwave-sim -- 127.0.0.1:4545
```

It plays a Serial API controller (SDK 7.22) on TCP port 4545 and prints its devices
and their DSKs:

```text
switch      DSK 31566-37019-48767-65092-50277-41504-00893-24718
sleeper     DSK 04010-26702-53896-26553-32586-27181-61021-63694
s2          DSK 31501-18393-13351-63537-04448-30748-31859-16344
smartstart  DSK 65326-58454-00492-07015-12556-30608-16453-34222
listening on tcp://127.0.0.1:4545
```

| Name | What it is |
|---|---|
| `switch` | a mains-powered binary switch with a meter, no security |
| `sleeper` | a battery sensor that sleeps, with S0 |
| `s2` | a sensor with S2 Authenticated and Unauthenticated |
| `smartstart` | a sensor that includes itself with SmartStart |

Leave it running: you type commands into it to act on the devices.
`press <name>` presses a device's button, `wake <name>` wakes a sleeper,
`power <name>` powers a device on, `temp <name> <celsius>` and
`motion <name>` make a sensor report, and `dsk <name>` prints a DSK.

## 2. Configure and run the bridge

In a second terminal, write `quickstart.toml`:

```toml
[serial]
port = "tcp://127.0.0.1:4545"

[mqtt]
host = "localhost"

[store]
dir = "./data"
```

and run it:

```sh
cargo run -p zwave-bridge -- --config quickstart.toml
```

```text
INFO zwave_bridge: generated network keys in ./data/keys.json
INFO zwave_host::controller: controller made SIS: true
INFO zwave_bridge: controller ready home="c0ffee01" node=1 version=Z-Wave 7.22
INFO zwave_bridge: MQTT connected
```

On this first start the bridge generated the network keys in
`data/keys.json`. With a real controller, back that file up now.

## 3. Watch MQTT

In a third terminal:

```sh
mosquitto_sub -h localhost -t 'zwave/#' -v
```

## 4. Include the switch

```sh
mosquitto_pub -h localhost -t zwave/bridge/request/include -m '{}'
```

When `zwave/bridge/event {"event":"inclusion_started"}` appears, type
`press switch` into the simulator. The bridge reports `node_added`, the
interview, and finally the response:

```text
zwave/bridge/response/include {"node":2,"security":[],"security_error":null,"ok":true}
```

Switch it on:

```sh
mosquitto_pub -h localhost -t zwave/nodes/2/binary_switch/current/set -m ON
```

## 5. Include the S2 sensor

Start inclusion again and type `press s2`. This device asks for keys:

```text
zwave/bridge/event {"event":"grant_security_classes","node":3,"requested":["s2_authenticated","s2_unauthenticated"]}
```

Grant them:

```sh
mosquitto_pub -h localhost -t zwave/bridge/request/grant \
    -m '{"classes": ["s2_authenticated", "s2_unauthenticated"]}'
```

The bridge then shows the device's DSK with the first five digits hidden,
and waits for you to confirm them:

```text
zwave/bridge/event {"event":"validate_dsk","node":3,"dsk":"xxxxx-18393-13351-63537-04448-30748-31859-16344"}
```

The PIN is the first group of the DSK the simulator printed for `s2`
(on a real device, the underlined digits on its label):

```sh
mosquitto_pub -h localhost -t zwave/bridge/request/validate_dsk -m '{"pin": "31501"}'
```

`security_bootstrapped` follows, then the interview. Make it report:
type `temp s2 23.5` into the simulator.

If Home Assistant is connected to the same broker, both devices are now
under **Settings > Devices & services > MQTT**.

## Next steps

- [Install](installation.md) a release binary as a service.
- [Choose a controller](hardware.md), then set `serial.port` to it. Use
  `zwave-bridge probe PORT` first to check that it answers.
- Read [Including devices](inclusion.md) for SmartStart, S0 and exclusion,
  and the [MQTT API](mqtt.md) for every request.
