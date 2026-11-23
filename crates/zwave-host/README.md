# zwave-host

A Z-Wave controller host stack.

The controller chip runs the radio and the mesh; this crate does the
application side a host has to do, on top of `zwave_serial`:

- inclusion and exclusion, with S2 (key exchange, DSK PIN check), S0 and
  SmartStart from a provisioning list;
- the interview: node information, secure command classes, manufacturer,
  versions, endpoints, and each command class's capabilities;
- a node store written to disk on change (batched, atomic);
- wake-up queues: commands for sleeping nodes wait for their next
  Wake Up Notification, and the node is sent back to sleep when idle;
- encapsulation in both directions: Security 0 and 2, Supervision, Multi
  Channel, Transport Service and CRC-16;
- values from reports, as `Event::Value`;
- the neighbor tables and the routes transmit reports describe, for
  topology;
- an NVM backup of the controller.

Start one with `Controller::start` and read `Event`s from the
returned channel.

Part of [zwave-bridge](https://github.com/pulzar-net/zwave-bridge), maintained by
[Pulzr](https://pulzr.net). See the [user guide](https://oss.pulzr.net/zwave-bridge/), the
[library guide](https://oss.pulzr.net/zwave-bridge/library/index.html) and the
[API documentation](https://oss.pulzr.net/zwave-bridge/api/zwave_host/index.html).

License: MIT.
