//! Rendering behind a trait, so the renderer can be swapped (wgpu today; the webview overlay or a
//! Direct2D version later) without touching input, lifecycle or the brush.
//!
//! The wgpu implementation draws in two passes: dabs -> a persistent ink texture (premultiplied alpha),
//! then ink texture -> surface. Surface contents are undefined after present, so the ink has to live in
//! its own texture.
//!
//! No windowing crate here: `WgpuRenderer::new` takes anything wgpu accepts as a surface target
//! (an `Arc<winit::window::Window>` works) plus its size in pixels.

const MAX_DABS: usize = 16384;
const INK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// One stamp of the brush: what the brush engine hands to a renderer. Positions are surface pixels.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Dab {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub alpha: f32,
}

/// What the app needs from a renderer.
pub trait InkRenderer {
    /// The window changed size. Existing ink may be discarded.
    fn resize(&mut self, w: u32, h: u32);
    /// Remove all ink.
    fn clear(&mut self);
    /// Stamp dabs onto the ink.
    fn draw_dabs(&mut self, dabs: &[Dab]);
    /// Show the current ink (and the border, if any) on screen. False if no frame could be presented.
    fn present(&mut self) -> bool;
}

/// A frame drawn over the ink along the surface's edge (the controller uses it to show that the overlay
/// is up). The studio uses none.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Border {
    pub width_px: f32,
    /// Straight (not premultiplied) colour, 0..1.
    pub rgb: [f32; 3],
    /// 0..1.
    pub alpha: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RendererOptions {
    pub border: Option<Border>,
}

const STAMP_WGSL: &str = r#"
struct Params { screen: vec2<f32>, opaque: f32, border_width: f32, border: vec4<f32> };
@group(0) @binding(0) var<uniform> params: Params;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
};

@vertex
fn vs_stamp(
    @builtin(vertex_index) vi: u32,
    @location(0) center: vec2<f32>,
    @location(1) radius: f32,
    @location(2) alpha: f32,
) -> VsOut {
    var corners = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, 1.0)
    );
    let c = corners[vi];
    let half = radius + 1.0;
    let px = center + c * half;
    var o: VsOut;
    o.pos = vec4<f32>(px.x / params.screen.x * 2.0 - 1.0, 1.0 - px.y / params.screen.y * 2.0, 0.0, 1.0);
    o.uv = c * half / radius; // length 1.0 at the circle edge
    o.alpha = alpha;
    return o;
}

@fragment
fn fs_stamp(i: VsOut) -> @location(0) vec4<f32> {
    let d = length(i.uv);
    let a = (1.0 - smoothstep(0.75, 1.0, d)) * i.alpha;
    let col = vec3<f32>(1.0, 0.282, 0.690); // the shared pink
    return vec4<f32>(col * a, a); // premultiplied
}
"#;

const PRESENT_WGSL: &str = r#"
struct Params { screen: vec2<f32>, opaque: f32, border_width: f32, border: vec4<f32> };
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var ink: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[vi], 0.0, 1.0);
}

@fragment
fn fs_full(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    var c = textureLoad(ink, vec2<i32>(frag.xy), 0);
    // Optional border, drawn over the ink. `params.border` is premultiplied; "over" blend.
    // border_width is 0 when there is none, so the test below never passes.
    let edge = min(min(frag.x, frag.y), min(params.screen.x - frag.x, params.screen.y - frag.y));
    if (edge < params.border_width) {
        let b = params.border;
        c = b + c * (1.0 - b.a);
    }
    if (params.opaque > 0.5) {
        let bg = vec3<f32>(0.08, 0.09, 0.12);
        return vec4<f32>(c.rgb + bg * (1.0 - c.a), 1.0);
    }
    return c; // already premultiplied, matches CompositeAlphaMode::PreMultiplied
}
"#;

const DAB_ATTRS: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32, 2 => Float32];

pub struct WgpuRenderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    opaque: bool,
    border: Option<Border>,
    params: wgpu::Buffer,
    instances: wgpu::Buffer,
    stamp_pipe: wgpu::RenderPipeline,
    stamp_bg: wgpu::BindGroup,
    present_pipe: wgpu::RenderPipeline,
    present_bgl: wgpu::BindGroupLayout,
    present_bg: wgpu::BindGroup,
    ink_view: wgpu::TextureView,
}

