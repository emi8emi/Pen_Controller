# Pen Input Controller + Written-to-Voice: Spec (draft v0.1)

Two new projects plus one shared crate. The existing pen overlay (Rust + Tauri) stays completely separate for now.

| Piece | What it is |
|---|---|
| **pen-core** | Shared Rust crate: pen/mouse detection, summon/dismiss rules, hotkeys, hands-off, ignore list. Extracted from the current `pen.rs` / `main.rs` logic. |
| **Project A: controller** | Native (no webview) pen input controller, delivered as a dependency for other apps. Renders an input canvas itself, or only reports pen activity/data. |
| **Project B: written-to-voice** | First client of A. Handwriting (and later custom symbols/shorthand/gestures) to speech, for communication and practice. |

Eventually the existing overlay becomes another client of A. Not now.

---

## 0. Decisions log

**Decided**
- A is native, no webview, written in Rust with wgpu, chosen partly out of interest (no latency measurement rig is available). The webview overlay remains the fallback, so the renderer sits behind a trait.
- A renders the input canvas itself, with settings passed in by the consumer (size, position, pen settings, brush settings).
- Consumers may be Rust, C#, or other languages, so A ships as a Rust core with a C ABI plus a thin daemon built on the same core.
- B is the first client of A.
- Linux support is best effort: X11 plus wlroots/KDE compositors. GNOME/Wayland is out of scope.
- B v1 recognition uses Windows ink recognition. TTS v1 is Windows TTS; Piper later.
- The most important post-v1 feature for B is custom recognition (symbols, dictionary, gestures).
- Speak timing is configurable (all the options in section 4.5).
- "Steno" means pen shorthand, not chorded machine steno.

**Open** (see section 5): latency target, default delivery mode, gesture-vs-ink rule, what "practice" means, confirm-before-speak default.

---

## 1. Shared core: `pen-core`

Extract and generalise what already exists:

- Raw Input digitizer detection (usage page 0x0D) for global "pen in range / touching" presence. Today this is hardcoded to the CTL-4100 report layout (ID `0xD5`, flags in byte 1). The new crate must make the report layout configurable per device profile, with the CTL-4100 as the first profile.
- Low-level mouse hook with the Windows Ink signature check, so pen-driven cursor movement never counts as a mouse.
- The 300 ms handoff rule: a real mouse move this long after the last pen report dismisses.
- State flags: auto-show, hands-off, pinned, visible. None are persisted; every launch starts with auto-show on and hands-off off.
- Foreground-app ignore list.
- Hotkey registration/release, including the "toggle hands-off stays registered" rule.

The core exposes events (`PenInRange`, `PenOut`, `Summon`, `Dismiss`, flag changes) and takes no dependency on any UI toolkit.

Platform abstraction: a `PenPresence` trait (global in-range detection) and an `OverlayWindow` trait. Windows implementations first.

---

## 2. Project A: controller

### 2.1 Goals
- Lowest practical pen-to-pixel latency for ink.
- Be a dependency: other programs configure it, receive its events, and never reimplement pen lifecycle logic.
- Pen-only usable: large targets, no keyboard-only features, works in hands-off mode.

### 2.2 Non-goals
- No recognition of any kind (writing, symbols, gestures). That belongs to clients.
- No layers/colour-triangle/docks. That's the existing overlay's job.
- No custom brush engines in v1.

### 2.3 Modes

| Mode | Controller does | Window | Typical consumer |
|---|---|---|---|
| **Presence** | Emits active/inactive and lifecycle events only | None | Apps/games with their own window and their own input path |
| **Stream** | Captures pen data and forwards strokes; draws nothing | Transparent capture window | Consumers that render themselves but have no window of their own |
| **Canvas** | Captures, draws the ink itself, and forwards strokes | Transparent window with built-in renderer | Project B, quick tools |

Notes:
- Pen data (`WM_POINTER`) only reaches the window under the pen. A capture window, even an invisible one, means the pen cannot click through to what is underneath until dismissed, same as the current overlay.
- Overlays do not appear over exclusive-fullscreen games. Borderless windowed is a requirement for game consumers.

