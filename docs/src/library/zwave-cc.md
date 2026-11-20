# zwave-cc

Z-Wave command classes. No I/O. [API docs](https://pulzar-net.github.io/zwave-bridge/api/zwave_cc/index.html).

Each command class is defined in a TOML file under
[`defs/cc`](https://github.com/pulzar-net/zwave-bridge/tree/main/crates/zwave-cc/defs/cc);
a build script generates typed commands with bounded parsers and encoders
from them. Adding a command or a field is a data change. `COMMAND_CLASSES`
lists the classes with their ids and names, and `Command` is any decoded
command, with `cc()` and `name()` (`binary_switch.report`).

Also here:

- `transport_service`: segmentation and reassembly of frames too large for
  one radio frame;
- `crc16`: CRC-16 encapsulation;
- `tables`: the Z-Wave Alliance's multilevel sensor types and scales, meter
  types and scales, and notification types, events and states, converted
  from Z-Wave JS's tables (MIT; see NOTICE).

Command classes in 0.1.0: Basic, Application Status, Binary Switch,
Multilevel Switch, Binary Sensor, Multilevel Sensor, Meter, Z-Wave Plus
Info, Device Reset Locally, CRC-16 Encapsulation, Transport Service, Multi
Channel, Supervision, Configuration, Notification, Manufacturer Specific,
Battery, Wake Up, Association, Version, Security 0 and Security 2; Firmware
Update Meta Data on `main`.