impl WgpuRenderer {
    /// `target`: the window (or anything else wgpu can make a surface from), e.g. an `Arc<winit::window::Window>`.
    /// `size`: its current size in pixels.
    pub fn new(target: impl Into<wgpu::SurfaceTarget<'static>>, size: (u32, u32), options: RendererOptions) -> WgpuRenderer {
        let backends = match std::env::var("PEN_BACKEND").unwrap_or_default().to_lowercase().as_str() {
            "vulkan" => wgpu::Backends::VULKAN,
            "dx12" => wgpu::Backends::DX12,
            _ if cfg!(windows) => wgpu::Backends::DX12,
            _ => wgpu::Backends::PRIMARY,
        };
        // wgpu 30: Instance::new takes the descriptor by value and request_adapter returns a Result.
        // DxgiFromVisual puts the swapchain on a DirectComposition visual, which is what makes per-pixel
        // transparency possible on DX12. The other option (DxgiFromHwnd) ignores alpha.
        // PEN_DX12=hwnd switches back to it, to compare.
        let swapchain_kind = match std::env::var("PEN_DX12").unwrap_or_default().to_lowercase().as_str() {
            "hwnd" => wgpu::Dx12SwapchainKind::DxgiFromHwnd,
            _ => wgpu::Dx12SwapchainKind::DxgiFromVisual,
        };
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        desc.backends = backends;
        desc.backend_options.dx12.presentation_system = swapchain_kind;
        let instance = wgpu::Instance::new(desc);
        let surface = instance.create_surface(target).expect("create_surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("no adapter");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("device");

        let caps = surface.get_capabilities(&adapter);
        let info = adapter.get_info();
        println!("backend: {:?}   adapter: {}   dx12 presentation: {:?}", info.backend, info.name, swapchain_kind);
        println!("formats:       {:?}", caps.formats);
        println!("present modes: {:?}", caps.present_modes);
        println!("alpha modes:   {:?}", caps.alpha_modes);

        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let alpha_mode = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            caps.alpha_modes[0]
        };
        let opaque = alpha_mode != wgpu::CompositeAlphaMode::PreMultiplied;
        if opaque {
            println!("!! PreMultiplied alpha is not available here: falling back to an OPAQUE dark window.");
            println!("!! Check that PEN_DX12 is not set to 'hwnd', or try PEN_BACKEND=vulkan.");
            println!("!! Known wgpu issue: the alpha mode must be PreMultiplied at the FIRST configure (it is here).");
        }
        let present_mode = match std::env::var("PEN_PRESENT").unwrap_or_default().to_lowercase().as_str() {
            "vsync" => wgpu::PresentMode::AutoVsync,
            "fifo" => wgpu::PresentMode::Fifo,
            "mailbox" => wgpu::PresentMode::Mailbox,
            "immediate" => wgpu::PresentMode::Immediate,
            // default: Mailbox (no vsync blocking, no tearing) when offered, else whatever has no vsync wait
            _ if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) => wgpu::PresentMode::Mailbox,
            _ => wgpu::PresentMode::AutoNoVsync,
        };

