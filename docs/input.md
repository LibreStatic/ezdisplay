# Input injection

The tablet sends pen, touch, button and scroll events; the host injects them into the desktop.
`ezd-host run` does this by default; `--no-input` turns it off. Without a working backend the
stream still runs and the tablet is told why (`InputStatus.note`).

## Host flags

- `--no-input`: do not inject anything.
- `--telemetry`: ask the tablet to show its telemetry overlay. The host logs the input latency
  (p50 / p95 from the tablet event to the injection) every 5 s either way.
- `--pen-mode tablet|mouse`: inject the pen as a pen (default) or as an absolute mouse, for apps
  without pen support (see each platform below).

## Linux (KDE Plasma 6)

### What the host creates

Three uinput devices per session, all with vendor `0x1209` and fixed names, so KWin can remember
their settings:

| Device | Product | Kind | Used for |
| --- | --- | --- | --- |
| `ezdisplay touch` | `0xEDD1` | multi-touch screen (type B, up to 10 contacts, `INPUT_PROP_DIRECT`) | finger touch |
| `ezdisplay pen` | `0xEDD2` | graphics tablet (pressure, tilt, eraser, barrel button) | stylus |
| `ezdisplay pointer` | `0xEDD3` | absolute pointer with buttons, wheel and Ctrl / Shift | mouse-style buttons, scrolling, pinch (Ctrl + wheel) |

The ranges are the protocol's 0..65535, with the resolution derived from the tablet's pixel size
and dpi. The devices disappear when the session ends.

### Permissions

`/dev/uinput` is `root:root 0600` by default. Install the udev rule once:

```sh
sudo install -m644 packaging/linux/70-ezdisplay-uinput.rules /etc/udev/rules.d/
sudo udevadm control --reload && sudo udevadm trigger --name-match=uinput
```

The rule gives the user at the seat access (the `uaccess` tag). Its file name sorts before
`73-seat-late.rules`, which is required. `ezd-host diag` reports `input.uinput` as `ok`,
`permission denied` or `missing` (load the module with `sudo modprobe uinput`).

### Mapping to the virtual output

KWin ignores touch and tablet devices that have no output. After creating the devices, the host
sets `outputName` of the touch and pen devices to the virtual output over D-Bus
(`org.kde.KWin.InputDevice` at `/org/kde/KWin/InputDevice/eventN`, session bus) and reads it back.
Touch and pen are only enabled once this worked; otherwise `InputStatus` omits them and says why.
KWin stores the mapping in `~/.config/kcminputrc` under
`[Libinput][4617][60881][ezdisplay touch]` (decimal vendor / product) and similar; the host sets it
again every session because the virtual output is new each time. These groups can be deleted safely.

The pointer is different: KWin scales an absolute pointer over the whole workspace (the bounding
box of the enabled outputs, in logical coordinates). The host reads the layout with
`kscreen-doctor -j`, refreshes it every 3 s and converts the stream position to that space,
including the output's scale. If the workspace does not start at (0, 0) (an output to the left of
or above the primary one) the pointer is off by that offset; the host logs a warning once.

### Pen button and `--pen-mode`

- Qt and Xwayland map `BTN_STYLUS` to middle click and `BTN_STYLUS2` to right click. The barrel
  button sends `BTN_STYLUS2` by default (barrel + tip = right click). Set `EZD_PEN_BUTTON=stylus`
  for `BTN_STYLUS`.
- Only Wayland clients with `zwp_tablet_v2` support (Qt, GTK, Firefox, Xwayland) see the pen, and
  a tablet does not move the system pointer. For apps without tablet support use
  `ezd-host run --pen-mode mouse`: the pen drives the pointer device instead (move, tip = left
  button, barrel = right button, no pressure or tilt).

### Troubleshooting

- `ezd-host diag`: the `input` section shows `uinput` and `kwin_input_dbus`.
- `busctl --user tree org.kde.KWin` lists `/org/kde/KWin/InputDevice/eventN` for each device;
  `busctl --user introspect org.kde.KWin /org/kde/KWin/InputDevice/eventN` shows `outputName`.
- `libinput list-devices` shows whether the touch device is a touchscreen and the pen a tablet.
- `wev` (or `libinput debug-events`) shows the events a client receives, including which button the
  barrel produces.
- "cannot open /dev/uinput (permission denied)": install the udev rule and re-login or re-trigger.
- Touch works but lands on the wrong screen: check `outputName` of the device; the note in the
  tablet's status says if the mapping failed.

## Windows

The Windows host injects the pen and touch through synthetic pointer devices (`CreateSyntheticPointerDevice`, Windows 10 1809 or later): a PT_PEN device with pressure, tilt, barrel button and eraser, and a PT_TOUCH device with up to 10 contacts. Pointer buttons and scrolling go through `SendInput`, as absolute mouse moves over the virtual desktop followed by the click or wheel. Ctrl and Shift are pressed around the wheel event, so a pinch (Ctrl + wheel) zooms.

- **Coordinates.** Normalized coordinates are mapped onto the pixel rectangle of the virtual display, read from its GDI name (`dmPosition` plus current size). The rectangle is re-read when the pen enters range and at most once per second otherwise. The process is per-monitor-v2 DPI aware, so these are physical pixels.
- **Pen.** Hover is INRANGE|UPDATE, DOWN and UP are injected as separate frames, and leaving range is an UPDATE without INRANGE. Pressure is scaled to 0..1024 and tilt to -90..90 degrees. The barrel button sets `PEN_FLAG_BARREL`. The eraser end sets `PEN_FLAG_INVERTED` while in range, plus `PEN_FLAG_ERASER` while in contact. Changing between pen and eraser leaves range and enters again.
- **Touch.** Every injection lists all active contacts. Stationary contacts are re-injected as UPDATE after 100 ms without any touch injection, and a contact missing from a frame is lifted.
- **Mouse pen mode** (`--pen-mode mouse`). No pen device is created. The pen moves the mouse, contact is the left button, the barrel button is the right button, and leaving range releases them.
- **Limits.** Windows blocks injected input to windows of an elevated (administrator) process (UIPI) unless the host also runs elevated. The host logs a rate-limited warning when `SendInput` injects fewer events than requested.
- **Status.** `InputStatus` carries TOUCH and PEN only when their device was created. If a device cannot be created (Windows older than 1809), the note says so. Without a known virtual display, input is unavailable. `ezd-host diag` has an `input` section that reports whether both devices can be created.
