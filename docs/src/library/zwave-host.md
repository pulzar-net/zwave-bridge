# zwave-host

The Z-Wave controller host: the application that runs a network on top of a
Serial API controller. [API docs](https://pulzar-net.github.io/zwave-bridge/api/zwave_host/index.html).

## What it does

- **Inclusion and exclusion**, with S2 (key grant and PIN check as callbacks
  through events), S0, and SmartStart from a provisioning list.
- **Interview**, resumable per stage: node information, manufacturer,
  version, Z-Wave Plus info, endpoints, and each command class's
  capabilities.
- **Wake-up queues**: requests to sleeping nodes wait for their next wake-up.
- **Encapsulation** in both directions: S2, S0, Supervision, Multi Channel,
  Transport Service, CRC-16.
- **Values**: reports become `ValueUpdate`s with a stable `ValueId`;
  `set_value` writes switches and levels.
- **Node store**: one JSON file, written atomically, in the background.
- **NVM** backup and restore, converting between 700 and 800 series;
  **firmware updates** of devices; **device files** in Z-Wave JS's format.
- **Topology** and last routes.

## The main types

| Type | What it is |
|---|---|
| `Controller` | a running network; cheap to clone, methods take `&self` |
| `Events` | the stream of `Event`s: values, inclusion progress, prompts, interviews |
| `HostConfig` | keys, store path, timeouts, wake-up interval |
| `NetworkKeys` | the S0, S2 and Long Range keys and the controller's key pair; `generate()`, serde |
| `InclusionOptions`, `GrantPolicy` | how to include and which keys to grant |
| `ValueId`, `ValueKind`, `ValueUpdate` | values |
| `NodeRecord`, `NodeStatus` | what the host knows about a node |

`Controller::start(port, config)` takes any `Port`: a serial port opened
with `zwave_serial::port::open`, a TCP stream, or an in-memory pipe to the
simulator. The README's
[library example](https://github.com/pulzar-net/zwave-bridge#use-it-as-a-library)
includes a switch and turns it on; it runs as a doctest in CI.

## S2 prompts

`include` runs until the node is included. With `GrantPolicy::Ask` (the
default), it emits `Event::GrantSecurityClasses` and waits for
`Controller::grant_security_classes`, then `Event::ValidateDsk` and waits
for `Controller::validate_dsk` with the PIN. Pass the grant or the PIN in
`InclusionOptions` to skip the prompts.
