//! Pen controller (skeleton). One transparent, always-on-top overlay over the primary monitor that the
//! pen summons and the mouse dismisses, drawing pressure-sensitive ink.
//!
//! Layout:
//!   pen-proto   the wire protocol (messages, framing). Not used on a transport yet; samples already use its type.
//!   pen-core    the show/hide rules (`Lifecycle`) and Windows pen/mouse detection (`pen_win`)
//!   pen-ink     the ink: `brush.rs` (pen samples -> dabs) and `renderer.rs` (`InkRenderer` trait + the wgpu implementation)
//!   controller  this crate:
//!     main.rs       app, event loop, hotkeys: wires the pieces together
//!     input_win.rs  WM_POINTER input and window styling (Windows only)
//!
//! Lifecycle (decided in `pen_core::Lifecycle`, executed here): the window starts hidden. The pen shows it
//! without taking focus, unless an ignored app has focus; a real mouse move 300 ms or more after the pen's
//! last report hides it.
//!   Ctrl+Alt+D  show / hide by hand (pinned: mouse movement will not hide it)
//!   Ctrl+Alt+A  pen auto-show on / off
//!   Ctrl+Alt+K  hands-off on / off: every other hotkey is released so those key combinations reach
//!               whatever program you are using. Only this one stays registered, so you can get back out.
//!   Ctrl+Alt+Q  quit
//! The tray icon has the same switches (it keeps working in hands-off mode) plus "Clear drawing when
//! hidden" (`CanvasOptions::clear_on_dismiss`), and a Quit item. Auto-show and hands-off always start
//! at on / off and are never saved.
//! The window never takes keyboard focus yet (`Lifecycle::can_take_focus` is ready for when it should).
//! The mouse also draws (pressure 0.5): pin the overlay first, or moving the mouse hides it.
//!
//! Env:  PEN_BACKEND = dx12 (default on Windows) | vulkan
//!       PEN_DX12 = visual (default, DirectComposition, needed for transparency) | hwnd
//!       PEN_PRESENT = (default: mailbox if available) | vsync | fifo | mailbox | immediate
//!       PEN_REDIRECT = set it to keep the window's redirection bitmap (to compare)
//!       PEN_CLEAR_ON_DISMISS = set it to start with "clear drawing when hidden" on (not saved otherwise)

#[cfg(windows)]
mod input_win;

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use pen_core::{Action, Lifecycle};
use pen_proto::{CanvasOptions, Phase, Sample};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId, WindowLevel};

use pen_ink::{Border, InkRenderer, RendererOptions, Stroker, WgpuRenderer};

/// Leave a strip this many pixels tall at the top of the screen uncovered, so browsers and video players
/// behind the overlay do not think they are hidden and stop painting.
const OCCLUSION_GAP_PX: u32 = 1;

/// The pen will not summon the overlay while one of these programs has focus (case-insensitive).
const IGNORE_APPS: &[&str] = &["krita.exe", "CLIPStudioPaint.exe", "Photoshop.exe"];

/// A sample plus when our window procedure saw it (for the timing printout).
pub(crate) struct Incoming {
    pub sample: Sample,
    pub recv: Instant,
}

// Everything that reaches the main loop from other threads (pen thread, hotkeys, window procedure).
static QUEUE: Mutex<Vec<Incoming>> = Mutex::new(Vec::new());
static PROXY: Mutex<Option<EventLoopProxy<UserEvent>>> = Mutex::new(None);
static LIFECYCLE: OnceLock<Mutex<Lifecycle>> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();

/// Something the user asked for, from a hotkey or the tray menu. Handled on the main loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    Toggle,
    ToggleAuto,
    ToggleHandsOff,
    ToggleClearOnDismiss,
    Quit,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum UserEvent {
    /// New pen samples are waiting in `QUEUE`.
    Samples,
    /// The lifecycle decided to show or hide the overlay (from the pen thread).
    Overlay(Action),
    Command(Command),
}

pub(crate) fn send(ev: UserEvent) {
    if let Some(p) = PROXY.lock().unwrap().as_ref() {
        let _ = p.send_event(ev);
    }
}

fn lifecycle<R>(f: impl FnOnce(&mut Lifecycle) -> R) -> R {
    let m = LIFECYCLE.get().expect("lifecycle is created in main");
    let mut guard = m.lock().unwrap();
    f(&mut guard)
}

/// Forward a lifecycle decision to the main loop.
fn apply(action: Option<Action>) {
    if let Some(a) = action {
        send(UserEvent::Overlay(a));
    }
}

fn now_us() -> u64 {
    START.get().map_or(0, |s| s.elapsed().as_micros() as u64)
}

