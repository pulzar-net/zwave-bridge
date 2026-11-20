# zwave-bridge

zwave-bridge is a Z-Wave controller stack and a Z-Wave to MQTT bridge,
written in Rust. It drives a Z-Wave controller stick, includes and secures
devices (S2, S0 and SmartStart), interviews them, and publishes their values
to MQTT, with Home Assistant discovery. It is a single static binary with no
runtime, small enough for routers and gateways, and a set of library crates
for anyone building Z-Wave software in Rust.

This site is the user guide and the reference. The
[README](https://github.com/pulzar-net/zwave-bridge#readme) is the
two-minute overview; the [API docs](https://pulzar-net.github.io/zwave-bridge/api/zwave_host/index.html)
cover every type and function.

## Where to start

| You want to | Read |
|---|---|
| try it without hardware | [Getting started](getting-started.md) |
| run it with a controller | [Installation](installation.md), then [Hardware and firmware](hardware.md) |
| add devices, securely | [Including devices](inclusion.md) |
| connect it to Home Assistant | [Home Assistant](home-assistant.md) |
| script it or build on its topics | [MQTT API](mqtt.md) |
| name device parameters | [Device files](device-files.md) |
| change a setting | [Configuration](configuration.md) and the [reference](config-reference.md) |
| protect the network keys | [Backup and restore](backup.md) |
| embed the stack in Rust | [Library overview](library/index.md) |
| understand how it works | [Architecture](architecture.md) |
| fix a problem | [Troubleshooting and FAQ](troubleshooting.md) |

## Status

zwave-bridge is at 0.x. Everything is tested in CI against a Serial API
controller simulator, but **no release has been tested on real hardware
yet**. Pages that describe something only real hardware can confirm say so.

Features added after the first release are marked with the version that
brought them ("since 0.2.0"). The [changelog](changelog.md) lists what each
release contains.

zwave-bridge is not certified by the Z-Wave Alliance. Z-Wave is a trademark
of Silicon Labs.

## Terms used in these docs

- **Controller**: the USB stick or module with the Z-Wave radio, running
  Silicon Labs' **Serial API** firmware. It runs the radio and mesh
  routing, and keeps the node list in its non-volatile memory (**NVM**).
- **Host**: the software that drives the controller. That is zwave-bridge:
  security, interviews and command classes run here.
- **Node**: a device on the network, numbered from 2 (the controller is
  usually node 1). **Home id**: the network's 32-bit id.
- **Inclusion** adds a node, **exclusion** removes one. **SmartStart** lets
  a device include itself when powered on, from a QR code you provisioned.
- **Command class (CC)**: a group of related commands, such as Binary
  Switch or Multilevel Sensor.
- **Security classes**: S2 Access Control, S2 Authenticated, S2
  Unauthenticated, and the older S0. Each has its own network key.
- **DSK**: a device's public key fingerprint, printed on its label as
  eight groups of five digits. The first group, often underlined, is the
  **PIN** you type during S2 inclusion.
- **Listening** nodes are always reachable; **sleeping** nodes (most battery
  devices) wake up periodically, and the bridge queues requests until they
  do.
