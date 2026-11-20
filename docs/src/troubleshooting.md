# Troubleshooting and FAQ

Start with the log. Run the bridge with `RUST_LOG=debug` to see every
request and value; see [logging](configuration.md#logging) for more.

## The bridge does not start

**`opening /dev/...: Permission denied`.** Your user (or the service's)
needs to be in the group that owns the device, usually `dialout`
(`ls -l /dev/ttyACM0` shows it). The [systemd unit](installation.md#as-a-systemd-service)
adds it.

**`opening ...: Device or resource busy`, or the controller does not
answer.** Something else has the port: Z-Wave JS, another bridge, or
ModemManager, which probes new serial devices
(`sudo systemctl stop ModemManager`). `zwave-bridge probe PORT` tells you
whether a controller answers.

**`unknown field` or a TOML error.** The configuration file has an error;
the message names the key. See the [configuration reference](config-reference.md).

**`reading .../keys.json`.** The keys file is damaged or unreadable by the
bridge's user. Restore it from your backup. Do not delete it unless you
mean to start a new network: secure devices would need to be included
again.

## Inclusion

**Nothing happens when I press the button.** Check for `inclusion_started`;
inclusion lasts `network.inclusion_timeout` seconds (default 60). Most
devices need a specific press sequence; see the manual. A device that
belonged to another network must be [excluded](inclusion.md#exclusion)
first.

**`the controller is busy with another inclusion or exclusion`.** Send
`stop`, then try again.

**`security_failed`.** The key exchange failed: usually a wrong PIN, keys
not granted in time, or the device too far away. The device is included
without the keys it failed to get. Exclude it, bring it closer, and include
it again. Each S2 step has a time limit (about a minute for the PIN), so
have the PIN ready before you start.

**The interview never finishes.** Sleeping devices only talk when awake.
Wake the device (press its button) a few times during the interview;
`reinterview` starts it again.

## Values

**A device is included but some values are missing.** It may use a command
class not implemented yet; see [Devices](hardware.md#devices). Commands it
sends that are not values appear on `zwave/nodes/<id>/command`.

**A battery device's values are late.** It reports when it wakes up, every
`network.wake_up_interval` seconds (default 3600) set at inclusion, or when
something happens (motion, a door opening). Requests to it wait for its
next wake-up.

**A node shows `dead`.** The controller could not reach it. Check its power
and range. If it is gone for good, use `remove_failed`.

## Home Assistant

**Nothing appears.** Check that both use the same broker, that the MQTT
integration has discovery on, and that `homeassistant.discovery_prefix`
matches. A value's entity is created the first time the value reports.

**Entities show as unavailable.** Home Assistant follows
`zwave/bridge/state`. If it says `offline`, the bridge stopped or lost the
broker.

## FAQ

**Can I use zwave-bridge and Z-Wave JS on one controller?** Not at the same
time: one program owns the serial port. You can move a controller between
them, but zwave-bridge cannot import Z-Wave JS's network keys yet, so secure
devices would need to be included again.

**Is the topic scheme compatible with Z-Wave JS UI's MQTT gateway?** No. It
is simpler and different.

**Can I change the controller's region?** Not through the bridge yet. Use
the manufacturer's tool.

**Does it support Z-Wave Long Range?** Since 0.2.0, with
a Long Range capable 800 series controller.

**Is there a web interface?** No. The bridge is headless; everything goes
through MQTT. Home Assistant or any MQTT tool is the interface.

**Where do I ask a question?** In
[Discussions](https://github.com/pulzar-net/zwave-bridge/discussions).
Bugs go in [issues](https://github.com/pulzar-net/zwave-bridge/issues). For security
problems, see [SECURITY.md](https://github.com/pulzar-net/zwave-bridge/blob/main/SECURITY.md).
