# Pen controller

A native (no webview) pen input controller: a transparent overlay that the pen summons and the mouse
dismisses, drawing pressure-sensitive ink with wgpu. Meant to become a dependency for other apps
(see `../pen-input-spec.md`). This is the skeleton the `pen-wgpu-spike` prototype grew into.

## Layout

| Crate | What it is | Tested here |
|---|---|---|
| `pen-proto` | Wire protocol: samples, presence, mode/canvas/brush messages, framing, a streaming decoder. No dependencies. | 12 unit tests |
| `pen-core` | `Lifecycle`: the show/hide rules as a pure state machine. `pen_win` (Windows only): raw-input pen presence, real-mouse detection, per-tablet `DeviceProfile`. `pointer` (Windows only): pen samples from `WM_POINTER` to a callback, state per window. `overlay` (Windows only): non-activating overlay window styles, show/hide. | 13 unit tests (11 lifecycle, 2 pointer) |
| `pen-ink` | The ink, shared with the sketch studio: `brush.rs` turns samples into dabs (configurable through `BrushConfig`; the default is the original brush), `renderer.rs` has the `InkRenderer` trait and the wgpu renderer (no windowing crate; the controller's border is a `RendererOptions` setting). | 11 unit tests (brush) |
| `controller` | The app. `main.rs` wires things together, `input_win.rs` is thin glue from the winit window to `pen-core`'s `pointer` and `overlay`. | none |

The Windows and wgpu code (`pen_win`, `overlay`, the `WM_POINTER` handling in `pointer`, `input_win`, the wgpu renderer, `main`) is not covered by the tests: it
needs a window, a GPU and a pen.

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

## Tests

```
cargo test -p pen-proto -p pen-core -p pen-ink
```

(`controller` needs Windows and a recent Rust to build: `cargo build -p controller`.)

## Protocol

Frames are `u32 length` | `u8 tag` | payload, little-endian. A sample is a fixed 40-byte body, so a client
in any language can read it as a plain struct. Capabilities a pen lacks are absent, not zero. Details and
the message list are in `pen-proto/src/lib.rs`. No transport is wired up yet.

## Not done yet

- A named-pipe server speaking `pen-proto`, with Presence / Stream / Canvas modes selected by clients.
- Taking keyboard focus for pinned overlays (`Lifecycle::can_take_focus` already decides it).
- A config file, so settings such as clear-on-dismiss persist.
- The airbrush and rectangle brushes from the Tauri overlay.
- Sub-pixel pen positions (`ptHimetricLocationRaw`) and tablet-normalised coordinates.
- Linux (evdev for presence; X11 / layer-shell for the window).
