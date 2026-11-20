---
title: "Installation"
description: "zwave-bridge is one static Linux binary."
---

zwave-bridge is one static Linux binary. Pick one of the ways below, then
[configure](/zwave-bridge/configuration.html) it.

## Release binaries

Each [release](https://github.com/pulzar-net/zwave-bridge/releases) has
statically linked binaries with no runtime dependencies, and a `SHA256SUMS`
file:

| File | For |
|---|---|
| `zwave-bridge-x86_64-linux-musl` | PCs and servers |
| `zwave-bridge-aarch64-linux-musl` | Raspberry Pi 3, 4, 5 and other 64-bit ARM boards with a 64-bit OS |
| `zwave-bridge-mipsel-linux-musl` | MT7621 and MT7628 routers (little-endian MIPS) |
| `zwave-sim-<arch>-linux-musl` | the controller simulator, for trying the bridge without hardware |

Download, verify and install (this is the README's quick start; CI runs it):

```sh
VERSION=0.2.1
ARCH=$(uname -m)   # x86_64 or aarch64
BASE=https://github.com/pulzar-net/zwave-bridge/releases/download/v$VERSION
curl -fsSLO "$BASE/zwave-bridge-$ARCH-linux-musl"
curl -fsSLO "$BASE/SHA256SUMS"
sha256sum --check --ignore-missing SHA256SUMS
sudo install -m 0755 "zwave-bridge-$ARCH-linux-musl" /usr/local/bin/zwave-bridge
zwave-bridge --version
```

There are no builds for macOS or Windows: build from source there.
Big-endian MIPS routers are not supported.

## From crates.io

With Rust 1.89 or newer:

```sh
cargo install --locked zwave-bridge
cargo install --locked zwave-sim     # the controller simulator, if you want it
```

Or build it from a clone, `cargo build --release -p zwave-bridge`; the binary is
`target/release/zwave-bridge`. No system packages are required.

## As a systemd service

The repository has a hardened unit in
[`contrib/systemd/zwave-bridge.service`](https://github.com/pulzar-net/zwave-bridge/blob/main/contrib/systemd/zwave-bridge.service).
It runs the bridge as an unprivileged dynamic user in the `dialout` group,
keeps its data in `/var/lib/zwave-bridge`, and restarts it on failure
(including after an NVM restore, when the bridge exits on purpose).

```sh
sudo mkdir -p /etc/zwave-bridge
sudo cp config.example.toml /etc/zwave-bridge/config.toml   # then edit it
sudo install -m 0644 contrib/systemd/zwave-bridge.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now zwave-bridge
journalctl -u zwave-bridge -f
```

The unit expects the binary at `/usr/local/bin/zwave-bridge`. If your
distribution uses another group for serial devices (`uucp` on Arch Linux),
change `SupplementaryGroups`. If the broker needs a password, the unit has a
commented `LoadCredential` line that hands it to the bridge without making
the file readable to the bridge's user.

## On OpenWrt

> Needs real hardware: this procedure has not been tested on a router yet.

The `mipsel` binary runs on MT7621 and MT7628 routers with OpenWrt.

1. Install the USB serial driver for your controller's USB chip: most
   700 and 800 series sticks are CDC ACM devices (`kmod-usb-acm`); a CP210x
   bridge needs `kmod-usb-serial-cp210x`. Use `apk add` on OpenWrt 25.x,
   `opkg install` on 24.x and older.
2. Copy the binary to `/usr/bin/zwave-bridge` and make it executable.
3. Write `/etc/zwave-bridge/config.toml`. **Set `[store] dir` to a directory
   on flash**, such as `/etc/zwave-bridge/data`: `/var` is in RAM on
   OpenWrt, and the network keys would be lost at reboot.
4. Install the procd script from
   [`contrib/openwrt/zwave-bridge.init`](https://github.com/pulzar-net/zwave-bridge/blob/main/contrib/openwrt/zwave-bridge.init):

```sh
cp zwave-bridge.init /etc/init.d/zwave-bridge
chmod +x /etc/init.d/zwave-bridge
/etc/init.d/zwave-bridge enable
/etc/init.d/zwave-bridge start
logread -f -e zwave-bridge
```

Add `/etc/zwave-bridge` to `/etc/sysupgrade.conf` so a firmware upgrade keeps
it.

## Command line

```text
zwave-bridge [--config FILE]   run the bridge (default /etc/zwave-bridge/config.toml)
zwave-bridge probe PORT        identify the Z-Wave controller on PORT (a device or tcp://host:port)
zwave-bridge --version
```

Exit status: 0 on a clean stop; 1 on an error, including the deliberate
exit after an [NVM restore](/zwave-bridge/backup.html#restoring-an-nvm-backup), after which
the service manager starts it again.

The log goes to standard error. `RUST_LOG` sets its level, for example
`RUST_LOG=debug`, or `RUST_LOG=info,zwave_serial=trace` to see every serial
frame.
