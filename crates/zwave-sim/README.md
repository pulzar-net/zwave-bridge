# zwave-sim

A Z-Wave Serial API controller simulator.

`Simulator` speaks the controller side of the Serial API over any byte
stream (an in-memory pipe in tests, TCP for the `zwave-sim` binary), with
simulated end nodes behind it: a mains-powered switch, a sleeping S0
sensor, S2 sensors and a SmartStart node (`node::NodeSpec`). Nodes join
when a test presses their button during inclusion, run their side of the
S0 and S2 key exchanges with this repository's security crate, answer the
interview, sleep and wake up, and send reports.

It is a test double, not an emulator of any chip's quirks; bench
recordings cover those.

Part of [zwave-bridge](https://github.com/pulzar-net/zwave-bridge), maintained by
[Pulzar](https://pulzr.net). See the [user guide](https://oss.pulzr.net/zwave-bridge/), the
[library guide](https://oss.pulzr.net/zwave-bridge/library/index.html) and the
[API documentation](https://oss.pulzr.net/zwave-bridge/api/zwave_sim/index.html).

License: MIT.
