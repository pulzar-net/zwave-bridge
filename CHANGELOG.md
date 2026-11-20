# Changelog

All notable changes are listed here. The project follows
[Semantic Versioning](https://semver.org/).

## 0.2.0 (2026-11-20)

- `zwave-host`: `ValueUpdate.rssi` (the carrying frame's RSSI), `Controller::queued`
  (requests waiting for a sleeping node) and `Controller::last_route`.
- `zwave-host`: store writes are serialized; `flush` racing the background
  writer could fail on the shared temporary file or leave an older snapshot.
- `zwave-host`: `device_config` reads Z-Wave JS device files as published
  (JSON5, `$import`, `$if`, partial parameters, associations, compat flags)
  from a directory or an in-memory map; `Controller::configure` writes
  settings by parameter key, merging partial parameters. CI loads every file
  of the latest `@zwave-js/config` release.
- Long Range: 16-bit node ids on the Serial API when the controller offers
  them, the Long Range node list, SmartStart inclusion over Long Range,
  separate Long Range S2 keys (Authenticated and Access Control only), no
  routing information for Long Range nodes. `zwave-serial` builders and
  frame encoders take the node id width (a breaking change).
- `zwave-host`: `nvm3` reads and writes NVM3 images (700 and 800 series,
  checked against real backups in CI); `Controller::restore_nvm` writes a
  backup back after checking it matches the controller's series and SDK.
  `zwave-bridge`: `restore` request.
- `zwave-cc`: Firmware Update Meta Data (version 8).
- `zwave-host`: `Controller::update_firmware`: CRC-checked fragments sized for
  the node's security, activation, Intel HEX images; progress events.
- `zwave-bridge`: `firmware_update` request; `[devices] dir`, device names in Home Assistant, the
  `parameters` topic and the `configure` request.

## 0.1.0 (2026-11-19)

First release.

- `zwave-serial`: Serial API frames and link layer (ACK/NAK/CAN, three
  transmissions with back-off, callback ids, response and callback timeouts),
  typed messages for initialization, SendData with transmit reports,
  inclusion, exclusion, node information, routing information and NVM
  operations; `probe` and `probe_blocking`, which identify a controller with
  read-only frames (firmware, series, home id, primary, node count, region)
  within 2.9 s and close the port.
- `zwave-security`: S0; S2 key derivation, AES-128 CTR_DRBG (NIST CAVP
  tested), SPAN and MPAN, Message Encapsulation and extensions; X25519 key
  pairs; DSK, PIN, SmartStart home ids, QR codes and the provisioning list.
- `zwave-cc`: 22 command classes generated from TOML definitions, Transport
  Service segmentation and reassembly, CRC-16, sensor/meter/notification
  tables.
- `zwave-host`: inclusion and exclusion with S2 (PIN check), S0 and
  SmartStart; interview with resume; node store; wake-up queues; Supervision,
  Multi Channel and Transport Service in both directions; values; topology;
  NVM backup (700/800 series and 500 series).
- `zwave-sim`: Serial API controller simulator with simulated nodes.
- `zwave-bridge`: MQTT bridge with Home Assistant discovery and a JSON topic
  scheme.
