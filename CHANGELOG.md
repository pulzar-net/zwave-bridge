# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crates
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Every crate is ready for crates.io: homepage, docs.rs links, its README,
  LICENSE and NOTICE, and only its sources and tests in the package. CI
  packages the workspace on every push; a release publishes the crates in
  dependency order once a `CARGO_REGISTRY_TOKEN` secret exists.

## [0.2.0] - 2026-11-20

### Added

- `zwave-host`: `nvm3::convert` turns a backup into an image for another
  controller: 700 to 800 series and back, or another SDK of the same series,
  repacking the backup's files in the target's layout with the target's
  application version and RF configuration, as Z-Wave JS's `migrateNVM` does.
  It refuses a protocol format newer than the target's SDK reads and images
  whose format cannot be told. `Controller::restore_nvm` converts as needed.
- `zwave-host`: a node included with S2 Authenticated or Access Control
  keeps its full DSK in its record (`NodeRecord::dsk`), as SmartStart nodes
  already did.
- `zwave-host`: `Controller::close` stops the controller and releases the
  serial port (`SerialApi::close` underneath); before, a dropped controller
  kept the port open, so a restart in the same process could not reopen it.
- `zwave-serial`: `probe::identify` on a running link. `zwave-host`:
  `Controller::factory_reset`, `set_region`, and `configure_raw` (settings by
  parameter key without a device file, sizes read from the device).
- `zwave-host`: `Controller::replace_failed_node` gives a failed node's id to
  a new node, with security and an interview; `zwave-sim` can power a node off
  and replace it.
- `zwave-host`: `ValueUpdate.rssi` (the carrying frame's RSSI), `Controller::queued`
  (requests waiting for a sleeping node) and `Controller::last_route`.
- `zwave-host`: `device_config` reads Z-Wave JS device files as published
  (JSON5, `$import`, `$if`, partial parameters, associations, compat flags)
  from a directory or an in-memory map; `Controller::configure` writes
  settings by parameter key, merging partial parameters. CI loads every file
  of the latest `@zwave-js/config` release.
- `zwave-host`: `nvm3` reads and writes NVM3 images (700 and 800 series,
  checked against real backups in CI); `Controller::restore_nvm` writes a
  backup back after checking it matches the controller's series and SDK.
  `zwave-bridge`: `restore` request.
- `zwave-cc`: Firmware Update Meta Data (version 8).
- `zwave-host`: `Controller::update_firmware`: CRC-checked fragments sized for
  the node's security, activation, Intel HEX images; progress events.
- `zwave-bridge`: `firmware_update` request; `[devices] dir`, device names in Home Assistant, the
  `parameters` topic and the `configure` request.

### Changed

- Long Range: 16-bit node ids on the Serial API when the controller offers
  them, the Long Range node list, SmartStart inclusion over Long Range,
  separate Long Range S2 keys (Authenticated and Access Control only), no
  routing information for Long Range nodes. `zwave-serial` builders and
  frame encoders take the node id width (a breaking change).

### Fixed

- `zwave-sim`: the binary kept serving only while its stdin was open, so a
  quick start that typed `press` from a script or FIFO lost the host
  mid-inclusion. It now serves until stopped; a test drives the binary over TCP.
- `zwave-bridge`: Home Assistant shows a notification sensor's state, not the
  value's name; responses echo the request's `transaction`.
- `zwave-host`: store writes are serialized; `flush` racing the background
  writer could fail on the shared temporary file or leave an older snapshot.

## [0.1.0] - 2026-11-19

First release.

### Added

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

[Unreleased]: https://github.com/pulzar-net/zwave-bridge/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/pulzar-net/zwave-bridge/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/pulzar-net/zwave-bridge/releases/tag/v0.1.0
