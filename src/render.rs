//! GPU 管線（wgpu）：擷取 → 環形記憶緩衝（texture array）→ 分身合成 → Bloom → 輸出。
//!
//! 只依賴 `wgpu::Device`／`Queue`，所以桌面程式（eframe 的裝置）與離線算圖（無視窗裝置）共用同一份程式。

use bytemuck::{Pod, Zeroable};
use eframe::wgpu;
use wgpu::util::DeviceExt;

use crate::frame::{Frame, Mask};
use crate::memory::EchoInstance;
use crate::params::{BgMode, ColourMode, Params, Surface};

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const BLOOM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const MAX_INSTANCES: usize = 64;
const MONITOR_WIDTH: u32 = 320;
/// Bloom 亮部門檻（規格 3.3：固定 0.7）
const BLOOM_THRESHOLD: f32 = 0.7;
/// Disintegrate 雜訊尺度（規格：先寫死）
const NOISE_SCALE: f32 = 9.0;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    out_size: [f32; 2],
    clip: [f32; 2],
    grade: [f32; 4],
    modes: [f32; 4],
    custom: [f32; 4],
    duo_dark: [f32; 4],
    duo_light: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuInstance {
    rect: [f32; 4],
    layer: u32,
    erosion: f32,
    opacity: f32,
    seed: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct PostUniform {
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
}

struct Tex {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl Tex {
    fn new(device: &wgpu::Device, label: &str, w: u32, h: u32, format: wgpu::TextureFormat, extra: wgpu::TextureUsages) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | extra,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Tex { texture, view, width: w.max(1), height: h.max(1) }
    }
}

struct Memory {
    _texture: wgpu::Texture,
    array_view: wgpu::TextureView,
    layer_views: Vec<wgpu::TextureView>,
    width: u32,
    height: u32,
}

struct Targets {
    comp: Tex,
    half_a: Tex,
    half_b: Tex,
    quarter_a: Tex,
    quarter_b: Tex,
    output: Tex,
}

/// 每個全畫面 pass 用自己的 uniform 槽，同一次提交裡不互相覆蓋。
#[derive(Clone, Copy)]
enum Slot {
    Capture,
    Background,
    Bright,
    BlurHalfH,
    BlurHalfV,
    Down,
    BlurQuarterH,
    BlurQuarterV,
    Final,
    Monitor0,
    Monitor1,
    Monitor2,
}
const SLOT_COUNT: usize = 12;

pub struct RenderRequest<'a> {
    pub params: &'a Params,
    /// 由舊到新
    pub echoes: &'a [EchoInstance],
    pub out_width: u32,
    pub out_height: u32,
    /// 背景「原始畫面」模式時，來源在輸出中的矩形（uv：左、上、右、下）
    pub source_rect_uv: [f32; 4],
    pub time: f32,
    pub monitors: bool,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    max_layers: u32,
    sampler: wgpu::Sampler,
    post_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    p_capture: wgpu::RenderPipeline,
    p_background: wgpu::RenderPipeline,
    p_bright: wgpu::RenderPipeline,
    p_blur: wgpu::RenderPipeline,
    p_final: wgpu::RenderPipeline,
    p_monitor: wgpu::RenderPipeline,
    p_composite: wgpu::RenderPipeline,
    slots: Vec<wgpu::Buffer>,
    globals: wgpu::Buffer,
    instances: wgpu::Buffer,
    lut: Tex,
    lut_key: Option<[i32; 3]>,
    src: Tex,
    mask: Tex,
    memory: Option<Memory>,
    targets: Option<Targets>,
    monitors: Option<[Tex; 3]>,
    /// 每次重建輸出或監看貼圖就 +1，讓介面知道要重新註冊
    pub generation: u64,
}

