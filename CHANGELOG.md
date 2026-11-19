# Changelog

All notable changes are listed here. The project follows
[Semantic Versioning](https://semver.org/).

## 0.1.0 (unreleased)

First release.

- `zwave-serial`: Serial API frames and link layer (ACK/NAK/CAN, three
  transmissions with back-off, callback ids, response and callback timeouts),
  typed messages for initialization, SendData with transmit reports,
  inclusion, exclusion, node information, routing information and NVM
  operations, and `probe`.
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