### 2.4 Lifecycle
Carry over the existing contract from the current README:
- Shows when the pen comes into range or touches, unless an ignored app has focus.
- Hides when a real mouse moves 300 ms or more after the pen's last report.
- Hotkey summon is pinned: mouse movement does not hide it.
- Pen summon is non-activating and never takes keyboard focus.
- Hands-off releases every hotkey except the hands-off toggle and the overlay can never take focus.
- Hotkeys are configurable (see 2.9) rather than hardcoded.

### 2.5 Input sources
Two sources by design:

1. **Presence (global):** raw HID digitizer reports on Windows, per-device profile. On Linux: `evdev`/libinput tablet devices (needs `input` group permission, e.g. a udev rule).
2. **Strokes (inside the capture window):** `WM_POINTER` with pointer history (coalesced samples, hardware timestamps) on Windows. On Linux: XInput2 (X11) or `tablet-v2` (Wayland compositors that provide it).

Do not rely on winit for pen data. It does not expose tilt, hover, or full coalesced history. Handle pointer messages directly in the window procedure and keep the windowing layer minimal.

### 2.6 Event schema
Per sample (names illustrative):

```json
{
  "pen_id": 0,
  "phase": "hover|down|move|up",
  "x": 812.4, "y": 301.9,
  "tx": 0.4121, "ty": 0.3305,
  "pressure": 0.62,
  "tilt_x": 0.0, "tilt_y": 0.0,
  "buttons": 0,
  "eraser": false,
  "t_us": 118234501
}
```

- `x`,`y`: overlay pixel coordinates. `tx`,`ty`: normalised tablet coordinates. Both, so recognizers can be resolution-independent.
- `t_us`: hardware timestamp where available, otherwise receive time (flagged).
- Absent capabilities (e.g. tilt on the CTL-4100) are reported as unavailable, not as zero.
- A stroke is down...up. Strokes carry an id and are batched.

### 2.7 Canvas and settings (consumer-supplied)
The consumer passes:
- Canvas rect: size and position (re-sendable at runtime).
- Pen settings: pressure curve, minimum pressure, smoothing, palm/touch rejection.
- Brush settings (built-in brushes only in v1): shape, size, colour, opacity, size-from-pressure.
- Behaviour: clear-on-dismiss, auto-clear timeout, whether ink persists between strokes.

The consumer receives:
- Stroke events (2.6), always.
- Optionally a bitmap snapshot of the canvas (some recognizers want pixels).

Deferred: consumer-supplied custom painting. In-process it could be a callback; across processes it requires a shared GPU texture. Revisit after v1.

### 2.8 Delivery

**Rust core library with a C ABI** (`cdylib`/`staticlib`), plus a **daemon** binary that links the same core.

- **In-process mode (library):** lowest latency, single-file distribution is possible for .NET Native AOT consumers by statically linking the Rust `staticlib`. For regular .NET, ship the DLL beside the app (single-file publish extracts it at runtime). For plugin/engine hosts (Unity/Mono and similar), ship the DLL beside the plugin or use daemon mode.
- **Daemon mode:** one process owns the overlay and pen hooks; clients connect over a named pipe (and optionally a local WebSocket bridge for browser clients). This prevents two embedding apps from creating competing overlays and competing raw-input registrations.
- C ABI rules: stable, versioned ABI; events delivered by callback on a controller thread (consumer marshals to its own thread); no panics across FFI; explicit ownership rules for strings and buffers; generated headers (cbindgen) and C# bindings (csbindgen).

Default mode is an open question (section 5).

### 2.9 Configuration
TOML, hot-reloaded, per-consumer profile.

```toml
[profile.default]
mode = "canvas"            # presence | stream | canvas
auto_show = true           # not persisted across launches by design
ignore_apps = ["krita.exe", "CLIPStudioPaint.exe", "Photoshop.exe"]
occlusion_gap_px = 1

[profile.default.hotkeys]
toggle = "Ctrl+Alt+D"
hands_off = "Ctrl+Alt+K"
auto_show = "Ctrl+Alt+A"

[profile.default.canvas]
x = 0
y = 0
width = 1920
height = 1080

[profile.default.pen]
min_pressure = 0.02
smoothing = 0.5

[profile.default.brush]
shape = "circle"
size = 6
opacity = 1.0
size_from_pressure = true
```

