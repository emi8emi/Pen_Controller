//! The config window (`--config`): the tray menu's switches as big buttons, for platforms where there is
//! no tray (Linux/sway) and for pens, which are bad at small menus.
//!
//! It is a second winit window in the same process. It never changes state itself: it draws what
//! `View` says and returns the `Command`s that were clicked, which the app runs through the same
//! path the tray and the hotkeys use.

use std::sync::Arc;

use egui::{Color32, RichText, Vec2, ViewportId};
use egui_wgpu::{RendererOptions, WgpuConfiguration};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::Command;

/// Not "pen controller": the sway rule in the README matches that title for the overlay.
const TITLE: &str = "pen-ctl config";

/// What the window shows. Copied out of `Lifecycle` and `CanvasOptions` before every frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct View {
    pub visible: bool,
    pub auto_show: bool,
    pub hands_off: bool,
    pub clear_on_dismiss: bool,
}

pub(crate) struct ConfigWindow {
    window: Arc<Window>,
    ctx: egui::Context,
    state: egui_winit::State,
    painter: egui_wgpu::winit::Painter,
}

impl ConfigWindow {
    pub fn new(el: &ActiveEventLoop) -> Self {
        let attrs = Window::default_attributes()
            .with_title(TITLE)
            .with_inner_size(winit::dpi::LogicalSize::new(460.0, 520.0));
        #[cfg(target_os = "linux")]
        let attrs = {
            use winit::platform::wayland::WindowAttributesExtWayland;
            attrs.with_name("pen-controller-config", "pen-controller-config")
        };
        let window = Arc::new(el.create_window(attrs).expect("create config window"));

        let ctx = egui::Context::default();
        // pen-sized: egui's defaults are made for a mouse
        ctx.set_zoom_factor(1.5);
        ctx.global_style_mut(|s| {
            s.spacing.item_spacing = Vec2::new(10.0, 12.0);
            s.spacing.button_padding = Vec2::new(14.0, 12.0);
        });

        let mut painter = pollster::block_on(egui_wgpu::winit::Painter::new(
            ctx.clone(),
            WgpuConfiguration::default(),
            false,
            RendererOptions::default(),
        ));
        pollster::block_on(painter.set_window(ViewportId::ROOT, Some(window.clone())))
            .expect("config window surface");

        let state = egui_winit::State::new(
            ctx.clone(),
            ViewportId::ROOT,
            &*window,
            Some(window.scale_factor() as f32),
            window.theme(),
            painter.max_texture_side(),
        );
        window.request_redraw();
        ConfigWindow {
            window,
            ctx,
            state,
            painter,
        }
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    /// Feed a window event to egui. Returns true when the window should be redrawn.
    pub fn on_event(&mut self, ev: &WindowEvent) -> bool {
        if let WindowEvent::Resized(size) = ev {
            if let (Some(w), Some(h)) = (
                std::num::NonZeroU32::new(size.width),
                std::num::NonZeroU32::new(size.height),
            ) {
                self.painter.on_window_resized(ViewportId::ROOT, w, h);
            }
        }
        self.state.on_window_event(&self.window, ev).repaint
    }

    /// Run one frame, paint it, and return the commands that were clicked in it.
    pub fn frame(&mut self, view: View) -> Vec<Command> {
        let mut out = Vec::new();
        let input = self.state.take_egui_input(&self.window);
        let full = self.ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| draw(ui, view, &mut out));
        });
        self.state
            .handle_platform_output(&self.window, full.platform_output);

        let ppp = full.pixels_per_point;
        let prims = self.ctx.tessellate(full.shapes, ppp);
        let mut textures = full.textures_delta;
        self.painter.paint_and_update_textures(
            ViewportId::ROOT,
            ppp,
            [0.0, 0.0, 0.0, 1.0],
            &prims,
            &mut textures,
            Vec::new(),
            &self.window,
        );
        // egui asks for another frame while animating (hover fades, press feedback)
        let wants_more = full
            .viewport_output
            .get(&ViewportId::ROOT)
            .is_some_and(|v| v.repaint_delay.is_zero());
        if wants_more {
            self.window.request_redraw();
        }
        out
    }
}

fn draw(ui: &mut egui::Ui, view: View, out: &mut Vec<Command>) {
    ui.heading("Pen controller");
    ui.add_space(4.0);

    let big = |text: RichText, w: f32| egui::Button::new(text).min_size(Vec2::new(w, 56.0));
    let w = ui.available_width();

    let (label, fill) = if view.visible {
        ("Hide overlay", Color32::from_rgb(120, 60, 90))
    } else {
        ("Show overlay", Color32::from_rgb(190, 50, 130))
    };
    if ui.add(big(RichText::new(label).size(18.0), w).fill(fill)).clicked() {
        out.push(Command::Toggle);
    }
    ui.label(
        RichText::new(if view.visible {
            "Pinned: the mouse will not hide it."
        } else {
            "Shown by hand stays up until you hide it."
        })
        .weak(),
    );

    ui.separator();
    switch(ui, "Show when the pen is used", view.auto_show, Command::ToggleAuto, out);
    switch(
        ui,
        "Clear drawing when hidden",
        view.clear_on_dismiss,
        Command::ToggleClearOnDismiss,
        out,
    );
    switch(
        ui,
        "Hands-off: ignore hotkeys",
        view.hands_off,
        Command::ToggleHandsOff,
        out,
    );

    ui.separator();
    if ui.add(big(RichText::new("Quit").size(18.0), w)).clicked() {
        out.push(Command::Quit);
    }
}

/// A big on/off row. The state shown is the real one, so a hotkey flipping it shows up here too.
fn switch(ui: &mut egui::Ui, text: &str, on: bool, cmd: Command, out: &mut Vec<Command>) {
    let (mark, fill) = if on {
        ("ON ", Color32::from_rgb(40, 110, 70))
    } else {
        ("off", Color32::from_gray(55))
    };
    let b = egui::Button::new(RichText::new(format!("[{mark}]  {text}")).size(16.0))
        .fill(fill)
        .min_size(Vec2::new(ui.available_width(), 52.0));
    if ui.add(b).clicked() {
        out.push(cmd);
    }
}
