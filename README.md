# zwave-bridge

A Z-Wave host stack and MQTT bridge in Rust. Plug a Z-Wave controller stick
into any Linux box, point the bridge at an MQTT broker, and include, secure,
interview and use Z-Wave devices from Home Assistant or anything that speaks
MQTT. Maintained by [Pulzar](https://pulzr.net). MIT licensed.

The controller chip runs the radio and the mesh. Everything a host has to do
is here: the Serial API link layer, S0 and S2 security (on the host, as Z-Wave
requires), SmartStart, inclusion and exclusion, the interview, command
classes, wake-up queues for sleeping devices, a node store, routing
information and NVM backups.

**Status: 0.1.0, pre-release.** The stack is tested end to end against a
Serial API simulator in CI; it has not yet been run against a real stick.
Z-Wave Long Range (SmartStart, 16-bit node ids, its own S2 keys), firmware
updates and NVM restore onto the same series are in; converting a backup
between 700 and 800 series is the next step. Not certified by the Z-Wave
Alliance (see below).

## Crates

| Crate | What it does |
|---|---|
| [`zwave-serial`](crates/zwave-serial) | Serial API frames, ACK/NAK/CAN, retransmits, callback ids and timeouts; typed controller messages; `probe(port)`, which says what controller answers on a port with one read-only frame |
| [`zwave-security`](crates/zwave-security) | S0; S2 with X25519 ECDH, AES-CMAC key derivation, AES-CCM, the CTR_DRBG nonce generator, SPAN and MPAN; DSK and PIN handling; SmartStart QR codes and the provisioning list |
| [`zwave-cc`](crates/zwave-cc) | Command classes generated from TOML definitions (`defs/cc`), with bounded parsers and encoders; Transport Service; CRC-16; the Alliance's sensor, meter and notification tables |
| [`zwave-host`](crates/zwave-host) | The controller: inclusion with S2/S0/SmartStart, interview, node store, wake-up queues, encapsulation (S2, S0, Supervision, Multi Channel, Transport Service), values, topology, NVM backup |
| [`zwave-sim`](crates/zwave-sim) | A Serial API controller simulator with simulated nodes (switch, sleeping S0 sensor, S2 sensors, SmartStart), as a library and the `zwave-sim` binary |
| [`zwave-bridge`](crates/zwave-bridge) | The `zwave-bridge` binary: MQTT with Home Assistant discovery and a plain JSON topic scheme |

Command classes in this release: Basic, Application Status, Binary Switch,
Multilevel Switch, Binary Sensor, Multilevel Sensor, Meter, Z-Wave Plus Info,
Device Reset Locally, CRC-16 Encapsulation, Transport Service, Multi Channel,
Supervision, Configuration, Notification, Manufacturer Specific, Battery,
Wake Up, Association, Version, Firmware Update Meta Data, Security 0 and
Security 2.

## Running the bridge

```sh
cargo build --release -p zwave-bridge
```

`/etc/zwave-bridge/config.toml`:

```toml
[serial]
port = "/dev/serial/by-id/usb-..."   # or tcp://host:port for ser2net

[store]
dir = "/var/lib/zwave-bridge"         # nodes.json, keys.json, NVM backups

[mqtt]
host = "127.0.0.1"
port = 1883
base_topic = "zwave"
# username = "zwave"
# password_file = "/etc/zwave-bridge/mqtt-password"

[homeassistant]
enabled = true
discovery_prefix = "homeassistant"

[network]
wake_up_interval = 3600

[devices]
# config/devices of an @zwave-js/config release, read as published
dir = "/var/lib/zwave-bridge/zwave-js-config/package/config/devices"
```

Device files come from Z-Wave JS's library, read in their own format
(JSON5, `$import` templates, `$if` firmware conditions). Fetch a release once
and point `[devices] dir` at it:

```sh
v=15.32.4
mkdir -p /var/lib/zwave-bridge/zwave-js-config
curl -sL https://registry.npmjs.org/@zwave-js/config/-/config-$v.tgz | tar -xz -C /var/lib/zwave-bridge/zwave-js-config
```

With a device file, a node's record names the device, its parameters are
published on `zwave/nodes/<id>/parameters`, and `configure` writes them by
key, partial parameters included: `{"node": 5, "settings": {"3": 20, "41[0xf00]": 2}}`.

```sh
zwave-bridge --config /etc/zwave-bridge/config.toml
zwave-bridge probe /dev/ttyUSB0      # what controller is on this port?
```

On first start the bridge generates the network keys and the controller's
S2 key pair in `keys.json` (mode 0600). **Back it up and keep it secret**:
anyone with it can read and control the network, and without it secure
devices must be re-included.

### Topics

| Topic | | Payload |
|---|---|---|
| `zwave/bridge/state` | out, retained | `online` / `offline` |
| `zwave/bridge/info` | out, retained | controller identity, home id, controller DSK |
| `zwave/bridge/event` | out | `inclusion_started`, `grant_security_classes`, `validate_dsk`, `security_bootstrapped`, `interview_completed`, ... |
| `zwave/bridge/request/<action>` | in | JSON arguments |
| `zwave/bridge/response/<action>` | out | `{"ok": true, ...}` or `{"ok": false, "error": "..."}` |
| `zwave/nodes/<id>` | out, retained | node record (device classes, command classes, security, interview stage, status) |
| `zwave/nodes/<id>/status` | out, retained | `alive`, `awake`, `asleep`, `dead` |
| `zwave/nodes/<id>/<class>/<property>[/<endpoint>]` | out, retained | `{"value": 22.5, "unit": "°C", "label": "Air temperature", "time": 1763580000000}` |
| `.../set` on a switch or dimmer value | in | `ON`, `OFF`, a level, or `{"value": ...}` |
| `zwave/nodes/<id>/route` | out, retained | repeaters and RSSI of the last frame to the node |

Requests: `include`, `exclude`, `stop`, `grant`, `validate_dsk`,
`provision`, `unprovision`, `provisioning_list`, `backup`, `reinterview`,
`remove_failed`, `rename`, `topology`, `configure`, `restore`
(`{"file": "nvm-1763580000.bin"}`: writes a `backup` back onto a stick of the
same series and SDK, then the bridge exits for its service manager to
restart it), `firmware_update`
(`{"node": 5, "file": "zse40_v2.gbl"}`, a file you placed in
`<store dir>/firmware`; `.hex` images are converted, others sent as they are).

Including an S2 device:

```sh
mosquitto_pub -t zwave/bridge/request/include -m '{}'
# press the device's button; the bridge asks:
#   zwave/bridge/event {"event":"grant_security_classes","node":5,"requested":["s2_authenticated","s2_unauthenticated"]}
mosquitto_pub -t zwave/bridge/request/grant -m '{"classes":["s2_authenticated","s2_unauthenticated"]}'
#   zwave/bridge/event {"event":"validate_dsk","node":5,"dsk":"xxxxx-23669-..."}
mosquitto_pub -t zwave/bridge/request/validate_dsk -m '{"pin":"34028"}'   # the underlined digits on the label
```

SmartStart: scan the device's QR code and provision it; it joins by itself
when powered on.

```sh
mosquitto_pub -t zwave/bridge/request/provision -m '{"qr":"9001..."}'
```

## The simulator

```sh
cargo run -p zwave-sim -- 127.0.0.1:4545
# in another shell, with [serial] port = "tcp://127.0.0.1:4545"
zwave-bridge --config config.toml
```

Type `press s2`, `wake sleeper`, `temp s2 23.5`, `motion s2` or
`power smartstart` into the simulator to act on its nodes.

## Budgets

Designed for small gateways: under 16 MB resident with 200 nodes and a
static binary under 4 MB (checked in CI for aarch64 and mipsel musl).

## Development

```sh
cargo test --workspace           # unit tests, NIST and RFC vectors, simulator end to end
cargo clippy --workspace --all-targets -- -D warnings
cd fuzz && cargo +nightly fuzz run command_classes   # see fuzz/fuzz_targets
```

The MQTT end-to-end test needs `mosquitto` on `PATH` and is skipped without
it locally; CI requires it. Parsers have no `unsafe`, are bounded by their
input and are fuzzed in CI on every commit. Crypto primitives come from
RustCrypto and dalek; the only construction written here is S2's AES-128
CTR_DRBG, checked against NIST's CAVP vectors.

## Certification and trademarks

Z-Wave is a trademark of Silicon Labs. This project is not affiliated with
or certified by the Z-Wave Alliance. A product that uses it can only be
marketed as Z-Wave (name or logo) after Z-Wave Alliance certification of that
product, which requires Alliance membership.

## License

MIT, see [LICENSE](LICENSE). Data tables derived from Z-Wave JS (MIT) are
credited in [NOTICE](NOTICE).
