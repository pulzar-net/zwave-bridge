# Hardware and firmware

zwave-bridge drives a Z-Wave controller: a USB stick or module running
Silicon Labs' Serial API firmware, attached over USB serial, a UART, or TCP
(for example through ser2net).

> **No release has been tested on real hardware yet.** "Expected" below
> means the controller speaks the Serial API the stack implements and the
> simulator covers it. Reports that something works, or does not, are very
> welcome: open a
> [hardware report](https://github.com/pulzar-net/zwave-bridge/issues/new?template=device.yml).

## Controllers

| Controller | Series | Notes | State |
|---|---|---|---|
| Zooz ZST39 LR | 800 | Long Range capable | expected |
| Home Assistant Connect ZWA-2 | 800 | Long Range capable | expected |
| Aeotec Z-Stick 7 | 700 | | expected |
| Zooz ZST10 700 | 700 | | expected |
| Silicon Labs UZB-7 | 700 | | expected |
| Aeotec Z-Stick Gen5, Gen5+ | 500 | | expected |
| Zooz ZST10 (500) | 500 | | expected |
| Z-Wave.Me UZB | 500 | | expected |
| 300 series and older | | | unsupported |
| Z-Way, hubs and bridges that do not expose the Serial API | | | unsupported |

Any 500, 700 or 800 series controller with Silicon Labs' static controller
Serial API firmware should work.

## Firmware

- **700 series:** update to SDK 7.17.2 or newer. Older 700 series firmware
  has known bugs that can make the stick stop responding.
- **800 series:** the stick's current firmware. Long Range (on `main`) needs
  a Long Range capable controller and region.
- **500 series:** the firmware it shipped with works; there is little to
  gain from updating.

Controller firmware updates are done with the manufacturer's tool, not with
zwave-bridge. zwave-bridge's [`firmware_update`](mqtt.md#firmware_update)
updates devices, not the controller.

## Regions

Z-Wave uses different frequencies in different regions (908.4 MHz in the
US, 868.4 MHz in Europe, and others). A controller and its devices must be
for the same region. The controller's region is shown in `region` on
`zwave/bridge/info`; the bridge does not change it.

## Check a controller

`probe` sends read-only Serial API requests, prints what answered, and
closes the port. It finishes within three seconds and never writes to the
controller:

```sh
zwave-bridge probe /dev/serial/by-id/usb-...
```

```text
Z-Wave controller: firmware 7.19, manufacturer 0x0086, product 0x0001:0x005d (Aeotec Z-Stick 7)
```

`no Z-Wave controller answered` and exit status 1 means nothing on that port
speaks the Serial API: check the port and that nothing else (Z-Wave JS,
ModemManager) has it open.

## Devices

Devices work through the command classes zwave-bridge implements. In 0.1.0:

| Kind | Command classes |
|---|---|
| Switches and dimmers | Basic, Binary Switch, Multilevel Switch |
| Sensors | Binary Sensor, Multilevel Sensor, Notification, Battery |
| Energy | Meter |
| Configuration and management | Configuration, Association, Version, Manufacturer Specific, Z-Wave Plus Info, Wake Up, Device Reset Locally, Application Status |
| Transport | Multi Channel, Supervision, Transport Service, CRC-16 Encapsulation |
| Security | Security 0, Security 2 |
| Firmware (not in 0.1.0) | Firmware Update Meta Data |

A device that relies on a command class not in this list includes and
interviews, but its other functions do not appear. Thermostats, locks,
color bulbs, window coverings and scene controllers are among those not
supported yet; see the [roadmap](https://github.com/pulzar-net/zwave-bridge#roadmap).

## Radio placement

Put the controller on a USB extension cable, away from the computer and
from USB 3 devices, which radiate noise. Z-Wave is a mesh: mains-powered
devices repeat for others, so add them between the controller and distant
battery devices. The [`topology`](mqtt.md#topology) request and each node's
`route` topic show how frames travel.
