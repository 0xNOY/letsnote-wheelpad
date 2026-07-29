# letsnote-wheelpad

> 日本語版は [README.ja.md](README.ja.md) を参照してください。

A userland Linux daemon that reproduces the **Panasonic Let's Note "WheelPad"** circular touchpad scrolling behaviour. Draw a slow circle in the outer ring of your touchpad to scroll vertically — just like on Windows.

Works on Wayland and X11 by reading evdev events directly from the physical Synaptics touchpad and emitting wheel events through a `uinput` virtual device. The physical pad keeps driving the cursor as normal; this daemon contributes scroll only.

## Why this exists

`libinput` rejected adding circular scrolling to the Wayland-era stack (see Peter Hutterer's 2015 reasoning). So if you want your Let's Note's circular scroll to work on Linux, the only path is a userland daemon that reads the touchpad through evdev and emits wheel events through a separate virtual device. That's what this is.

## Install

### Ubuntu / Debian

```sh
sudo dpkg -i letsnote-wheelpad_0.1.0_amd64.deb
systemctl --user enable --now letsnote-wheelpad.service
```

### Fedora / RHEL

```sh
sudo rpm -i letsnote-wheelpad-0.1.0-1.x86_64.rpm
systemctl --user enable --now letsnote-wheelpad.service
```

### Arch

```sh
yay -S letsnote-wheelpad      # AUR
systemctl --user enable --now letsnote-wheelpad.service
```

### From source

```sh
git clone https://github.com/Nerahikada/letsnote-wheelpad
cd letsnote-wheelpad
cargo build --release
sudo install -Dm755 target/release/letsnote-wheelpad /usr/bin/letsnote-wheelpad
sudo install -Dm644 packaging/udev/70-letsnote-wheelpad.rules /etc/udev/rules.d/70-letsnote-wheelpad.rules
sudo install -Dm644 packaging/systemd/letsnote-wheelpad.service /etc/systemd/user/letsnote-wheelpad.service
sudo install -Dm644 packaging/modules-load/letsnote-wheelpad.conf /etc/modules-load.d/letsnote-wheelpad.conf
sudo udevadm control --reload-rules && sudo udevadm trigger
sudo modprobe uinput
systemctl --user daemon-reload
systemctl --user enable --now letsnote-wheelpad.service
```

## Configuration

Configuration lives in `~/.config/letsnote-wheelpad/config.toml`. All keys are optional; defaults match the Windows out-of-box behaviour.

```toml
# Auto-detected by name regex. Override only if you have a non-standard pad.
# device = "/dev/input/event4"
# device_name_regex = "Synaptics.*TM3562"

[scroll]
enable               = true   # master enable
reverse_vertical     = false  # flip vertical scroll direction
horizontal_enable    = false  # enable bottom-edge horizontal-scroll wedge
reverse_horizontal   = false
sensitivity          = 0      # -2..+2 ; lower = less sensitive
detect_area_width    = 0      # 0..10 ; 0 = outer ring only, 10 = whole pad
detect_area_radius   = 200.0  # inner radius at width 0, in X-axis units
coordinate_y_scale   = 1.0    # multiplier for Y; use X range / Y range
minimum_rotation_radius = 0.0 # reject tighter circles; 0 disables the limit
horizontal_start     = 2      # arc start in π/8 units (2 → 45°)
horizontal_end       = 6      # arc end in π/8 units (6 → 135°)

[log]
level = "info"  # trace | debug | info | warn | error
```

| Key | Default | Range | Notes |
| --- | --- | --- | --- |
| `scroll.enable` | `true` | bool | Disable to keep the daemon alive but suppress all scroll. |
| `scroll.reverse_vertical` | `false` | bool | "Natural" scroll = `true`. |
| `scroll.horizontal_enable` | `false` | bool | Off by default; same as Windows. |
| `scroll.reverse_horizontal` | `false` | bool | |
| `scroll.sensitivity` | `0` | -2..+2 | Indexes the multiplier table `[10, 14, 20, 28, 40]`. |
| `scroll.detect_area_width` | `0` | 0..10 | `0` = require finger near the edge; `10` = whole pad. |
| `scroll.detect_area_radius` | `200.0` | > 0 | Inner dead-zone radius at width `0`, in raw X-axis units. Increase it if the active ring is too wide. |
| `scroll.coordinate_y_scale` | `1.0` | > 0 | Multiplier applied to every Y distance. Set it to `X range / Y range` when a circular pad reports anisotropic coordinates. |
| `scroll.minimum_rotation_radius` | `0.0` | ≥ 0 | Minimum local circle radius in corrected X-axis units. Smaller circular motions are ignored; `0` disables the limit. |
| `scroll.horizontal_start` | `2` | 0..15 | π/8 units. Default 45° → 135° = the bottom edge of the pad. |
| `scroll.horizontal_end` | `6` | 0..15 | |

### CF-SZ6 (SYN0502)

On one CF-SZ6, the circular pad was observed as
`SynPS/2 Synaptics TouchPad`, with X=1210..5780 and Y=1250..4680. Vertical
circular scrolling was verified with:

```toml
device_name_regex = "SynPS/2 Synaptics TouchPad"

[scroll]
detect_area_width  = 0
detect_area_radius = 1965.0
coordinate_y_scale = 1.33236 # (5780 - 1210) / (4680 - 1250)
minimum_rotation_radius = 500.0
```

### View logs

```sh
journalctl --user -u letsnote-wheelpad -f
```

If scrolling feels too fast or too slow, adjust `scroll.sensitivity` in the config (-2..+2). The daemon does not auto-calibrate — history capacity is fixed at 20 slots to match Windows exactly (see DECISIONS.md D-021-followup).

## Known issues / non-goals

- **`WheelUnderCursor` is not configurable.** On Wayland the compositor routes input to the focused surface; there's no userland override.
- **Vertical circular scrolling is tested on Synaptics TM3562-3 and CF-SZ6 SYN0502.** Other touchpads may work with `device_name_regex` and coordinate calibration, but no compatibility promises.
- **Excel arrow-key fallback is gone.** Modern Excel routes horizontal wheel events natively; we don't need the Windows hack.
- **No coasting/kinetic scrolling.** Matches the Windows WheelPad behaviour; xf86 has it but we don't.

## How it works (one-paragraph version)

The daemon takes exclusive ownership of the physical touchpad at startup (`EVIOCGRAB`, held forever) and creates two virtual `uinput` devices that libinput attaches to instead: a touchpad mirror (same capabilities as the physical pad) and a wheel. All physical touch events are forwarded verbatim to the virtual touchpad — so cursor, taps, clicks, and multi-finger gestures keep working exactly as before. When a 6-state FSM (`Idle → Contact → Moving → Scrolling → Debounce`) decides a finger is drawing a circle in the outer ring, we **suppress** the forwarding for that gesture's duration (cursor freezes, as desired) and integrate chord-direction angles into an accumulator. Each ±π crossing emits one wheel notch on the virtual wheel. When the finger lifts, we forward the lift event (with position stripped) so libinput sees a clean end-of-gesture without a synthetic cursor jump.

For the full algorithm details and the architectural pivot history — see `DECISIONS.md` (D-022 is the passthrough decision; D-008..D-021 are the algorithm choices) and the analysis docs alongside the source.

## License

MIT. See [LICENSE](LICENSE).

## Acknowledgements

- Panasonic for the original WheelPad design, which this ports.
- The X.Org `xf86-input-synaptics` project for the angle-of-point-about-a-center reference implementation we compared against during reverse engineering.
- Peter Hutterer for the [2015 libinput discussion](https://gitlab.freedesktop.org/libinput/libinput/-/issues/) that explained why this had to be a daemon and not a libinput patch.
