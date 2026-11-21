# zwave-cc

Z-Wave command classes, generated from data definitions.

Every command class in `defs/cc/*.toml` becomes a module with one struct
per command (`multilevel_sensor::Report`, `wake_up::IntervalSet`, ...),
a `Command` enum per class, and the crate-level `Command` that
dispatches on the class id. Parsers are generated from the same
definitions as encoders, are bounded by their input, never panic and
never use `unsafe`; trailing bytes from newer versions are ignored.

The sensor, meter and notification types assigned by the Z-Wave Alliance
are in `tables`. Transport Service segmentation and reassembly
(`transport_service`) and CRC-16 encapsulation (`crc16`) are written
by hand, because their command ids carry data bits.

```rust
use zwave_cc::{Command, multilevel_sensor};

let frame = [0x31, 0x05, 0x01, 0x22, 0x00, 0xe1];
let Command::MultilevelSensor(multilevel_sensor::Command::Report(r)) = Command::parse(&frame).unwrap() else { panic!() };
assert_eq!(r.sensor_type, 1); // air temperature
assert_eq!(r.value.to_f64(), 22.5);
assert_eq!(Command::from(r).encode(), frame);
```

Part of [zwave-bridge](https://github.com/pulzar-net/zwave-bridge), maintained by
[Pulzar](https://pulzr.net). See the [user guide](https://oss.pulzr.net/zwave-bridge/), the
[library guide](https://oss.pulzr.net/zwave-bridge/library/index.html) and the
[API documentation](https://oss.pulzr.net/zwave-bridge/api/zwave_cc/index.html).

License: MIT.
