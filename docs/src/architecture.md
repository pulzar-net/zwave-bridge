# Architecture

## Who does what

A Z-Wave controller is split in two:

- The **controller firmware** (Silicon Labs' Serial API) runs the radio,
  the MAC, and mesh routing, and keeps the node list and routes in its
  non-volatile memory.
- The **host** is software on the computer. Z-Wave puts more in the host
  than most radio protocols: S0 and S2 encryption and key exchange,
  interviews, command classes, wake-up queues, and every decision about the
  network.

zwave-bridge is the host. It talks to the controller over a serial line (or
TCP) using the Serial API.

## Layers

```text
 MQTT clients (Home Assistant, Node-RED, scripts)
        │
 zwave-bridge ─── config, topics, requests, discovery, device files
        │  Event (channel)                 ▲ Controller methods (async)
        ▼                                  │
 zwave-host ───── inclusion, S2/S0 bootstrapping, interview, wake-up queues,
        │         encapsulation, values, node store, NVM, firmware, topology
        ├── zwave-cc         command classes, Transport Service, tables
        ├── zwave-security   S0, S2, key exchange, DSK, QR
        ▼
 zwave-serial ─── Serial API frames, ACK/NAK/CAN, retransmits, callbacks
        │ serial port or TCP
 the controller (or zwave-sim)
```

- **`SerialApi`** owns the port: it frames requests, waits for the ACK,
  retransmits, matches responses and callbacks by id, and passes unsolicited
  frames (incoming application commands, inclusion callbacks) up.
- **`Controller`** owns the network's state. Its methods are async and take
  `&self`; it can be cloned and shared. It emits `Event`s on a channel.
- **The bridge** owns the MQTT session and the topic scheme. It never touches
  the serial port directly.

## The life of a value

1. A sensor sends a Multilevel Sensor Report, encrypted with S2. The
   controller passes it to the host as an application command.
2. zwave-serial checks the frame and acknowledges it.
3. zwave-host looks up the node, decrypts the S2 encapsulation with that
   node's span, unwraps Supervision or Multi Channel if present, decodes the
   command with zwave-cc, and turns it into a `ValueUpdate`
   (`multilevel_sensor/air_temperature`, 23.5 °C), emitted as
   `Event::Value`.
4. zwave-bridge publishes it on `zwave/nodes/<id>/multilevel_sensor/air_temperature`,
   and the first time, a Home Assistant discovery message for it.

Commands go the other way: a `.../set` message becomes
`Controller::set_value`, which builds the command, wraps it in Supervision
and S2 as the node requires, queues it if the node is asleep, sends it, and
reads the value back.

## Design rules

- **Never panic on input.** Every byte from the radio or the serial port is
  parsed with bounds checks, and every parser is fuzzed in CI.
- **Bounded resources.** Parsers allocate in proportion to their input; the
  event stream has a soft cap; CI checks that the host stays under 16 MB
  resident with 200 simulated nodes.
- **Never lose the keys.** Keys are generated once and written atomically
  with owner-only permissions before they are used.
- **No `unsafe` code**, forbidden by the workspace lints.
- **Small and static.** Release binaries are statically linked against musl,
  built for size, and must stay under 4 MB.
- **Data over code.** Command classes are generated from TOML definitions;
  device knowledge comes from Z-Wave JS's device files, read as published.

## Concurrency

Everything runs on tokio. The Serial API link, the controller's event loop,
its store writer and wake-up queues are tasks connected by channels. The
bridge runs one loop that selects over controller events, MQTT messages and
the stop signal; requests that wait for a device (inclusion, firmware
updates) run in their own tasks and answer when they finish.