### 2.10 Privacy and security
A pen stream is a handwriting stream, which can include passwords.
- Stream only while the overlay is active **and** a client has subscribed.
- Daemon: per-client token or pipe ACL; no anonymous local access.
- No logging of stroke data by default. The recorder (2.12) is opt-in and local.

### 2.11 Platform support
- Windows: primary.
- Linux: best effort, X11 plus wlroots/KDE compositors (layer-shell). GNOME/Wayland is not supported.
- macOS: not planned; the traits leave room for it.

Linux specifics: tablet-to-screen coordinate mapping must be handled when presence comes from raw evdev (it carries tablet coordinates, not screen coordinates). Strokes inside the overlay come from XInput2 or `tablet-v2`, which are already screen-mapped.

### 2.12 Stroke recorder (v1 requirement)
Opt-in, local recorder that exports strokes as JSON (timestamped points with pressure) in the same schema as 2.6. Purpose: accumulate real samples of handwriting, shorthand outlines, symbols and gestures from day one, so custom recognition (B, post-v1) has data.

### 2.13 UI
Minimal, pen-first: large buttons, a tray menu, a mode indicator. No keyboard-only features. Built on a small native toolkit or a few hand-drawn controls; choice made after the spike.

### 2.14 Latency
No camera-based measurement is available, so no latency promise is made. Compare against the existing overlay by feel (the gap between pen tip and the front of the ink on fast strokes) and with software timestamps (pen event received to present() call). Neither includes the display.

### 2.15 Milestones
0. **wgpu prototype** (`pen-wgpu-spike/`): transparent always-on-top window with pen ink via `WM_POINTER` history. The renderer sits behind a trait so the webview overlay stays a fallback. Per-pixel transparency on DX12 needs a wgpu version with the DirectComposition presentation option (`DxgiFromVisual`), and the alpha mode must be set before the first surface configure. Compare against the existing overlay by feel and by software timestamps. Direct2D/DirectComposition is plan B for the renderer.
1. Extract `pen-core` and keep the current overlay working against it (or just compile-share it).
2. Presence mode + C ABI + daemon pipe.
3. Stream mode with `WM_POINTER` history.
4. Canvas mode with built-in brushes.
5. Config and hot-reload, recorder.
6. Linux presence (evdev) and X11 capture window.

---

## 3. Shared contracts between A and clients

- **Stroke schema:** the one in 2.6 is the stable contract. Version it.
- **Coordinate spaces:** overlay pixels and normalised tablet coordinates, always both.
- **Capability flags:** clients query what the connected pen supports (tilt, hover, eraser, timestamps).

---

## 4. Project B: written-to-voice

### 4.1 Goals
- Write by hand, hear it spoken. For communication and for practice.
- Pluggable recognition and pluggable TTS, so v1 can use built-ins and later versions can use custom pieces without a rewrite.

### 4.2 Pipeline
strokes (from A, Canvas mode) → segmentation (word/line) → recognition → candidates → speak policy → TTS.

### 4.3 Recognizer interface
Stable from v1:

```text
recognize(strokes, context) -> Vec<Candidate { text, score: Option<f32> }>
```

- `score` is optional: Windows ink returns ranked candidates without confidence.
- Multiple recognizers may run; a policy picks the winner (e.g. custom match above a threshold, else Windows ink).

### 4.4 Recognition plan

**v1: Windows ink recognition** (WinRT `InkRecognizerContainer`, strokes built with `InkStrokeBuilder`):
- Pick recognizer by language; which ones exist depends on the installed handwriting packs.
- Recognise per word, per line, or whole input.
- Ranked candidates per word. No custom vocabulary, no training, no confidence.

