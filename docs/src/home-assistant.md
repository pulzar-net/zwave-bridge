# Home Assistant

zwave-bridge works with Home Assistant through its
[MQTT integration](https://www.home-assistant.io/integrations/mqtt/) and
MQTT discovery. There is no custom integration to install, and it does not
use Home Assistant's own Z-Wave integration (which needs Z-Wave JS). Use one
or the other for a given controller, not both.

## Setup

1. Run an MQTT broker. The
   [Mosquitto add-on](https://github.com/home-assistant/addons/tree/master/mosquitto)
   is the usual choice on Home Assistant OS; any broker works.
2. Add the MQTT integration in Home Assistant (**Settings > Devices &
   services > Add integration > MQTT**) and point it at the broker.
   Discovery is on by default.
3. Point zwave-bridge at the same broker (`[mqtt] host`, `port`, and, if the
   broker needs them, `username` and `password_file`).
4. Start the bridge and [include a device](inclusion.md).

Each value appears as an entity the first time the device reports it.

## What is created

Each node becomes one Home Assistant device, identified by the network's home
id and the node id, connected through the controller. Its name is the node's
name if you [renamed](mqtt.md#rename) it, otherwise the manufacturer and
label from its [device file](device-files.md), otherwise "Z-Wave node 5".

| Value | Entity | Notes |
|---|---|---|
| Binary Switch | `switch` | on and off |
| Multilevel Switch | `light` | brightness on Z-Wave's 0 to 99 scale |
| Multilevel Sensor | `sensor` | unit, and a device class for temperature, humidity, illuminance, power, voltage, current, CO2, CO, pressure, UV, PM2.5, PM10 and VOC |
| Meter | `sensor` | energy (`total_increasing`), power, voltage, current, gas |
| Battery level | `sensor` | battery class, diagnostic |
| Binary values (Binary Sensor, battery low, ...) | `binary_sensor` | a device class from the value's name: motion, door, moisture, smoke, carbon monoxide, tamper, heat, battery |
| Notification states | `binary_sensor` or `sensor` | `binary_sensor` when the state maps to a device class (motion, door, water leak, smoke, tamper, ...), otherwise a `sensor` |

Basic and Configuration values are not given entities.

Every entity uses `zwave/bridge/state` for availability: when the bridge
stops or loses the broker, its devices show as unavailable.

## Topics

| Setting | Default | What |
|---|---|---|
| `homeassistant.discovery_prefix` | `homeassistant` | where discovery messages go, matching Home Assistant's discovery prefix |
| `homeassistant.enabled` | `true` | set `false` to publish no discovery at all |

Discovery topics have the form
`homeassistant/<component>/zwave_<home id>_<node>/<value>/config`, for
example `homeassistant/switch/zwave_c0ffee01_2/binary_switch_current/config`.
They are retained, so Home Assistant finds them again after a restart.

## Removing a device

Exclude the device with the bridge's [`exclude`](inclusion.md#exclusion)
request (or `remove_failed` for a dead one). Deleting it only in Home
Assistant leaves it in the Z-Wave network.

## Using it with other software

Discovery is the only part that is specific to Home Assistant. Node-RED,
openHAB, scripts and anything else that speaks MQTT can use the
[topics](mqtt.md) directly; set `homeassistant.enabled = false` if nothing
uses discovery.