impl Renderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let max_layers = device.limits().max_texture_array_layers;
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let tex_entry = |binding: u32, dim: wgpu::TextureViewDimension, filterable: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: dim,
                multisampled: false,
            },
            count: None,
        };
        let uniform_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let sampler_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let d2 = wgpu::TextureViewDimension::D2;

        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post layout"),
            entries: &[uniform_entry(0), tex_entry(1, d2, true), tex_entry(2, d2, true), tex_entry(3, d2, true), sampler_entry(4)],
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("composite layout"),
            entries: &[
                uniform_entry(0),
                tex_entry(1, wgpu::TextureViewDimension::D2Array, true),
                sampler_entry(2),
                tex_entry(3, d2, false),
            ],
        });

        let post_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/post.wgsl").into()),
        });
        let comp_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("composite.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/composite.wgsl").into()),
        });

        let post_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post pl"),
            bind_group_layouts: &[Some(&post_layout)],
            immediate_size: 0,
        });
        let post_pipeline = |entry: &str, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&post_pl),
                vertex: wgpu::VertexState {
                    module: &post_shader,
                    entry_point: Some("vs_full"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &post_shader,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let p_capture = post_pipeline("fs_capture", COLOR_FORMAT);
        let p_background = post_pipeline("fs_background", COLOR_FORMAT);
        let p_bright = post_pipeline("fs_bright", BLOOM_FORMAT);
        let p_blur = post_pipeline("fs_blur", BLOOM_FORMAT);
        let p_final = post_pipeline("fs_final", COLOR_FORMAT);
        let p_monitor = post_pipeline("fs_monitor", COLOR_FORMAT);

        let comp_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("composite pl"),
            bind_group_layouts: &[Some(&composite_layout)],
            immediate_size: 0,
        });
        let p_composite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite"),
            layout: Some(&comp_pl),
            vertex: wgpu::VertexState {
                module: &comp_shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Uint32, 2 => Float32, 3 => Float32, 4 => Float32],
                })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &comp_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        let slots = (0..SLOT_COUNT)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("post uniform"),
                    size: std::mem::size_of::<PostUniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (std::mem::size_of::<GpuInstance>() * MAX_INSTANCES) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let copy_dst = wgpu::TextureUsages::COPY_DST;
        let lut = Tex::new(&device, "curve lut", 256, 1, wgpu::TextureFormat::R8Unorm, copy_dst);
        let src = Tex::new(&device, "source", 1, 1, COLOR_FORMAT, copy_dst);
        let mask = Tex::new(&device, "mask", 1, 1, wgpu::TextureFormat::R8Unorm, copy_dst);

        let r = Renderer {
            device,
            queue,
            max_layers,
            sampler,
            post_layout,
            composite_layout,
            p_capture,
            p_background,
            p_bright,
            p_blur,
            p_final,
            p_monitor,
            p_composite,
            slots,
            globals,
            instances,
            lut,
            lut_key: None,
            src,
            mask,
            memory: None,
            targets: None,
            monitors: None,
            generation: 0,
        };
        r.write_tex(&r.mask, &[255], 1);
        r
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn max_layers(&self) -> u32 {
        self.max_layers
    }

    fn write_tex(&self, tex: &Tex, data: &[u8], bytes_per_px: u32) {
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &tex.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(tex.width * bytes_per_px), rows_per_image: Some(tex.height) },
            wgpu::Extent3d { width: tex.width, height: tex.height, depth_or_array_layers: 1 },
        );
    }

    pub fn upload_source(&mut self, frame: &Frame) {
        if self.src.width != frame.width || self.src.height != frame.height {
            self.src = Tex::new(&self.device, "source", frame.width, frame.height, COLOR_FORMAT, wgpu::TextureUsages::COPY_DST);
            self.generation += 1;
        }
        self.write_tex(&self.src, &frame.rgba, 4);
    }

    pub fn upload_mask(&mut self, mask: &Mask) {
        if self.mask.width != mask.width || self.mask.height != mask.height {
            self.mask = Tex::new(&self.device, "mask", mask.width, mask.height, wgpu::TextureFormat::R8Unorm, wgpu::TextureUsages::COPY_DST);
        }
        self.write_tex(&self.mask, &mask.to_u8(), 1);
    }

    /// 配置（或沿用）記憶緩衝；重新配置時回傳 true（內容全清）。
    pub fn ensure_memory(&mut self, width: u32, height: u32, layers: u32) -> bool {
        let layers = layers.clamp(2, self.max_layers);
        if let Some(m) = &self.memory {
            if m.width == width && m.height == height && m.layer_views.len() as u32 == layers {
                return false;
            }
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("echo memory"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: layers },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let array_view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let layer_views = (0..layers)
            .map(|i| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        self.memory = Some(Memory { _texture: texture, array_view, layer_views, width, height });
        true
    }

    pub fn memory_size(&self) -> Option<(u32, u32, u32)> {
        self.memory.as_ref().map(|m| (m.width, m.height, m.layer_views.len() as u32))
    }

    fn post_bind_group(&self, slot: Slot, u: PostUniform, t0: &wgpu::TextureView, t1: &wgpu::TextureView, t2: &wgpu::TextureView) -> wgpu::BindGroup {
        let buf = &self.slots[slot as usize];
        self.queue.write_buffer(buf, 0, bytemuck::bytes_of(&u));
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.post_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(t0) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(t1) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(t2) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        })
    }

    fn fullscreen(enc: &mut wgpu::CommandEncoder, pipeline: &wgpu::RenderPipeline, bg: &wgpu::BindGroup, target: &wgpu::TextureView) {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bg, &[]);
        pass.draw(0..3, 0..1);
    }

    /// 把目前的來源影格＋遮罩寫進記憶緩衝第 `layer` 層。
    pub fn capture(&mut self, layer: usize, mirror: bool) {
        let Some(mem) = &self.memory else { return };
        let u = PostUniform { c: [mirror as u32 as f32, 0.0, 0.0, 0.0], ..Default::default() };
        let bg = self.post_bind_group(Slot::Capture, u, &self.src.view, &self.mask.view, &self.mask.view);
        let mut enc = self.device.create_command_encoder(&Default::default());
        Self::fullscreen(&mut enc, &self.p_capture, &bg, &mem.layer_views[layer]);
        self.queue.submit([enc.finish()]);
    }

    fn ensure_targets(&mut self, w: u32, h: u32) {
        if self.targets.as_ref().is_some_and(|t| t.output.width == w && t.output.height == h) {
            return;
        }
        let d = &self.device;
        let ra = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let (hw, hh, qw, qh) = ((w / 2).max(1), (h / 2).max(1), (w / 4).max(1), (h / 4).max(1));
        self.targets = Some(Targets {
            comp: Tex::new(d, "comp", w, h, COLOR_FORMAT, ra),
            half_a: Tex::new(d, "bloom half a", hw, hh, BLOOM_FORMAT, ra),
            half_b: Tex::new(d, "bloom half b", hw, hh, BLOOM_FORMAT, ra),
            quarter_a: Tex::new(d, "bloom quarter a", qw, qh, BLOOM_FORMAT, ra),
            quarter_b: Tex::new(d, "bloom quarter b", qw, qh, BLOOM_FORMAT, ra),
            output: Tex::new(d, "output", w, h, COLOR_FORMAT, ra | wgpu::TextureUsages::COPY_SRC),
        });
        self.generation += 1;
    }

    fn ensure_monitors(&mut self) {
        let w = MONITOR_WIDTH;
        let h = ((w as f32 * self.src.height as f32 / self.src.width as f32).round() as u32).max(1);
        if self.monitors.as_ref().is_some_and(|m| m[0].width == w && m[0].height == h) {
            return;
        }
        let ra = wgpu::TextureUsages::RENDER_ATTACHMENT;
        self.monitors = Some([0, 1, 2].map(|_| Tex::new(&self.device, "monitor", w, h, COLOR_FORMAT, ra)));
        self.generation += 1;
    }

    /// 畫一張輸出畫面。
    pub fn render(&mut self, req: &RenderRequest) {
        let p = req.params;
        self.ensure_targets(req.out_width, req.out_height);
        if req.monitors {
            self.ensure_monitors();
        }

        let key = [p.curve_shadows, p.curve_mids, p.curve_highs].map(|v| v.round() as i32);
        if self.lut_key != Some(key) {
            let lut = crate::curve::build_lut(p.curve_shadows, p.curve_mids, p.curve_highs);
            self.write_tex(&self.lut, &lut, 1);
            self.lut_key = Some(key);
        }

        let rgba = |c: [f32; 3]| [c[0], c[1], c[2], 1.0];
        let globals = Globals {
            out_size: [req.out_width as f32, req.out_height as f32],
            clip: [p.clip_black / 100.0, p.clip_white / 100.0],
            grade: [p.brightness / 100.0, p.contrast / 100.0, p.saturation / 100.0, NOISE_SCALE],
            modes: [
                matches!(p.surface, Surface::Solid) as u32 as f32,
                match p.colour_mode {
                    ColourMode::Original => 0.0,
                    ColourMode::White => 1.0,
                    ColourMode::Mono => 2.0,
                    ColourMode::Custom => 3.0,
                },
                p.duotone as u32 as f32,
                req.time,
            ],
            custom: rgba(p.custom_colour.to_f32()),
            duo_dark: rgba(p.duotone_dark.to_f32()),
            duo_light: rgba(p.duotone_light.to_f32()),
        };
        self.queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));

        let inst: Vec<GpuInstance> = req
            .echoes
            .iter()
            .take(MAX_INSTANCES)
            .map(|e| GpuInstance { rect: e.rect, layer: e.layer, erosion: e.erosion, opacity: e.opacity, seed: e.index as f32 })
            .collect();
        if !inst.is_empty() {
            self.queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&inst));
        }

        let t = self.targets.as_ref().unwrap();
        let mut enc = self.device.create_command_encoder(&Default::default());

        // 背景
        let bg_u = PostUniform {
            a: req.source_rect_uv,
            b: rgba(p.bg_colour.to_f32()),
            c: [p.mirror as u32 as f32, matches!(p.bg_mode, BgMode::Source) as u32 as f32, 0.0, 0.0],
        };
        let bg = self.post_bind_group(Slot::Background, bg_u, &self.src.view, &self.src.view, &self.src.view);
        Self::fullscreen(&mut enc, &self.p_background, &bg, &t.comp.view);

        // 分身
        if let (Some(mem), false) = (&self.memory, inst.is_empty()) {
            let lut_view = &self.lut.view;
            let cbg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("composite bg"),
                layout: &self.composite_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.globals.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&mem.array_view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(lut_view) },
                ],
            });
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.comp.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.p_composite);
            pass.set_bind_group(0, &cbg, &[]);
            pass.set_vertex_buffer(0, self.instances.slice(..));
            pass.draw(0..6, 0..inst.len() as u32);
        }

        // Bloom：亮部 → 1/2 模糊 → 1/4 模糊 → 疊回
        let strength = p.bloom / 100.0 * 1.6;
        if strength > 0.0 {
            let texel = |tex: &Tex| [1.0 / tex.width as f32, 1.0 / tex.height as f32];
            let bright_u = PostUniform { a: [BLOOM_THRESHOLD, 0.0, 0.0, 0.0], ..Default::default() };
            let b = self.post_bind_group(Slot::Bright, bright_u, &t.comp.view, &t.comp.view, &t.comp.view);
            Self::fullscreen(&mut enc, &self.p_bright, &b, &t.half_a.view);
            let [tx, ty] = texel(&t.half_a);
            let b = self.post_bind_group(Slot::BlurHalfH, PostUniform { a: [tx, 0.0, 0.0, 0.0], ..Default::default() }, &t.half_a.view, &t.half_a.view, &t.half_a.view);
            Self::fullscreen(&mut enc, &self.p_blur, &b, &t.half_b.view);
            let b = self.post_bind_group(Slot::BlurHalfV, PostUniform { a: [0.0, ty, 0.0, 0.0], ..Default::default() }, &t.half_b.view, &t.half_b.view, &t.half_b.view);
            Self::fullscreen(&mut enc, &self.p_blur, &b, &t.half_a.view);
            let b = self.post_bind_group(Slot::Down, PostUniform::default(), &t.half_a.view, &t.half_a.view, &t.half_a.view);
            Self::fullscreen(&mut enc, &self.p_blur, &b, &t.quarter_a.view);
            let [qx, qy] = texel(&t.quarter_a);
            let b = self.post_bind_group(Slot::BlurQuarterH, PostUniform { a: [qx, 0.0, 0.0, 0.0], ..Default::default() }, &t.quarter_a.view, &t.quarter_a.view, &t.quarter_a.view);
            Self::fullscreen(&mut enc, &self.p_blur, &b, &t.quarter_b.view);
            let b = self.post_bind_group(Slot::BlurQuarterV, PostUniform { a: [0.0, qy, 0.0, 0.0], ..Default::default() }, &t.quarter_b.view, &t.quarter_b.view, &t.quarter_b.view);
            Self::fullscreen(&mut enc, &self.p_blur, &b, &t.quarter_a.view);
        }
        let fin_u = PostUniform { a: [strength * 0.5, 0.0, 0.0, 0.0], ..Default::default() };
        let (g1, g2) = if strength > 0.0 { (&t.half_a.view, &t.quarter_a.view) } else { (&t.comp.view, &t.comp.view) };
        let b = self.post_bind_group(Slot::Final, fin_u, &t.comp.view, g1, g2);
        Self::fullscreen(&mut enc, &self.p_final, &b, &t.output.view);

        // 監看小窗
        if req.monitors {
            let mons = self.monitors.as_ref().unwrap();
            for (i, slot) in [Slot::Monitor0, Slot::Monitor1, Slot::Monitor2].into_iter().enumerate() {
                let u = PostUniform {
                    a: [mons[i].width as f32, mons[i].height as f32, 0.0, 0.0],
                    c: [p.mirror as u32 as f32, i as f32, 0.0, 0.0],
                    ..Default::default()
                };
                let b = self.post_bind_group(slot, u, &self.src.view, &self.mask.view, &self.mask.view);
                Self::fullscreen(&mut enc, &self.p_monitor, &b, &mons[i].view);
            }
        }

        self.queue.submit([enc.finish()]);
    }

    pub fn output_view(&self) -> Option<&wgpu::TextureView> {
        self.targets.as_ref().map(|t| &t.output.view)
    }

    pub fn output_size(&self) -> Option<(u32, u32)> {
        self.targets.as_ref().map(|t| (t.output.width, t.output.height))
    }

    pub fn monitor_views(&self) -> Option<[&wgpu::TextureView; 3]> {
        self.monitors.as_ref().map(|m| [&m[0].view, &m[1].view, &m[2].view])
    }

    pub fn monitor_size(&self) -> Option<(u32, u32)> {
        self.monitors.as_ref().map(|m| (m[0].width, m[0].height))
    }

    /// 把輸出畫面讀回 CPU（RGBA8，緊密排列）。會等 GPU 做完。
    pub fn read_output(&self) -> Option<Frame> {
        let t = self.targets.as_ref()?;
        let (w, h) = (t.output.width, t.output.height);
        let row = (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("readback"),
            contents: &vec![0u8; (row * h) as usize],
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &t.output.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).ok()?;
        let data = slice.get_mapped_range().ok()?;
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            let start = (y * row) as usize;
            rgba.extend_from_slice(&data[start..start + (w * 4) as usize]);
        }
        drop(data);
        buf.unmap();
        Some(Frame::new(w, h, rgba))
    }
}