        // Start from the default config so new fields (e.g. color_space) are filled in, then override.
        // alpha_mode is set here, BEFORE the first configure(), on purpose.
        let mut config = surface
            .get_default_config(&adapter, size.0.max(1), size.1.max(1))
            .expect("surface not supported by this adapter");
        config.usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        config.format = format;
        config.present_mode = present_mode;
        config.desired_maximum_frame_latency = 1;
        config.alpha_mode = alpha_mode;
        config.view_formats = vec![];
        surface.configure(&device, &config);

        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("params"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dabs"),
            size: (MAX_DABS * std::mem::size_of::<Dab>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // stamp pipeline: dabs -> ink texture
        let stamp_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("stamp"),
            source: wgpu::ShaderSource::Wgsl(STAMP_WGSL.into()),
        });
        let stamp_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("stamp bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let stamp_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("stamp bg"),
            layout: &stamp_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() }],
        });
        let stamp_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("stamp layout"),
            bind_group_layouts: &[Some(&stamp_bgl)],
            immediate_size: 0,
        });
        let stamp_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("stamp"),
            layout: Some(&stamp_layout),
            vertex: wgpu::VertexState {
                module: &stamp_mod,
                entry_point: Some("vs_stamp"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Dab>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &DAB_ATTRS,
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &stamp_mod,
                entry_point: Some("fs_stamp"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: INK_FORMAT,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // present pipeline: ink texture -> surface
        let present_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("present"),
            source: wgpu::ShaderSource::Wgsl(PRESENT_WGSL.into()),
        });
        let present_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("present bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let present_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("present layout"),
            bind_group_layouts: &[Some(&present_bgl)],
            immediate_size: 0,
        });
        let present_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("present"),
            layout: Some(&present_layout),
            vertex: wgpu::VertexState {
                module: &present_mod,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &present_mod,
                entry_point: Some("fs_full"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let (ink_view, present_bg) = Self::make_ink(&device, &present_bgl, &params, config.width, config.height);
        let mut g = WgpuRenderer {
            surface,
            device,
            queue,
            config,
            opaque,
            border: options.border,
            params,
            instances,
            stamp_pipe,
            stamp_bg,
            present_pipe,
            present_bgl,
            present_bg,
            ink_view,
        };
        g.write_params();
        g.clear_ink();
        g
    }

    fn make_ink(
        device: &wgpu::Device,
        bgl: &wgpu::BindGroupLayout,
        params: &wgpu::Buffer,
        w: u32,
        h: u32,
    ) -> (wgpu::TextureView, wgpu::BindGroup) {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ink"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: INK_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("present bg"),
            layout: bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
            ],
        });
        (view, bg)
    }

    /// Layout matches `Params` in the shaders: screen (2), opaque, border width, border colour (premultiplied).
    fn write_params(&self) {
        let (bw, b) = match self.border {
            Some(b) => (b.width_px, [b.rgb[0] * b.alpha, b.rgb[1] * b.alpha, b.rgb[2] * b.alpha, b.alpha]),
            None => (0.0, [0.0; 4]),
        };
        let p = [
            self.config.width as f32,
            self.config.height as f32,
            if self.opaque { 1.0 } else { 0.0 },
            bw,
            b[0],
            b[1],
            b[2],
            b[3],
        ];
        self.queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&p));
    }

    fn resize_impl(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 || (w == self.config.width && h == self.config.height) {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(&self.device, &self.config);
        // the ink does not survive a resize yet
        let (view, bg) = Self::make_ink(&self.device, &self.present_bgl, &self.params, w, h);
        self.ink_view = view;
        self.present_bg = bg;
        self.write_params();
        self.clear_ink();
    }

    fn clear_ink(&self) {
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear ink"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.ink_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        self.queue.submit([enc.finish()]);
    }

    fn draw_dabs_impl(&self, dabs: &[Dab]) {
        for chunk in dabs.chunks(MAX_DABS) {
            // one submit per chunk: write_buffer to the same offset twice before a submit would overwrite
            self.queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(chunk));
            let mut enc = self.device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("stamp"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.ink_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.stamp_pipe);
                pass.set_bind_group(0, &self.stamp_bg, &[]);
                pass.set_vertex_buffer(0, self.instances.slice(..));
                pass.draw(0..4, 0..chunk.len() as u32);
            }
            self.queue.submit([enc.finish()]);
        }
    }

    /// Returns true if a frame was actually presented.
    fn present_impl(&mut self) -> bool {
        // wgpu 29+: get_current_texture returns a single enum instead of a Result.
        let (frame, reconfigure_after) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => (f, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => (f, true),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return false;
            }
            // Timeout, Occluded, Validation: skip this frame
            _ => return false,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.present_pipe);
            pass.set_bind_group(0, &self.present_bg, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([enc.finish()]);
        self.queue.present(frame);
        if reconfigure_after {
            // only safe once the frame has been presented
            self.surface.configure(&self.device, &self.config);
        }
        true
    }
}

impl InkRenderer for WgpuRenderer {
    fn resize(&mut self, w: u32, h: u32) {
        self.resize_impl(w, h)
    }
    fn clear(&mut self) {
        self.clear_ink()
    }
    fn draw_dabs(&mut self, dabs: &[Dab]) {
        self.draw_dabs_impl(dabs)
    }
    fn present(&mut self) -> bool {
        self.present_impl()
    }
}