#[derive(Default)]
struct Stats {
    n: u64,
    sum: u64,
    max: u64,
}

impl Stats {
    fn add(&mut self, us: u64) {
        self.n += 1;
        self.sum += us;
        self.max = self.max.max(us);
        if self.n == 200 {
            eprintln!("event -> present() call: avg {} us, max {} us (software side only)", self.sum / self.n, self.max);
            *self = Stats::default();
        }
    }
}

/// Cover the primary monitor, minus the strip at the top described at `OCCLUSION_GAP_PX`.
fn fit_to_monitor(window: &Window) {
    if let Some(mon) = window.primary_monitor().or_else(|| window.current_monitor()) {
        let pos = mon.position();
        let size = mon.size();
        let gap = OCCLUSION_GAP_PX.min(size.height.saturating_sub(1));
        window.set_outer_position(PhysicalPosition::new(pos.x, pos.y + gap as i32));
        let _ = window.request_inner_size(PhysicalSize::new(size.width, size.height - gap));
    }
}

/// The tray icon: pink normally, blue-grey while hands-off is on.
fn make_icon(hands_off: bool) -> Icon {
    const N: u32 = 32;
    let fill: [u8; 3] = if hands_off { [120, 128, 160] } else { [255, 72, 176] };
    let ring: [u8; 3] = [26, 31, 58];
    let c = (N as f32 - 1.0) / 2.0;
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let alpha = ((15.5 - d).clamp(0.0, 1.0) * 255.0) as u8;
            let rgb = if d <= 12.0 { fill } else { ring };
            rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], alpha]);
        }
    }
    Icon::from_rgba(rgba, N, N).expect("a valid icon")
}

/// The tray icon and its menu. Check items flip themselves when clicked, so after every command the
/// app sets them again from the real state (`sync`), which also covers changes made by hotkeys.
struct Tray {
    icon: TrayIcon,
    auto: CheckMenuItem,
    clear: CheckMenuItem,
    hands: CheckMenuItem,
}

impl Tray {
    fn new(options: CanvasOptions) -> Result<Tray, Box<dyn std::error::Error>> {
        let toggle = MenuItem::new("Show / hide overlay  (Ctrl+Alt+D)", true, None);
        let auto = CheckMenuItem::new("Show when the pen is used  (Ctrl+Alt+A)", true, true, None);
        let clear = CheckMenuItem::new("Clear drawing when hidden", true, options.clear_on_dismiss, None);
        let hands = CheckMenuItem::new("Hands-off: ignore hotkeys  (Ctrl+Alt+K)", true, false, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append_items(&[
            &toggle,
            &auto,
            &clear,
            &PredefinedMenuItem::separator(),
            &hands,
            &PredefinedMenuItem::separator(),
            &quit,
        ])?;

        // menu clicks arrive on another thread: translate them into commands for the main loop
        let ids = [toggle.id().clone(), auto.id().clone(), clear.id().clone(), hands.id().clone(), quit.id().clone()];
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let cmd = if e.id == ids[0] {
                Command::Toggle
            } else if e.id == ids[1] {
                Command::ToggleAuto
            } else if e.id == ids[2] {
                Command::ToggleClearOnDismiss
            } else if e.id == ids[3] {
                Command::ToggleHandsOff
            } else if e.id == ids[4] {
                Command::Quit
            } else {
                return;
            };
            send(UserEvent::Command(cmd));
        }));

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Pen controller")
            .with_icon(make_icon(false))
            .build()?;
        Ok(Tray { icon, auto, clear, hands })
    }

    fn sync(&self, auto_show: bool, hands_off: bool, clear_on_dismiss: bool) {
        self.auto.set_checked(auto_show);
        self.clear.set_checked(clear_on_dismiss);
        self.hands.set_checked(hands_off);
        let _ = self.icon.set_icon(Some(make_icon(hands_off)));
        let _ = self.icon.set_tooltip(Some(if hands_off { "Pen controller (hands-off)" } else { "Pen controller" }));
    }
}

/// The global hotkeys. Hands-off releases all of them except `hands`.
struct Keys {
    toggle: HotKey,
    auto: HotKey,
    quit: HotKey,
    hands: HotKey,
}

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Box<dyn InkRenderer>>,
    stroker: Stroker,
    stats: Stats,
    cursor: (f32, f32),
    mouse_down: bool,
    /// Canvas behaviour. A client will be able to set this through the protocol; for now the tray does.
    options: CanvasOptions,
    tray: Option<Tray>,
    hotkeys: GlobalHotKeyManager,
    keys: Keys,
}

