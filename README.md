# Pen controller

A native (no webview) pen input controller: a transparent overlay that the pen summons and the mouse
dismisses, drawing pressure-sensitive ink with wgpu. Meant to become a dependency for other apps
(see `../pen-input-spec.md`). This is the skeleton the `pen-wgpu-spike` prototype grew into.

Platforms: Windows is the primary target. Linux works as a stand-in on sway (see "Run (Linux)"):
a transparent pressure-sensitive canvas, not yet a true overlay.

## Layout

| Crate | What it is | Tested here |
|---|---|---|
| `pen-proto` | Wire protocol: samples, presence, mode/canvas/brush messages, framing, a streaming decoder. No dependencies. | 12 unit tests |
| `pen-core` | `Lifecycle`: the show/hide rules as a pure state machine. `pen_win` (Windows only): raw-input pen presence, real-mouse detection, per-tablet `DeviceProfile`. `pointer` (Windows only): pen samples from `WM_POINTER` to a callback, state per window. `overlay` (Windows only): non-activating overlay window styles, show/hide. `pen_evdev` (Linux only): pen pressure, tip, range and barrel state read from the tablet's evdev node. | 13 unit tests (11 lifecycle, 2 pointer) |
| `pen-ink` | The ink, shared with the sketch studio: `brush.rs` turns samples into dabs (configurable through `BrushConfig`; the default is the original brush), `renderer.rs` has the `InkRenderer` trait and the wgpu renderer (no windowing crate; the controller's border is a `RendererOptions` setting). | 11 unit tests (brush) |
| `controller` | The app. `main.rs` wires things together, `input_win.rs` is thin glue from the winit window to `pen-core`'s `pointer` and `overlay`. | none |

The Windows and wgpu code (`pen_win`, `overlay`, the `WM_POINTER` handling in `pointer`, `input_win`, the wgpu renderer, `main`) and
the Linux evdev reader (`pen_evdev`) are not covered by the tests: they need a window, a GPU and a pen.

## Run (Windows)

```
cargo run -p controller --release
```

The overlay starts hidden. Bring the pen near the tablet to show it; move the mouse (with the pen away) to
hide it.

| Hotkey | Does |
|---|---|
| `Ctrl+Alt+D` | show / hide by hand (pinned: mouse movement will not hide it) |
| `Ctrl+Alt+A` | pen auto-show on / off |
| `Ctrl+Alt+K` | hands-off on / off |
| `Ctrl+Alt+Q` | quit |

**Hands-off** releases every hotkey except `Ctrl+Alt+K`, so those key combinations reach whatever program
you are using. The tray menu keeps working. The pen still summons the overlay. Auto-show and hands-off always
start at on / off and are never saved.

**Tray icon:** show/hide, pen auto-show, "Clear drawing when hidden", hands-off, Quit. The icon turns
blue-grey while hands-off is on.

**Clear drawing when hidden** (`CanvasOptions::clear_on_dismiss` in the protocol): the ink is wiped whenever
the overlay hides, for any reason. Off by default and not saved between launches yet; set
`PEN_CLEAR_ON_DISMISS=1` to start with it on. A client will be able to set it over the protocol.

Environment variables: `PEN_REDIRECT` (set it to keep the window's redirection bitmap),
`PEN_CLEAR_ON_DISMISS`. Read by the renderer in `pen-ink`: `PEN_BACKEND` (`dx12` | `vulkan`),
`PEN_DX12` (`visual` | `hwnd`), `PEN_PRESENT` (`mailbox` | `immediate` | `fifo` | `vsync`).

## Run (Linux, sway)

Tested on EndeavourOS with sway (Wayland), an Intel iGPU (Vulkan through Mesa) and a Wacom Intuos S
(CTL-4100). GNOME/Wayland is not a target: Mutter has no layer-shell.

```
cargo run -p controller --release
```

Needs a working Vulkan driver (`vulkaninfo --summary` should list your GPU). `alpha modes` in the startup
output must include `PreMultiplied` for the window to be see-through.

**1. Let your user read the tablet** (pressure comes from the tablet's evdev node). Prefer a udev rule that
covers only the tablet over adding yourself to the `input` group, which exposes every input device,
keyboards included. `056a:0374` is the CTL-4100; get yours from `sudo libinput list-devices`.

```
# /etc/udev/rules.d/70-pen.rules
SUBSYSTEM=="input", KERNEL=="event*", ATTRS{idVendor}=="056a", ATTRS{idProduct}=="0374", TAG+="uaccess"
```
```
sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=input
getfacl /dev/input/event8      # should list: user:<you>:rw-   (the event number can change)
```

The rule is re-applied when the tablet is replugged; if pressure stops working after a replug, run the
`udevadm trigger` line again. If you used the `input` group instead, undo it when you are done:
`sudo gpasswd -d $USER input`, then log out and in.

**2. Tell sway how to place the window.** A Wayland client cannot choose its own position or stay on top,
so sway does it. Do not use `fullscreen enable`: sway stops drawing everything underneath a fullscreen
window and the "transparent" canvas turns black.

```
# ~/.config/sway/config      (then: swaymsg reload)
for_window [title="pen controller"] floating enable, border none, sticky enable, resize set 1920 1080, move position 0 0
```

Use your output's pixel size for `W H` (`swaymsg -t get_outputs`).

**3. Config window (optional).** There is no tray or global hotkey on Wayland, so run with `--config`
(`cargo run -p controller -- --config`) to get a second window with the tray's switches as big buttons:
show/hide overlay, show when the pen is used, clear when hidden, hands-off, quit. Give it its own rule
(its title is `pen-ctl config`, so the overlay rule above does not match it):

```
for_window [app_id="pen-controller-config"] floating enable, resize set 460 520
```

Closing it leaves the overlay running; it can only be reopened by restarting with `--config`. While it has
focus the pen does not summon the overlay. `tray-icon` is built on Windows only.

**What works:** a transparent canvas with pressure-sensitive ink, positioned by sway's pointer events and
driven by the pen device (pressure, tip contact, barrel button in `Sample::buttons`).

**How it differs from Windows**

- The tray icon and global hotkeys are Windows-only for now (`tray-icon` needs GTK and a StatusNotifier
  host; `global-hotkey` only works through X11).
- When a pen device is found, the pen draws and the touchpad/mouse no longer does. With no readable pen
  device, the mouse fallback is used and the startup output says why.
- Summon and dismiss follow the pen's range, not the mouse: the pen coming into range shows the overlay, and
  the pen staying out of range for `PEN_DISMISS_MS` (default 1500) dismisses it. Wayland does not let a
  client see global mouse movement, so the Windows "a real mouse move dismisses" rule cannot be
  reproduced. A mouse nudge while the pen still hovers does not dismiss.
- A Wayland client cannot hide its own window, so "hidden" means click-through (`set_cursor_hittest(false)`)
  with no border. Ink stays on screen when hidden unless clear-on-dismiss is on, so use
  `PEN_CLEAR_ON_DISMISS=1` for now.
- The window does not follow the active workspace or monitor, and it is not above other windows by itself.
  `sticky enable` in the sway rule keeps it on every workspace of its output; it is not a real fix.
- No tilt, and no hover samples carrying their own positions.

Environment variables (Linux): `PEN_EVDEV` (a device path such as `/dev/input/event8`, to skip the
automatic search), `PEN_DISMISS_MS` (dismiss delay), `PEN_PINNED` (set it to show the overlay pinned and
never dismiss, the old always-on behaviour), plus the ones above that apply (`PEN_CLEAR_ON_DISMISS`,
`PEN_PRESENT`; `PEN_BACKEND`/`PEN_DX12` only matter on Windows).

## Tests

```
cargo test -p pen-proto -p pen-core -p pen-ink
```

These run on Linux and Windows. `controller` needs a recent Rust and a GPU to run.

If you have no Windows machine, you can still check that the Windows code compiles:

```
rustup target add x86_64-pc-windows-gnu
cargo check -p controller --target x86_64-pc-windows-gnu
```

This needs a mingw toolchain and only proves it builds, not that it runs. A CI job on `windows-latest`
running `cargo check` is the sturdier version of the same check.

## Protocol

Frames are `u32 length` | `u8 tag` | payload, little-endian. A sample is a fixed 40-byte body, so a client
in any language can read it as a plain struct. Capabilities a pen lacks are absent, not zero. Details and
the message list are in `pen-proto/src/lib.rs`. No transport is wired up yet.

## Not done yet

- A named-pipe server speaking `pen-proto` (a Unix socket on Linux), with Presence / Stream / Canvas modes
  selected by clients.
- Linux, proper: a layer-shell window with `tablet-v2` input, replacing winit on Linux. That gives a real
  overlay on every compositor with layer-shell (sway, Hyprland, KDE), follows the output it is placed on,
  adds tilt and hover, and removes the evdev permission setup.
- Linux: hide the ink (not just clear it) when dismissed, and a way to show/hide and quit without the
  tray and hotkeys.
- Windows build not re-checked after the Linux changes (the tray is stubbed out on non-Windows; the
  Windows path is meant to be unchanged).
- Taking keyboard focus for pinned overlays (`Lifecycle::can_take_focus` already decides it).
- A config file, so settings such as clear-on-dismiss persist.
- The airbrush and rectangle brushes from the Tauri overlay.
- Sub-pixel pen positions (`ptHimetricLocationRaw`) and tablet-normalised coordinates.