**Post-v1 (top priority): custom recognition**
- Symbols, personal dictionary, gestures, and pen shorthand outlines.
- Approach to start with: template matching (the $P/$Q family) over samples you record yourself with the recorder. No ML required for a small vocabulary.
- Data: shorthand has no off-the-shelf recognizer, so the personal template set is the product.
- The matcher plugs in through the interface in 4.3.

### 4.5 Speak policy (configurable)

```toml
[speak]
trigger = "pause"          # pause | line_end | gesture | button
pause_ms = 900
confirm = "preview"        # off | preview
confirm_ms = 1200
on_new_input = "queue"     # queue | interrupt
```

- **Trigger:** pause threshold, end of line, an explicit gesture, or a button.
- **Confirm:** speak immediately, or show the top candidate for a short window (with the alternates) before speaking.
- **Interruption:** queue new utterances or cut off the current one.

A misrecognised word spoken aloud cannot be taken back, so the confirm default matters for communication use. See section 5.

### 4.6 Gestures
Gestures (e.g. an underline meaning "speak", a strike meaning "delete") must be distinguishable from ink by an explicit rule. Options: a gesture zone, a pen button, a gesture mode, or a stroke-shape classifier. Decide one in the spec, since "random" gesture detection would eat real writing.

### 4.7 TTS interface
- v1: Windows TTS (WinRT SpeechSynthesizer or SAPI). Voices available depend on installed language packs.
- Later: Piper (local neural voices), selectable per language.
- Behind an interface from day one: `speak(text, voice, rate)`, `stop()`.

### 4.8 Practice use
Purpose is open (section 5). Likely shapes: hear back what was recognised to check legibility; later, per-character or per-outline feedback. Do not design scoring until the purpose is pinned.

### 4.9 Milestones
1. Canvas client of A; strokes in.
2. Windows ink recognition, word-level, one language.
3. Windows TTS and the speak policy (all four triggers).
4. Confirm/preview UI, pen-only.
5. Recorder-fed template set for custom recognition; gestures.
6. Piper.

---

## 5. Open questions and risks

1. **Default delivery mode** for A: in-process library or daemon. Recommendation: daemon as default, library for single-consumer or AOT scenarios.
2. **Latency:** the native-vs-webview benefit is unmeasured and may be negligible. The native route is justified by interest and control, not by numbers.
3. **Gesture-vs-ink rule:** which of the options in 4.6.
4. **What "practice" means** for B: hearing recognised text only, or also correction/scoring (e.g. of shorthand outlines or characters).
5. **Confirm-before-speak default:** on for communication safety, off for speed. Possibly per-profile.
6. **Renderer:** wgpu (DX12 with `DxgiFromVisual` for transparency) vs Direct2D/DirectComposition, decided by whether the prototype gets reliable per-pixel transparency.
7. **Linux GNOME/Wayland** is out of scope; revisit if layer-shell support appears.
8. **Device profiles:** report layouts for tablets other than the CTL-4100 (or moving presence to a source that does not need per-device bytes).
9. **Windows ink recognition** is an older API: it works, but expect no improvements. Keep it replaceable.

---

## 6. Explicit non-goals (whole effort)
- Recognition inside A.
- A layers/colour-picker UI inside A.
- GNOME/Wayland support.
- Cloud speech or cloud recognition in v1.
- Merging with the existing overlay now.

---

## 7. Implementation status

Workspace: `pen-controller/` (see its README).

- Done: `pen-proto` (messages, framing, decoder, tests); `pen-core` (`Lifecycle` state machine with tests, `pen_win` with a per-tablet `DeviceProfile`); `controller` (wgpu renderer behind an `InkRenderer` trait, brush engine with tests, `WM_POINTER` input, hidden-until-summoned non-activating window with a border, hotkeys).
- Checked on a real machine: DirectComposition (`DxgiFromVisual`) transparency on DX12, pen ink with pressure, summon/dismiss, no Alt+Tab entry. Mailbox is the default present mode.
- Next: named-pipe server speaking `pen-proto` (Presence, Stream, Canvas), then config (TOML), hands-off and tray, then the recorder.
