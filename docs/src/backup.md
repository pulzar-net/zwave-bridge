# Backup and restore

A Z-Wave network lives in two places: the controller's non-volatile memory
(NVM) holds the node list and routing, and the host holds the security keys
and what it learned in each interview. Losing either is painful: without the
keys, every secure device must be excluded and included again.

## What to back up

| What | Where | How |
|---|---|---|
| Network keys | `keys.json` in the data directory | copy the file; it never changes after it is created |
| Node store | `nodes.json` in the data directory | copy the file |
| Controller memory | the controller | the `backup` request |

Back up the whole data directory after including or excluding devices.
`keys.json` holds the S0, S2 and Long Range network keys and the
controller's S2 private key: anyone with it can read and control the
network. Store backups of it as a secret.

## Backing up the controller

```sh
mosquitto_pub -t zwave/bridge/request/backup -m '{}'
```

The bridge reads the controller's NVM and writes it to
`nvm-<unix time>.bin` in the data directory:

```json
{ "file": "/var/lib/zwave-bridge/nvm-1795137434.bin", "bytes": 4096, "ok": true }
```

This works on 500, 700 and 800 series controllers. The controller is busy
while its memory is read, so do it at a quiet time.

## Restoring an NVM backup

Since 0.2.0.

> Needs real hardware: restore is tested against the simulator and real
> NVM images in CI, not yet on a controller.

`restore` writes a backup onto a controller, converting it when the
controller is of the other series (700 or 800) or runs another SDK, the
way Z-Wave JS's NVM migration does. It refuses a backup whose format is
newer than the controller's SDK can read, or whose format cannot be told.

1. Stop the bridge, plug in the replacement, start the bridge. Copy the
   backup file into the data directory if it is not there.
2. Send the request, with the file's name (not a path):

   ```sh
   mosquitto_pub -t zwave/bridge/request/restore -m '{"file": "nvm-1795137434.bin"}'
   ```

3. The bridge checks the backup against the controller, converts it if
   needed, writes it, answers `{"file": "nvm-1795137434.bin", "restart": true, "ok": true}`,
   publishes `offline`, and exits. The service manager (the systemd unit or
   the OpenWrt script) starts it again, now on the restored network.

The keys stay in `keys.json`, so keep the same data directory, or restore it
from your backup first.

Conversion covers the 700 and 800 series, whose memory is in NVM3 format.

## Moving to a new machine

Copy the data directory and the configuration, move the controller, and
start the bridge. Nothing else is needed: the network is in the controller,
the keys and node store in the data directory.
