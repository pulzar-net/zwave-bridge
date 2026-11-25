# zwave-bridge

[![CI](https://github.com/pulzar-net/zwave-bridge/actions/workflows/ci.yml/badge.svg)](https://github.com/pulzar-net/zwave-bridge/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/pulzar-net/zwave-bridge)](https://github.com/pulzar-net/zwave-bridge/releases/latest)
[![crates.io](https://img.shields.io/crates/v/zwave-host)](https://crates.io/crates/zwave-host)
[![docs.rs](https://docs.rs/zwave-host/badge.svg)](https://docs.rs/zwave-host)
[![Docs](https://img.shields.io/badge/docs-oss.pulzr.net-blue)](https://oss.pulzr.net/zwave-bridge/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.89-blue)](Cargo.toml)

A Z-Wave controller stack and Z-Wave to MQTT bridge, written in Rust. Plug a
Z-Wave controller stick into a Linux machine, point the bridge at an MQTT
broker, and include, secure, interview and use Z-Wave devices from Home
Assistant or anything else that speaks MQTT. It is one static binary with no
runtime: CI holds it under 4 MB, and the stack under 16 MB of memory with
200 nodes, so it fits on routers and small gateways as well as on a
Raspberry Pi or a server. The same stack is a set of library crates for
anyone building Z-Wave software in Rust.

It exists because the established Z-Wave stack, Z-Wave JS, needs a Node.js
runtime, and there was no complete, embeddable Z-Wave controller stack in
Rust.

**[Documentation](https://oss.pulzr.net/zwave-bridge/)** |
**[Quick start](#quick-start)** |
**[Hardware](#supported-hardware)** |
**[MQTT API](https://oss.pulzr.net/zwave-bridge/mqtt.html)** |
**[Library](#use-it-as-a-library)** |
**[Releases](https://github.com/pulzar-net/zwave-bridge/releases)**

## Supported hardware

A Z-Wave controller is a USB stick (or module) running Silicon Labs' Serial
API firmware. **Expected** means the controller runs firmware the stack
implements and is covered by the simulator, but nobody has reported it
working yet. If you try one, please [tell us how it went](https://github.com/pulzar-net/zwave-bridge/issues/new?template=device.yml).

| Controller | Series | Firmware | State |
|---|---|---|---|
| Zooz ZST39 LR | 800 | Serial API, SDK 7.x | expected, with Long Range |
| Home Assistant Connect ZWA-2 | 800 | Serial API, SDK 7.x | expected |
| Aeotec Z-Stick 7, Zooz ZST10 700, Silicon Labs UZB-7 | 700 | Serial API, SDK 7.17.2 or newer recommended | expected |
| Aeotec Z-Stick Gen5 and Gen5+, Zooz ZST10 (500) | 500 | Serial API, SDK 6.x | expected |
| Z-Wave.Me UZB | 500 | Z-Wave.Me firmware | expected |
| 300 series and older | | | unsupported |
| Bridges that do not speak the Serial API (Z-Way, proprietary hubs) | | | unsupported |

Devices: any Z-Wave Plus device whose command classes are implemented works
without device-specific code; with Z-Wave JS's device files configured, its
parameters have names. The [hardware page](https://oss.pulzr.net/zwave-bridge/hardware.html)
covers regions, firmware and what each command class exposes.

## Quick start

From nothing to a device in Home Assistant, on 64-bit Linux (x86_64 or
aarch64) with a Z-Wave controller stick. You need an MQTT broker (for
example Mosquitto) that Home Assistant's MQTT integration already uses.

**1. Download the release and check its checksum.**

<!-- quickstart:download -->
```sh
VERSION=0.2.1
ARCH=$(uname -m)   # x86_64 or aarch64
BASE=https://github.com/pulzar-net/zwave-bridge/releases/download/v$VERSION
curl -fsSLO "$BASE/zwave-bridge-$ARCH-linux-musl"
curl -fsSLO "$BASE/SHA256SUMS"
sha256sum --check --ignore-missing SHA256SUMS
sudo install -m 0755 "zwave-bridge-$ARCH-linux-musl" /usr/local/bin/zwave-bridge
zwave-bridge --version
```

**2. Check that the controller answers.** Use the stable name under
`/dev/serial/by-id/` rather than `/dev/ttyUSB0`, which can change between
boots.

```sh
ls /dev/serial/by-id/
sudo zwave-bridge probe /dev/serial/by-id/usb-...
```

It prints what it found, for example
`Z-Wave controller: firmware 7.19, manufacturer 0x0086, product 0x0001:0x005d (Aeotec Z-Stick 7)`.

**3. Write a minimal configuration** to `/etc/zwave-bridge/config.toml`.
Only `serial.port` is required; everything else has a default.

<!-- quickstart:config -->
```toml
[serial]
port = "/dev/serial/by-id/usb-..."   # the path from step 2

[mqtt]
host = "localhost"                   # your broker
```

**4. Run it.** On first start it generates the network keys and keeps them,
with the node store, in `/var/lib/zwave-bridge`.

<!-- quickstart:run -->
```sh
sudo zwave-bridge --config /etc/zwave-bridge/config.toml
```

**5. Include a device.** Start inclusion, then press the device's inclusion
button (see its manual).

<!-- quickstart:pair -->
```sh
mosquitto_pub -h localhost -t zwave/bridge/request/include -m '{}'
```

A device without security joins right away. A device with S2 security asks
for its keys and its PIN, the five underlined digits on its label; answer on
MQTT as the [S2 guide](https://oss.pulzr.net/zwave-bridge/inclusion.html)
shows. After the interview, the device appears in Home Assistant under
**Settings > Devices & services > MQTT**. To watch everything the bridge
publishes:

```sh
mosquitto_sub -h localhost -t 'zwave/#' -v
```

No controller yet? The [getting started guide](https://oss.pulzr.net/zwave-bridge/getting-started.html)
runs the same steps against the simulator. To run the bridge as a service,
see [installation](#installation).

## Installation

- **Release binaries.** Each [release](https://github.com/pulzar-net/zwave-bridge/releases)
  has static Linux binaries of `zwave-bridge` and the `zwave-sim` simulator
  for `x86_64`, `aarch64` (Raspberry Pi 3 and newer with a 64-bit OS) and
  `mipsel` (MT7621 and MT7628 routers), with a `SHA256SUMS` file. They have
  no runtime dependencies.
- **From crates.io**, with Rust 1.89 or newer: `cargo install --locked zwave-bridge`
  (and `cargo install --locked zwave-sim` for the simulator).
- **systemd**: a hardened unit is in [`contrib/systemd/`](contrib/systemd/zwave-bridge.service).
- **OpenWrt**: a procd init script is in [`contrib/openwrt/`](contrib/openwrt/zwave-bridge.init).

The [installation guide](https://oss.pulzr.net/zwave-bridge/installation.html)
covers each one step by step.

## Configuration

The bridge reads one TOML file, `/etc/zwave-bridge/config.toml` by default
(`--config FILE` to change it):

| Table | What it sets |
|---|---|
| `[serial]` | the controller's port: a serial device or `tcp://host:port` |
| `[store]` | the data directory: network keys, node store, NVM backups, firmware images |
| `[mqtt]` | broker, credentials (password from a file), client id, base topic |
| `[homeassistant]` | discovery on or off, discovery prefix |
| `[network]` | the wake-up interval given to sleeping devices, the inclusion timeout |
| `[devices]` | a directory of Z-Wave JS device files (since 0.2.0) |

Unknown keys are an error, so a typo stops the bridge at startup. Every key,
its type and default are in the
[configuration reference](https://oss.pulzr.net/zwave-bridge/config-reference.html),
which is generated from the code. [`config.example.toml`](config.example.toml)
shows them all. Log verbosity comes from `RUST_LOG` (default `info`).

## MQTT API

Everything lives under a base topic, `zwave` by default.

| Topic | Direction | What |
|---|---|---|
| `zwave/bridge/state` | out, retained | `online` or `offline` (also the last will) |
| `zwave/bridge/info` | out, retained | controller identity, home id, region, the controller's DSK |
| `zwave/bridge/event` | out | inclusion progress, key grant and PIN prompts, interviews, firmware update progress |
| `zwave/bridge/request/<name>` | in | `include`, `exclude`, `stop`, `grant`, `validate_dsk`, `provision`, `unprovision`, `provisioning_list`, `backup`, `reinterview`, `remove_failed`, `rename`, `topology`, `configure`, `restore`, `firmware_update` |
| `zwave/bridge/response/<name>` | out | `{"ok": true, ...}` or `{"ok": false, "error": "..."}`, with the request's `transaction` (since 0.2.0) |
| `zwave/nodes/<id>` | out, retained | the node: device classes, command classes, security, interview, status |
| `zwave/nodes/<id>/status` | out, retained | `alive`, `awake`, `asleep` or `dead` |
| `zwave/nodes/<id>/<class>/<property>[/<endpoint>]` | out, retained | a value: `{"value": 22.5, "unit": "°C", "label": "Air temperature", "time": 1795137000000}` |
| `zwave/nodes/<id>/<class>/<property>[/<endpoint>]/set` | in | `ON`, `OFF`, a level, or `{"value": ...}` |
| `zwave/nodes/<id>/route` | out, retained | repeaters and RSSI of the last frame from the node |

Network keys never appear on MQTT. The
[MQTT reference](https://oss.pulzr.net/zwave-bridge/mqtt.html) has
every request and payload with examples.

## Use it as a library

The bridge is a thin layer over library crates you can use on their own.

| Crate | What it is | |
|---|---|---|
| [`zwave-host`](crates/zwave-host) | the controller: inclusion with S2, S0 and SmartStart, interview, node store, wake-up queues, encapsulation, values, topology, NVM backup | [crates.io](https://crates.io/crates/zwave-host), [docs.rs](https://docs.rs/zwave-host) |
| [`zwave-cc`](crates/zwave-cc) | command classes generated from TOML definitions, Transport Service, CRC-16, the Alliance's sensor, meter and notification tables | [crates.io](https://crates.io/crates/zwave-cc), [docs.rs](https://docs.rs/zwave-cc) |
| [`zwave-security`](crates/zwave-security) | S0 and S2: key exchange, key derivation, encryption, nonces, DSKs, PINs, SmartStart QR codes | [crates.io](https://crates.io/crates/zwave-security), [docs.rs](https://docs.rs/zwave-security) |
| [`zwave-serial`](crates/zwave-serial) | the Serial API: frames, the ACK/NAK/CAN link layer, typed controller messages, `probe` | [crates.io](https://crates.io/crates/zwave-serial), [docs.rs](https://docs.rs/zwave-serial) |
| [`zwave-sim`](crates/zwave-sim) | a Serial API controller simulator with simulated nodes, for tests without hardware | [crates.io](https://crates.io/crates/zwave-sim), [docs.rs](https://docs.rs/zwave-sim) |
| [`zwave-bridge`](crates/zwave-bridge) | the MQTT bridge, as a binary and a library | [crates.io](https://crates.io/crates/zwave-bridge), [docs.rs](https://docs.rs/zwave-bridge) |

```sh
cargo add zwave-host        # or zwave-serial, zwave-security, zwave-cc
cargo add --dev zwave-sim   # the simulator, for tests without hardware
cargo add tokio --features macros,rt-multi-thread
```

This example starts a controller, includes a switch and turns it on. It runs
here against the simulator; on hardware, open the port with
`zwave_serial::port::open("/dev/ttyUSB0").await?` instead.

```rust
use zwave_host::{Controller, Event, HostConfig, InclusionOptions, NetworkKeys, ValueId, ValueKind};
# use zwave_sim::{node::NodeSpec, SimConfig, Simulator};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
#   let (port, sim_end) = tokio::io::duplex(16 * 1024);
#   let sim = Simulator::new(SimConfig::default()).with_node(NodeSpec::switch("switch")).spawn(sim_end);
    // New network keys. Keep them: they serialize with serde, and secure
    // devices must be included again without them.
    let keys = NetworkKeys::generate()?;
    let (controller, mut events) = Controller::start(port, HostConfig::new(keys, None)).await?;

    // Start inclusion, then press the device's button.
    let c = controller.clone();
    let inclusion = tokio::spawn(async move { c.include(InclusionOptions::default()).await });
#   while !matches!(events.recv().await, Some(Event::InclusionStarted)) {}
#   sim.press("switch").await;
    let node = inclusion.await??.node;
    while let Some(event) = events.recv().await {
        if matches!(event, Event::InterviewCompleted { node: n } if n == node) {
            break;
        }
    }

    // Turn it on: Binary Switch (command class 0x25), endpoint 0.
    let switch = ValueId { node, endpoint: 0, cc: 0x25, property: "current".into() };
    controller.set_value(&switch, &ValueKind::Bool(true)).await?;
    Ok(())
}
```

The [library guide](https://oss.pulzr.net/zwave-bridge/library/index.html)
covers each crate, and the [API docs](https://oss.pulzr.net/zwave-bridge/api/zwave_host/index.html)
cover every item.

## Architecture

```text
 Home Assistant, Node-RED, scripts
                │  MQTT (JSON, Home Assistant discovery)
┌───────────────┴───────────────────────────────────────┐
│ zwave-bridge    topics, requests, discovery, device   │
│                 files                                 │
├───────────────────────────────────────────────────────┤
│ zwave-host      inclusion, S2/S0 bootstrapping,       │
│                 interview, wake-up queues, values,    │
│                 node store, topology, NVM, firmware   │
├──────────────────┬──────────────────┬─────────────────┤
│ zwave-cc         │ zwave-security   │                 │
│ command classes  │ S0, S2, DSK, QR  │                 │
├──────────────────┴──────────────────┘                 │
│ zwave-serial    Serial API frames and link layer      │
└──────────────────────────┬────────────────────────────┘
                           │ serial or TCP
             Z-Wave controller (or zwave-sim)
                           │
               Z-Wave mesh: devices
```

The controller's firmware runs the radio and the mesh routing. Everything
above it runs in the host, including S0 and S2 encryption, which Z-Wave
requires the host to do. The
[architecture page](https://oss.pulzr.net/zwave-bridge/architecture.html)
explains the layers, the event flow and the design rules (no `unsafe`, no
panics on radio input, bounded memory).

## Compared with other projects

| | zwave-bridge | [Z-Wave JS](https://zwave-js.github.io/zwave-js/) with [Z-Wave JS UI](https://zwave-js.github.io/zwave-js-ui/) |
|---|---|---|
| Language and runtime | Rust, one static binary | Node.js |
| Command classes | 22 so far | nearly all of them |
| Device database | reads Z-Wave JS's device files | its own, the largest there is |
| Interface | MQTT and Home Assistant MQTT discovery | Home Assistant integration, MQTT, web UI |
| Small hardware | runs on 128 MB MIPS routers | needs a Node.js capable host |
| Embeddable library | yes, Rust crates | yes, TypeScript |
| Maturity | 0.x, not yet hardware-tested | mature, very widely used |

**Pick Z-Wave JS** if you want the broadest device support today, a web UI,
or Home Assistant's native Z-Wave integration: it is mature and maintained by
a large community. **Pick zwave-bridge** if you need Z-Wave on
hardware too small for Node.js, a single static binary, or a Z-Wave stack to
embed in Rust software, and your devices use the command classes it
implements. zwave-bridge reads Z-Wave JS's device files as published rather
than keeping its own database, so device knowledge is shared, not forked.

## Roadmap

- **Next:** more command classes: Central Scene, Thermostat (mode and
  setpoint), Door Lock, Color Switch.

Questions go in [Discussions](https://github.com/pulzar-net/zwave-bridge/discussions); bugs and requests in [issues](https://github.com/pulzar-net/zwave-bridge/issues).

## Contributing

Contributions are welcome, from hardware reports to code. The whole stack
can be developed and tested without hardware, using the simulator. See
[CONTRIBUTING.md](CONTRIBUTING.md) for how to build, test, fuzz and send a
change, and [the code of conduct](CODE_OF_CONDUCT.md).

## Security

On first start the bridge writes the network keys to `keys.json` in the data
directory, readable by its owner only. **Back it up and keep it secret:**
anyone with it can read and control the network, and without it secure
devices must be included again.

Please report vulnerabilities privately through
[GitHub private vulnerability reporting](https://github.com/pulzar-net/zwave-bridge/security/advisories/new),
never in a public issue. See [SECURITY.md](SECURITY.md) for what to include
and how fast we respond, and the
[security model](https://oss.pulzr.net/zwave-bridge/security.html)
for what the bridge protects.

## License

MIT. See [LICENSE](LICENSE). Data tables derived from Z-Wave JS (MIT) are
credited in [NOTICE](NOTICE).

Z-Wave is a trademark of Silicon Labs. This project is not affiliated with
or certified by the Z-Wave Alliance; a product that uses it can only be
marketed as Z-Wave after Z-Wave Alliance certification of that product.

## Maintained by Pulzr

zwave-bridge is maintained by [Pulzr](https://pulzr.net), which uses it in
its own gateways. It is developed in the open, for anyone to use.