impl App {
    fn handle_batch(&mut self, batch: &[Incoming]) {
        let Some(renderer) = self.renderer.as_mut() else { return };
        let mut dabs = Vec::new();
        for item in batch {
            self.stroker.feed(item.sample, &mut dabs);
        }
        if dabs.is_empty() {
            return;
        }
        renderer.draw_dabs(&dabs);
        if renderer.present() {
            if let Some(last) = batch.last() {
                self.stats.add(last.recv.elapsed().as_micros() as u64);
            }
        }
    }

    fn mouse_sample(&self, phase: Phase) -> Incoming {
        Incoming {
            sample: Sample {
                pen_id: 0,
                phase,
                x: self.cursor.0,
                y: self.cursor.1,
                tablet: None,
                pressure: Some(0.5),
                tilt: None,
                buttons: 0,
                eraser: false,
                t_us: now_us(),
                t_hardware: false,
            },
            recv: Instant::now(),
        }
    }

    fn show(&mut self, pinned: bool) {
        let Some(window) = self.window.clone() else { return };
        fit_to_monitor(&window);
        #[cfg(windows)]
        input_win::show_noactivate(&window);
        #[cfg(not(windows))]
        window.set_visible(true);
        // a hidden window keeps showing the last frame it presented: refresh it
        if let Some(r) = self.renderer.as_mut() {
            r.present();
        }
        println!("overlay shown{}", if pinned { " (pinned)" } else { "" });
    }

    fn hide(&mut self) {
        if self.options.clear_on_dismiss {
            if let Some(r) = self.renderer.as_mut() {
                r.clear();
                r.present(); // blank the swapchain too, while the window is still up, so nothing flashes on the next show
            }
        }
        if let Some(window) = self.window.as_ref() {
            #[cfg(windows)]
            input_win::hide(window);
            #[cfg(not(windows))]
            window.set_visible(false);
        }
        println!("overlay hidden");
        self.stroker = Stroker::default();
        self.mouse_down = false;
    }

    fn run(&mut self, action: Option<Action>) {
        match action {
            Some(Action::Show { pinned }) => self.show(pinned),
            Some(Action::Hide) => self.hide(),
            None => {}
        }
    }

    /// Hands-off: release every hotkey except the hands-off one (or take them back).
    fn release_hotkeys(&mut self, released: bool) {
        for (name, key) in [("toggle", self.keys.toggle), ("auto-show", self.keys.auto), ("quit", self.keys.quit)] {
            let result = if released { self.hotkeys.unregister(key) } else { self.hotkeys.register(key) };
            if let Err(e) = result {
                eprintln!("could not {} the {name} hotkey: {e}", if released { "release" } else { "register" });
            }
        }
    }

    fn sync_tray(&self) {
        if let Some(tray) = self.tray.as_ref() {
            let (auto_show, hands_off) = lifecycle(|l| (l.auto_show(), l.hands_off()));
            tray.sync(auto_show, hands_off, self.options.clear_on_dismiss);
        }
    }

    fn command(&mut self, el: &ActiveEventLoop, cmd: Command) {
        match cmd {
            Command::Toggle => {
                let action = lifecycle(|l| l.toggle());
                self.run(action);
            }
            Command::ToggleAuto => {
                let on = lifecycle(|l| {
                    let on = !l.auto_show();
                    l.set_auto_show(on);
                    on
                });
                println!("auto-show {}", if on { "on" } else { "off" });
            }
            Command::ToggleHandsOff => {
                let off = lifecycle(|l| {
                    let off = !l.hands_off();
                    l.set_hands_off(off);
                    off
                });
                self.release_hotkeys(off);
                println!("hands-off {}", if off { "ON: only Ctrl+Alt+K (and the tray menu) still work" } else { "off" });
            }
            Command::ToggleClearOnDismiss => {
                self.options.clear_on_dismiss = !self.options.clear_on_dismiss;
                println!("clear drawing when hidden: {}", if self.options.clear_on_dismiss { "on" } else { "off" });
            }
            Command::Quit => el.exit(),
        }
        self.sync_tray();
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }
        // Start hidden: the window gets its size and its first (transparent) frame before it is ever shown.
        let attrs = Window::default_attributes()
            .with_title("pen controller")
            .with_decorations(false)
            .with_transparent(true)
            .with_visible(false)
            .with_window_level(WindowLevel::AlwaysOnTop);
        // A DirectComposition swapchain does not need the window's own redirection bitmap, which would
        // show up as a white rectangle at the window's initial size. PEN_REDIRECT keeps it, to compare.
        #[cfg(windows)]
        let attrs = {
            use winit::platform::windows::WindowAttributesExtWindows;
            if std::env::var("PEN_REDIRECT").is_ok() {
                attrs
            } else {
                attrs.with_no_redirection_bitmap(true)
            }
        };
        let window = Arc::new(el.create_window(attrs).expect("create window"));
        fit_to_monitor(&window);

