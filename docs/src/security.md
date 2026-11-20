# Security model

This page describes what zwave-bridge protects, what it relies on you for,
and what it cannot do. To report a vulnerability, see
[SECURITY.md](https://github.com/pulzar-net/zwave-bridge/blob/main/SECURITY.md):
privately, never in a public issue.

## What is at stake

- **The network keys** (S0, S2 Unauthenticated, Authenticated, Access
  Control, and the Long Range keys) and the controller's S2 private key, in
  `keys.json`. Whoever has them can decrypt traffic and command secure
  devices, locks included.
- **Control of devices.** Whoever can publish to `zwave/nodes/+/+/+/set` or
  `zwave/bridge/request/#` controls them, and can include and exclude
  devices.

## Threats and defenses

| Threat | Defense |
|---|---|
| Malformed or hostile frames from a nearby radio | Every parser is bounds checked, allocates in proportion to its input, and is fuzzed; no `unsafe` code. Bad frames are dropped. |
| An attacker joining during S2 inclusion (man in the middle) | S2 Authenticated and Access Control require the device's PIN, which proves the key exchange is with the device in your hand. The bridge shows the DSK so you can compare it with the label. |
| Replayed or forged S2 frames | S2 encryption with per-node nonces (SPAN) and CCM authentication; frames that fail are dropped. |
| Keys leaking through MQTT or logs | Keys are never published. Key types do not print their contents. `keys.json` is written atomically with mode 0600. |
| Path traversal through MQTT requests | `restore` and `firmware_update` take a file name inside the data directory only; paths are refused. |
| Losing the keys | Generated once and kept in one file you can back up. |

## What you are responsible for

- **The MQTT broker.** The bridge does whatever authorized MQTT clients ask,
  including inclusion and exclusion. Require authentication on the broker,
  use its access control so only the bridge and trusted clients can publish
  to `zwave/#`, and keep it off untrusted networks.
- **`keys.json`.** Back it up and protect the backups like a password.
- **Granting keys.** Grant S2 Access Control only to devices that need it
  (locks, garage doors), and check the DSK during inclusion.
- **Trace logs.** Don't post `trace` level logs publicly without checking
  them.

## Current limits

- **No TLS to the MQTT broker yet.** Run the broker on the same host or a
  trusted network, or put a TLS tunnel in between.
- **S0 is weak.** S0 sends its network key protected by a well-known key
  during inclusion. Prefer S2 devices; include S0 devices close to the
  controller.
- **Not certified.** zwave-bridge has not been through Z-Wave Alliance
  certification or a third-party security audit.
