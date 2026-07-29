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
minimum_rotation_radius = 250.0 # reject tighter circles; 0 disables the limit
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
| `scroll.minimum_rotation_radius` | `250.0` | ≥ 0 | Minimum local circle radius in corrected X-axis units. Smaller circular motions are ignored; `0` disables the limit. |
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

If scrolling feels too fast or too slow, adjust `scroll.sensitivity` in the config (-2..+2). The daemon does not auto-calibrate — detector history is fixed at 20 samples to match Windows exactly.

## Known issues / non-goals

- **`WheelUnderCursor` is not configurable.** On Wayland the compositor routes input to the focused surface; there's no userland override.
- **Vertical circular scrolling is tested on Synaptics TM3562-3 and CF-SZ6 SYN0502.** Other touchpads may work with `device_name_regex` and coordinate calibration, but no compatibility promises.
- **Hands-on validation of the input-proxy recovery changes is still pending.** Idle startup and SIGTERM shutdown have been exercised with a physical touchpad and uinput, but startup interaction, actual touch lifecycles, `SYN_DROPPED` recovery, and button-held scrolling still require an end-to-end test through libinput.
- **Excel arrow-key fallback is gone.** Modern Excel routes horizontal wheel events natively; we don't need the Windows hack.
- **No coasting/kinetic scrolling.** Matches the Windows WheelPad behaviour; xf86 has it but we don't.

## Input proxy safety and recovery

Only physical Type B multitouch devices advertising `ABS_MT_SLOT`, `ABS_MT_TRACKING_ID`, `ABS_MT_POSITION_X/Y`, and `BTN_TOUCH` are accepted. The uinput-compatible slot range is `0..99` (at most 100 slots); larger physical ranges are rejected. Discovery filters unsupported candidates before deciding whether a device name is ambiguous. All `BUS_VIRTUAL` devices are rejected, as are devices carrying letsnote-wheelpad's own name or input IDs, to prevent the daemon from recursively selecting its uinput output.

The physical evdev FD is changed to nonblocking immediately after open while preserving its existing file-status flags. At startup, the virtual devices are created while the physical device remains ungrabbed. The daemon drains only events already available in the ungrabbed queue, stopping normally at `EAGAIN`, and then queries current state. If any MT tracking ID, `BTN_TOUCH`, or physical touchpad button is active, it waits so existing consumers can finish that lifecycle.

After observing quiescence, the daemon grabs the device and immediately re-queries kernel state without reading or discarding any post-grab event. If the recheck is quiescent, post-grab events remain queued for normal proxy processing. Before systemd `READY=1`, the daemon selects the confirmed physical current MT slot on the virtual touchpad and initializes gesture and Router state from the same snapshot. It does not attempt to proxy a lifecycle already active before the permanent grab.

This recheck reduces, but does not eliminate, the ownership-transition race. A touch or button press can begin after `EVIOCGRAB` becomes effective but before the state query. If the recheck observes that input, the daemon releases the grab and waits for quiescence without consuming the queued event; however, the kernel does not replay the exclusively delivered touch-down or press to existing evdev clients. That in-progress contact may therefore be ignored or incomplete until the user fully lifts. The daemon must then accept the next complete lifecycle normally. Eliminating this gap requires an architecture in which libinput permanently ignores the physical device and consumes only the virtual proxy.

The evdev stream is read in raw mode so `SYN_DROPPED` remains visible. After a drop, the daemon refreshes every slot's tracking ID and position. The reconstruction selects each affected or active slot, emits required tracking end/start events, emits refreshed X/Y for every active identity (including unchanged identities), restores the refreshed physical current slot, and then emits `SYN_REPORT`. Routing may suppress the captured contact's reconstructed X/Y while Scrolling, but reconstructed positions for non-captured contacts remain forwarded. Full reconstruction of every auxiliary key/ABS property after `SYN_DROPPED` is not implemented.

The previous unconditional five-second scrolling watchdog has been removed. While scrolling, the poll loop wakes once per second to query physical tracking IDs. A stationary session remains active while the captured `(slot, tracking_id)` still exists. If that identity has disappeared, reconciliation supplies its lift to the normal FSM and ends the session. A reconciliation I/O failure is fatal, causing the grab to be released and the daemon to exit non-zero.

SIGTERM and SIGINT are blocked and received through `signalfd`, which is polled with the evdev FD and therefore cannot be lost between a stop-flag check and a blocking poll. Fatal poll, evdev, reconciliation, or uinput errors terminate the daemon; the `EVIOCGRAB` owner is released by an RAII guard on normal shutdown and error paths. Duplicate instances are prevented per device within the same UID and XDG runtime directory.

## How it works (one-paragraph version)

The daemon takes exclusive ownership of the physical touchpad while it is running and creates a virtual touchpad mirror plus a virtual wheel. Outside a WheelPad session, events are forwarded in order. Once the existing FSM and circular detector engage, routing suppresses only the captured contact's `ABS_MT_POSITION_X/Y` events and the primary `ABS_X/Y` cursor mirror. Buttons, slot/tracking lifecycle, auxiliary touch data, MSC events, and non-captured contacts remain forwarded. The detector's mathematics, engagement threshold, constants, fixed 20-sample history, sensitivity table, and pause/resume interaction are unchanged. Each existing ±π accumulator crossing emits one wheel notch; lifting the captured tracking ID ends the session while preserving the contact lifecycle on the virtual touchpad.

For the full algorithm details and architectural history, see the source and its synthetic regression tests.

## License

MIT. See [LICENSE](LICENSE).

## Acknowledgements

- Panasonic for the original WheelPad design, which this ports.
- The X.Org `xf86-input-synaptics` project for the angle-of-point-about-a-center reference implementation we compared against during reverse engineering.
- Peter Hutterer for the [2015 libinput discussion](https://gitlab.freedesktop.org/libinput/libinput/-/issues/) that explained why this had to be a daemon and not a libinput patch.
