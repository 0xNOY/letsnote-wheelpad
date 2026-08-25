# letsnote-wheelpad

[日本語: README.ja.md](README.ja.md)

`letsnote-wheelpad` brings Panasonic Let's Note WheelPad circular scrolling to Linux. It reads the physical touchpad, proxies ordinary pointer and button input through a virtual touchpad, and emits scrolling through a virtual wheel, on both Wayland and X11. Its fail-open design releases the physical device if the daemon stops so ordinary touchpad input can recover.

## Installation

Package installation is recommended because it installs the daemon together with its systemd, udev, sysusers, configuration, and migration assets. Installation itself does not automatically enable WheelPad.

### Arch Linux

Install the [AUR package](https://aur.archlinux.org/packages/letsnote-wheelpad-bin):

```sh
yay -S letsnote-wheelpad-bin
```

`letsnote-wheelpad-bin` conflicts with the alternative packages `letsnote-wheelpad` and `letsnote-wheelpad-git`.

### Debian / Ubuntu

Download the `.deb` from the [latest release](https://github.com/0xNOY/letsnote-wheelpad/releases/latest), then install it with APT:

```sh
sudo apt install ./letsnote-wheelpad_*_amd64.deb
```

### Fedora / RPM-based distributions

Download the `.rpm` from the [latest release](https://github.com/0xNOY/letsnote-wheelpad/releases/latest), then install it with DNF:

```sh
sudo dnf install ./letsnote-wheelpad-*.x86_64.rpm
```

## Quick start: recommended system mode

System mode runs the daemon as the dedicated `letsnote-wheelpad` user, does not give the login user direct access to raw evdev or `/dev/uinput`, and starts again after reboot.

### 1. Stop legacy mode if it is enabled

```sh
systemctl --user disable --now letsnote-wheelpad.service
```

Skip this step if you have never enabled the legacy user service.

### 2. Enable system mode

```sh
sudo /usr/libexec/letsnote-wheelpad-migrate enable
```

The helper proceeds only when it finds exactly one supported physical touchpad; otherwise it stops without enabling system mode. The explicit `--device /dev/input/eventN` form remains available, but always use the current path reported by `status` because event numbers can change across boots.

If `enable` reports a named ACL for your login user, inspect the exact nodes:

```sh
sudo /usr/libexec/letsnote-wheelpad-migrate status
```

Remove only your login user's entry from the reported touchpad and `/dev/uinput`, then retry `enable`:

```sh
uid="$(id -u)"
sudo /usr/bin/setfacl -x "u:${uid}" /dev/input/eventN
sudo /usr/bin/setfacl -x "u:${uid}" /dev/uinput
sudo /usr/libexec/letsnote-wheelpad-migrate enable
```

Do not use `setfacl --remove-all`. The migration helper deliberately does not delete named ACLs automatically.

## Configuration

System mode reads `/etc/letsnote-wheelpad/config.toml`. Legacy user mode reads `~/.config/letsnote-wheelpad/config.toml`. Migration does not copy the user configuration into the system configuration.

The settings most commonly adjusted are:

- `scroll.sensitivity`
- `scroll.reverse_vertical`
- `scroll.horizontal_enable`
- `scroll.detect_area_radius`
- `scroll.coordinate_y_scale`
- `scroll.minimum_rotation_radius`

The packaged configuration file documents all defaults and options. The following calibration was verified on the tested CF-SZ6; it is not a universal setting for every CF-SZ6:

```toml
device_name_regex = "SynPS/2 Synaptics TouchPad"

[scroll]
detect_area_width = 0
detect_area_radius = 1965.0
coordinate_y_scale = 1.33236
minimum_rotation_radius = 500.0
```

## Logs and status

For system mode, use:

```sh
sudo /usr/libexec/letsnote-wheelpad-migrate status
journalctl -u 'letsnote-wheelpad@*.service' -f
```

For legacy user mode, use:

```sh
journalctl --user -u letsnote-wheelpad.service -f
```

## Disable and uninstall

Before removing the package, disable system mode:

```sh
sudo /usr/libexec/letsnote-wheelpad-migrate disable
```

Also stop the legacy service if it is enabled. Arch intentionally blocks package removal while migration state or a daemon remains active.

Arch:

```sh
yay -R letsnote-wheelpad-bin
```

Debian / Ubuntu:

```sh
sudo apt remove letsnote-wheelpad
```

Fedora / RPM-based distributions:

```sh
sudo dnf remove letsnote-wheelpad
```

Disable system mode and stop all package daemons before downgrading. Direct unprepared Arch or RPM downgrades are unsupported.

## Legacy user mode

Legacy mode uses the per-user configuration and remains available for compatibility with older setups:

```sh
systemctl --user enable --now letsnote-wheelpad.service
```

Do not run legacy and system daemons at the same time. System mode is recommended for new installations.

## Tested hardware and limitations

Final v0.2.0 acceptance passed on:

- Panasonic CF-SZ6
- `SynPS/2 Synaptics TouchPad` (`0011/0002/0007`)
- Wayland / Hyprland

Other Let's Note models may require touchpad-name and coordinate calibration; this is not a compatibility guarantee. System migration requires exactly one supported physical touchpad. System mode has been tested across reboot.

There is no kinetic or coasting scroll. On Wayland, compositor routing also means `WheelUnderCursor` cannot be overridden in the same way as on X11.

## How it works

The daemon reads the physical evdev device and proxies ordinary touchpad input through a virtual touchpad. A circular gesture emits wheel events through a second virtual device while normal pointer, contact, and button behavior is preserved. Fatal errors and normal shutdown release the physical grab so ordinary input can recover.

The implementation and regression tests contain the detailed input-proxy and recovery semantics.

## Development build

```sh
git clone https://github.com/0xNOY/letsnote-wheelpad.git
cd letsnote-wheelpad
cargo build --release
cargo test
```

For normal installation, use the packaged releases so the required systemd, udev, sysusers, and migration assets are installed together.

## License and acknowledgements

Licensed under [MIT](LICENSE).

Thanks to Panasonic for the original WheelPad design, the X.Org `xf86-input-synaptics` project for its angle calculation reference, and Peter Hutterer for the libinput discussion that clarified the user-space approach.