        #[cfg(windows)]
        input_win::install(&window);

        let size = window.inner_size(); let mut renderer = WgpuRenderer::new( window.clone(), (size.width, size.height), RendererOptions { border: Some(Border { width_px: 3.0, rgb: [1.0, 0.282, 0.690], alpha: 0.55 }), }, );
        renderer.present(); // first frame: fully transparent
        self.renderer = Some(Box::new(renderer));
        self.window = Some(window);

        // winit: the earliest a tray icon can be created is once the loop is running
        match Tray::new(self.options) {
            Ok(tray) => self.tray = Some(tray),
            Err(e) => eprintln!("no tray icon: {e}"),
        }
        println!("Hidden. Bring the pen near the tablet to show it. Ctrl+Alt+D pins it, Ctrl+Alt+K hands-off, Ctrl+Alt+Q quits.");
    }

    fn user_event(&mut self, el: &ActiveEventLoop, ev: UserEvent) {
        match ev {
            UserEvent::Samples => {
                let batch: Vec<Incoming> = std::mem::take(&mut *QUEUE.lock().unwrap());
                if !batch.is_empty() {
                    self.handle_batch(&batch);
                }
            }
            UserEvent::Overlay(action) => self.run(Some(action)),
            UserEvent::Command(cmd) => self.command(el, cmd),
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        match ev {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(r) = self.renderer.as_mut() {
                    r.present();
                }
            }
            // mouse fallback so the prototype is usable without a pen
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                if self.mouse_down {
                    let item = self.mouse_sample(Phase::Move);
                    self.handle_batch(&[item]);
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                self.mouse_down = state == ElementState::Pressed;
                let phase = if self.mouse_down { Phase::Down } else { Phase::Up };
                let item = self.mouse_sample(phase);
                self.handle_batch(&[item]);
            }
            _ => {}
        }
    }
}

fn main() {
    let _ = START.set(Instant::now());
    let _ = LIFECYCLE.set(Mutex::new(Lifecycle::new(IGNORE_APPS)));

    let event_loop = EventLoop::<UserEvent>::with_user_event().build().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    *PROXY.lock().unwrap() = Some(event_loop.create_proxy());

    // Hotkeys. The manager lives in the app (hands-off needs it to release and re-take them). A press is
    // translated into a command for the main loop; nothing is decided on this thread.
    let hotkeys = GlobalHotKeyManager::new().expect("hotkey manager");
    let mods = Some(Modifiers::CONTROL | Modifiers::ALT);
    let keys = Keys {
        toggle: HotKey::new(mods, Code::KeyD),
        auto: HotKey::new(mods, Code::KeyA),
        quit: HotKey::new(mods, Code::KeyQ),
        hands: HotKey::new(mods, Code::KeyK),
    };
    for (name, key) in [("toggle", keys.toggle), ("auto-show", keys.auto), ("hands-off", keys.hands), ("quit", keys.quit)] {
        if let Err(e) = hotkeys.register(key) {
            // usually means another program already owns that combination
            eprintln!("could not register the {name} hotkey: {e}");
        }
    }
    let ids = [keys.toggle.id(), keys.auto.id(), keys.hands.id(), keys.quit.id()];
    GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
        if e.state != HotKeyState::Pressed {
            return;
        }
        let cmd = if e.id == ids[0] {
            Command::Toggle
        } else if e.id == ids[1] {
            Command::ToggleAuto
        } else if e.id == ids[2] {
            Command::ToggleHandsOff
        } else if e.id == ids[3] {
            Command::Quit
        } else {
            return;
        };
        send(UserEvent::Command(cmd));
    }));

    // Pen in range / touching -> summon; a real mouse move -> dismiss. Runs on the pen thread: the rules
    // live in `Lifecycle`, and the actual show/hide happens on the main loop.
    #[cfg(windows)]
    {
        use pen_core::pen_win::{self, PenEvent};
        pen_win::spawn(pen_win::WACOM_CTL_4100, |ev| match ev {
            PenEvent::Summon => apply(lifecycle(|l| l.pen_summon(pen_win::foreground_exe))),
            PenEvent::Dismiss => apply(lifecycle(|l| l.mouse_moved())),
        });
    }

    let mut app = App {
        window: None,
        renderer: None,
        stroker: Stroker::default(),
        stats: Stats::default(),
        cursor: (0.0, 0.0),
        mouse_down: false,
        options: CanvasOptions { clear_on_dismiss: std::env::var("PEN_CLEAR_ON_DISMISS").is_ok() },
        tray: None,
        hotkeys,
        keys,
    };
    event_loop.run_app(&mut app).expect("run");
}
