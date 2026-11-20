# Including devices

Including a device adds it to the network and gives it the network keys it
is entitled to. Z-Wave has three ways to do it.

| Method | When | What you need |
|---|---|---|
| Classic inclusion | any device | press its button while the bridge listens |
| S2 inclusion | devices with S2 security | classic inclusion, then grant keys and confirm the PIN |
| SmartStart | devices with a SmartStart QR code | scan the QR code once; the device includes itself when powered |

## Classic inclusion

```sh
mosquitto_pub -t zwave/bridge/request/include -m '{}'
```

The bridge publishes `inclusion_started` and listens for
`network.inclusion_timeout` seconds (default 60). Press the device's
inclusion button (often three quick presses; see its manual). The bridge
reports `node_added`, bootstraps security if the device asks for it, and
interviews it. The response comes once the device is included:

```json
{ "node": 5, "security": ["s2_authenticated"], "security_error": null, "ok": true }
```

`stop` cancels a running inclusion or exclusion.

Options for `include`:

| Field | Meaning |
|---|---|
| `strategy` | `"requested"`: grant every key the device asks for, without a prompt; `"insecure"`: include without security |
| `classes` | grant these classes without a prompt, for example `["s2_authenticated"]` |
| `pin` | the device's PIN, given up front, so the bridge does not ask |
| `timeout` | seconds to listen, instead of `network.inclusion_timeout` |

Without `strategy` or `classes`, the bridge asks (below).

## S2: granting keys and confirming the PIN

An S2 device asks for one or more security classes. The bridge asks you
which to grant:

```json
{ "event": "grant_security_classes", "node": 5, "requested": ["s2_authenticated", "s2_unauthenticated"] }
```

Answer with the classes to grant (normally all of them):

```sh
mosquitto_pub -t zwave/bridge/request/grant -m '{"classes": ["s2_authenticated", "s2_unauthenticated"]}'
```

For S2 Authenticated and S2 Access Control, the bridge must check that it is
talking to the device in your hand and not an attacker in range. It shows the
device's DSK with the first five digits hidden:

```json
{ "event": "validate_dsk", "node": 5, "dsk": "xxxxx-18393-13351-63537-04448-30748-31859-16344" }
```

Check that the rest matches the DSK on the device's label or box, and send
the hidden five digits, the PIN (often underlined on the label):

```sh
mosquitto_pub -t zwave/bridge/request/validate_dsk -m '{"pin": "31501"}'
```

The bridge then exchanges keys with the device and reports
`security_bootstrapped` with the classes it now has, or `security_failed`
with the reason. A wrong PIN fails the key exchange; the device is still
included, without those keys. Exclude it and try again.

The security classes, strongest first:

| Class | For | Name on MQTT |
|---|---|---|
| S2 Access Control | locks, garage doors | `s2_access_control` |
| S2 Authenticated | most S2 devices | `s2_authenticated` |
| S2 Unauthenticated | S2 devices without a PIN | `s2_unauthenticated` |
| S0 | older secure devices | `s0_legacy` |

## SmartStart

A SmartStart device has a QR code on its label or box, starting `90`.
Provision it once, and it includes itself, with S2, whenever it is powered
on in range: no button, no PIN prompt.

```sh
mosquitto_pub -t zwave/bridge/request/provision -m '{"qr": "90..."}'
```

where `90...` is the whole string of digits the QR code holds (scan it with
any QR reader app).

Or with the DSK instead of the QR code (`classes` defaults to S2
Authenticated and Unauthenticated; `name` is optional):

```sh
mosquitto_pub -t zwave/bridge/request/provision \
    -m '{"dsk": "65326-58454-00492-07015-12556-30608-16453-34222", "name": "hallway sensor"}'
```

`provisioning_list` lists the entries, and `unprovision` (with `dsk`)
removes one. A `provisioning_changed` event follows every change. The list
is kept in the node store.

With a Long Range capable controller (on `main`), SmartStart includes Long
Range devices over Long Range.

## Exclusion

To remove a device, or to reset a device that belonged to another network:

```sh
mosquitto_pub -t zwave/bridge/request/exclude -m '{}'
```

Then press the device's button. The response names the node removed
(`{"node": 5, "ok": true}`), or `null` for a device from another network.
Any controller can exclude any device, so excluding first is the usual way
to reset a second-hand device before including it.

A device that is dead (broken, or gone) cannot be excluded. Remove it from
the controller with `remove_failed`:

```sh
mosquitto_pub -t zwave/bridge/request/remove_failed -m '{"node": 5}'
```

The controller only accepts this for a node it has marked as failed.
