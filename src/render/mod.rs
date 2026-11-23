//! The city on the GPU, inside egui's frame (an egui paint callback on
//! eframe's wgpu renderer): the scene into an HDR picture of its own (sky
//! and rain, sea, instanced boxes - one draw call for every building), the
//! bloom chain, the final picture (tone, scanlines, warp, the flash
//! guard's veil), its slow average, and the coarse luminance grid read
//! back for the guard ([`crate::flash`]). `paint` only copies the final
//! picture into egui's pass.

use crate::city::Instance;
use crate::flash::{FlashGuard, GRID_H, GRID_W};
use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor, wgpu};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

/// The uniforms both shaders read (see city.wgsl's `Globals`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Globals {
    pub view_proj: [[f32; 4]; 4],
    pub inv_view_proj: [[f32; 4]; 4],
    pub eye: [f32; 4],
    pub fog: [f32; 4],
    pub sky_top: [f32; 4],
    pub sky_horizon: [f32; 4],
    pub rain: [f32; 4],
    pub edge: [f32; 4],
    pub grid: [f32; 4],
    pub highlight: [f32; 4],
    pub hover_min: [f32; 4],
    pub hover_max: [f32; 4],
    pub sel_min: [f32; 4],
    pub sel_max: [f32; 4],
    pub viewport: [f32; 4],
    pub params: [f32; 4],
    /// x: a test signal (a flashing picture the guard must veil; 0 in use).
    pub debug: [f32; 4],
}

/// What one frame draws (shared with the paint callback).
pub struct FrameData {
    pub globals: Globals,
    /// The instances (opaque first, then the glowing ones) and how many
    /// are opaque; `version` changes when they do.
    pub instances: Arc<Vec<Instance>>,
    pub opaque: u32,
    pub version: u64,
    pub guard: bool,
    pub dt: f32,
}

/// What the renderer tells the window (the flash guard's state, the GPU).
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub veil: bool,
    pub engaged: u64,
    pub adapter: String,
    pub software: bool,
    pub instances: u32,
    pub size: (u32, u32),
}

const SCENE: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const FINAL: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The guard's grid: 8 bits each (luminance, saturated red / 320) - a
/// format every backend can render to and copy.
const LUMA: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const BLOOM_LEVELS: usize = 4;
/// A luminance row padded to wgpu's copy alignment.
const LUMA_ROW: u32 = ((GRID_W as u32 * 4).div_ceil(256)) * 256;
const STAGING: usize = 3;

const FREE: u8 = 0;
const COPIED: u8 = 1;
const MAPPING: u8 = 2;
const MAPPED: u8 = 3;

struct Staging {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    /// The time between the frames it measured and the one before.
    dt: f32,
}

struct Targets {
    size: (u32, u32),
    scene: wgpu::TextureView,
    depth: wgpu::TextureView,
    bloom: Vec<wgpu::TextureView>,
    final_view: wgpu::TextureView,
    history: [wgpu::TextureView; 2],
    current: usize,
    luma: wgpu::Texture,
    luma_view: wgpu::TextureView,
    bright_bg: wgpu::BindGroup,
    down_bg: Vec<wgpu::BindGroup>,
    up_bg: Vec<wgpu::BindGroup>,
    final_bg: [wgpu::BindGroup; 2],
    history_bg: [wgpu::BindGroup; 2],
    luma_bg: wgpu::BindGroup,
    blit_bg: wgpu::BindGroup,
}

pub struct Renderer {
    globals: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    tex_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    sky: wgpu::RenderPipeline,
    sea: wgpu::RenderPipeline,
    boxes: wgpu::RenderPipeline,
    glow: wgpu::RenderPipeline,
    bright: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    final_pipe: wgpu::RenderPipeline,
    history: wgpu::RenderPipeline,
    luma: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
    instances: wgpu::Buffer,
    capacity: u64,
    version: u64,
    count: u32,
    opaque: u32,
    targets: Option<Targets>,
    veil: wgpu::Texture,
    veil_view: wgpu::TextureView,
    dummy: wgpu::TextureView,
    staging: Vec<Staging>,
    frame: u64,
    guard: FlashGuard,
    pending_dt: f32,
    linear_target: bool,
    pub stats: Arc<Mutex<Stats>>,
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    extra: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size.0.max(1),
            height: size.1.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | extra,
        view_formats: &[],
    })
}

fn color(
    v: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
) -> Option<wgpu::RenderPassColorAttachment<'_>> {
    Some(wgpu::RenderPassColorAttachment {
        view: v,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load,
            store: wgpu::StoreOp::Store,
        },
    })
}

fn view(t: &wgpu::Texture) -> wgpu::TextureView {
    t.create_view(&wgpu::TextureViewDescriptor::default())
}

impl Renderer {
    /// Sets the renderer up in egui's wgpu renderer (once, as the window
    /// is made); `stats` is what it reports back.
    pub fn install(rs: &egui_wgpu::RenderState) -> Arc<Mutex<Stats>> {
        let r = Renderer::new(&rs.device, rs.target_format);
        let info = rs.adapter.get_info();
        if let Ok(mut s) = r.stats.lock() {
            s.adapter = crate::gpu::describe(&info);
            s.software = crate::gpu::is_software(&info.name, info.device_type);
        }
        let stats = r.stats.clone();
        rs.renderer.write().callback_resources.insert(r);
        stats
    }

    pub fn new(device: &wgpu::Device, target: wgpu::TextureFormat) -> Renderer {
        let city = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("afm city"),
            source: wgpu::ShaderSource::Wgsl(include_str!("city.wgsl").into()),
        });
        let post = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("afm post"),
            source: wgpu::ShaderSource::Wgsl(include_str!("post.wgsl").into()),
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("afm globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("afm textures"),
            entries: &[
                tex_entry(0),
                tex_entry(1),
                tex_entry(2),
                tex_entry(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("afm globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("afm globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        // (no filtering: some adapters cannot filter half floats -
        // the shaders take their taps themselves; the shaders take their taps themselves)
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("afm sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let scene_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("afm scene"),
            bind_group_layouts: &[Some(&globals_layout)],
            immediate_size: 0,
        });
        let post_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("afm post"),
            bind_group_layouts: &[Some(&globals_layout), Some(&tex_layout)],
            immediate_size: 0,
        });
        let depth = |write: bool, compare: wgpu::CompareFunction| {
            Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(write),
                depth_compare: Some(compare),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            })
        };
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::REPLACE,
        };
        let inst_attrs =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Uint32, 2 => Float32x3, 3 => Uint32];
        let inst_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Instance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &inst_attrs,
        };
        #[allow(clippy::too_many_arguments)]
        let pipeline = |label: &str,
                        layout: &wgpu::PipelineLayout,
                        module: &wgpu::ShaderModule,
                        vs: &str,
                        fs: &str,
                        format: wgpu::TextureFormat,
                        blend: Option<wgpu::BlendState>,
                        depth_stencil: Option<wgpu::DepthStencilState>,
                        buffers: &[Option<wgpu::VertexBufferLayout>],
                        cull: Option<wgpu::Face>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: cull,
                    ..Default::default()
                },
                depth_stencil,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        use wgpu::CompareFunction as C;
        let sky = pipeline(
            "afm sky",
            &scene_layout,
            &city,
            "vs_full",
            "fs_sky",
            SCENE,
            None,
            depth(false, C::Always),
            &[],
            None,
        );
        let sea = pipeline(
            "afm sea",
            &scene_layout,
            &city,
            "vs_full",
            "fs_sea",
            SCENE,
            None,
            depth(true, C::Greater),
            &[],
            None,
        );
        let boxes = pipeline(
            "afm boxes",
            &scene_layout,
            &city,
            "vs_box",
            "fs_box",
            SCENE,
            None,
            depth(true, C::Greater),
            &[Some(inst_layout.clone())],
            Some(wgpu::Face::Back),
        );
        let glow = pipeline(
            "afm glow",
            &scene_layout,
            &city,
            "vs_box",
            "fs_box",
            SCENE,
            Some(additive),
            depth(false, C::Greater),
            &[Some(inst_layout.clone())],
            None,
        );
        let bright = pipeline(
            "afm bright",
            &post_layout,
            &post,
            "vs_full",
            "fs_bright",
            SCENE,
            None,
            None,
            &[],
            None,
        );
        let down = pipeline(
            "afm down",
            &post_layout,
            &post,
            "vs_full",
            "fs_down",
            SCENE,
            None,
            None,
            &[],
            None,
        );
        let up = pipeline(
            "afm up",
            &post_layout,
            &post,
            "vs_full",
            "fs_up",
            SCENE,
            Some(additive),
            None,
            &[],
            None,
        );
        let final_pipe = pipeline(
            "afm final",
            &post_layout,
            &post,
            "vs_full",
            "fs_final",
            FINAL,
            None,
            None,
            &[],
            None,
        );
        let history = pipeline(
            "afm history",
            &post_layout,
            &post,
            "vs_full",
            "fs_history",
            FINAL,
            None,
            None,
            &[],
            None,
        );
        let luma = pipeline(
            "afm luma",
            &post_layout,
            &post,
            "vs_full",
            "fs_luma",
            LUMA,
            None,
            None,
            &[],
            None,
        );
        let blit = pipeline(
            "afm blit",
            &post_layout,
            &post,
            "vs_full",
            "fs_blit",
            target,
            None,
            None,
            &[],
            None,
        );
        let capacity = 4096u64;
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("afm instances"),
            size: capacity * std::mem::size_of::<Instance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let veil = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("afm veil"),
            size: wgpu::Extent3d {
                width: GRID_W as u32,
                height: GRID_H as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let veil_view = view(&veil);
        let dummy = view(&texture(
            device,
            "afm dummy",
            (1, 1),
            FINAL,
            wgpu::TextureUsages::empty(),
        ));
        let staging = (0..STAGING)
            .map(|_| Staging {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("afm luma readback"),
                    size: (LUMA_ROW * GRID_H as u32) as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(FREE)),
                dt: 0.0,
            })
            .collect();
        Renderer {
            globals,
            globals_bg,
            tex_layout,
            sampler,
            sky,
            sea,
            boxes,
            glow,
            bright,
            down,
            up,
            final_pipe,
            history,
            luma,
            blit,
            instances,
            capacity,
            version: u64::MAX,
            count: 0,
            opaque: 0,
            targets: None,
            veil,
            veil_view,
            dummy,
            staging,
            frame: 0,
            guard: FlashGuard::new(),
            pending_dt: 0.0,
            linear_target: target.is_srgb(),
            stats: Arc::new(Mutex::new(Stats::default())),
        }
    }

    fn group(&self, device: &wgpu::Device, views: [&wgpu::TextureView; 4]) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("afm post textures"),
            layout: &self.tex_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(views[2]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(views[3]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    fn ensure_targets(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let scene_t = texture(
            device,
            "afm scene",
            size,
            SCENE,
            wgpu::TextureUsages::empty(),
        );
        let depth_t = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("afm depth"),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let mut bloom = Vec::new();
        let mut s = size;
        for _ in 0..BLOOM_LEVELS {
            s = ((s.0 / 2).max(1), (s.1 / 2).max(1));
            bloom.push(view(&texture(
                device,
                "afm bloom",
                s,
                SCENE,
                wgpu::TextureUsages::empty(),
            )));
        }
        let final_t = texture(
            device,
            "afm final",
            size,
            FINAL,
            wgpu::TextureUsages::empty(),
        );
        let history = [
            view(&texture(
                device,
                "afm history",
                size,
                FINAL,
                wgpu::TextureUsages::empty(),
            )),
            view(&texture(
                device,
                "afm history",
                size,
                FINAL,
                wgpu::TextureUsages::empty(),
            )),
        ];
        let luma = texture(
            device,
            "afm luma",
            (GRID_W as u32, GRID_H as u32),
            LUMA,
            wgpu::TextureUsages::COPY_SRC,
        );
        let scene = view(&scene_t);
        let final_view = view(&final_t);
        let d = &self.dummy;
        let bright_bg = self.group(device, [&scene, d, d, d]);
        let down_bg = (0..BLOOM_LEVELS - 1)
            .map(|i| self.group(device, [&bloom[i], d, d, d]))
            .collect();
        let up_bg = (1..BLOOM_LEVELS)
            .map(|i| self.group(device, [&bloom[i], d, d, d]))
            .collect();
        let final_bg = [
            self.group(device, [&scene, &bloom[0], &history[0], &self.veil_view]),
            self.group(device, [&scene, &bloom[0], &history[1], &self.veil_view]),
        ];
        let history_bg = [
            self.group(device, [&history[0], &final_view, d, d]),
            self.group(device, [&history[1], &final_view, d, d]),
        ];
        // (the raw picture: the scene and its bloom, before the veil)
        let luma_bg = self.group(device, [&scene, &bloom[0], d, d]);
        let blit_bg = self.group(device, [&final_view, d, d, d]);
        self.targets = Some(Targets {
            size,
            scene,
            depth: view(&depth_t),
            bloom,
            final_view,
            history,
            current: 0,
            luma_view: view(&luma),
            luma,
            bright_bg,
            down_bg,
            up_bg,
            final_bg,
            history_bg,
            luma_bg,
            blit_bg,
        });
    }

    /// Reads back what the GPU measured, feeds the guard, uploads its veil.
    fn read_guard(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let _ = device.poll(wgpu::PollType::Poll);
        for k in 0..self.staging.len() {
            let st = &self.staging[k];
            match st.state.load(Ordering::Acquire) {
                COPIED => {
                    st.state.store(MAPPING, Ordering::Release);
                    let flag = st.state.clone();
                    st.buffer.map_async(wgpu::MapMode::Read, .., move |r| {
                        flag.store(if r.is_ok() { MAPPED } else { FREE }, Ordering::Release);
                    });
                }
                MAPPED => {
                    let (mut lum, mut red) =
                        (vec![0f32; GRID_W * GRID_H], vec![0f32; GRID_W * GRID_H]);
                    if let Ok(data) = st.buffer.get_mapped_range(..) {
                        for y in 0..GRID_H {
                            for x in 0..GRID_W {
                                let o = y * LUMA_ROW as usize + x * 4;
                                lum[y * GRID_W + x] = data[o] as f32 / 255.0;
                                red[y * GRID_W + x] = data[o + 1] as f32 / 255.0 * 320.0;
                            }
                        }
                    }
                    st.buffer.unmap();
                    st.state.store(FREE, Ordering::Release);
                    let dt = st.dt;
                    self.guard.update(&lum, &red, dt);
                    let bytes = self.guard.veil_bytes();
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &self.veil,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        &bytes,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(GRID_W as u32),
                            rows_per_image: Some(GRID_H as u32),
                        },
                        wgpu::Extent3d {
                            width: GRID_W as u32,
                            height: GRID_H as u32,
                            depth_or_array_layers: 1,
                        },
                    );
                    if let Ok(mut s) = self.stats.lock() {
                        s.veil = self.guard.active();
                        s.engaged = self.guard.engaged;
                    }
                }
                _ => {}
            }
        }
    }

    fn prepare_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        f: &FrameData,
        size: (u32, u32),
    ) {
        self.ensure_targets(device, size);
        self.frame += 1;
        if f.guard {
            self.read_guard(device, queue);
        }
        let mut g = f.globals;
        g.viewport = [
            size.0 as f32,
            size.1 as f32,
            1.0 / size.0.max(1) as f32,
            1.0 / size.1.max(1) as f32,
        ];
        g.params[0] = if self.linear_target { 1.0 } else { 0.0 };
        g.params[2] = if f.guard { 1.0 } else { 0.0 };
        // the slow average: about a tenth of a second
        g.params[3] = 1.0 - (-f.dt.max(0.001) / 0.12).exp();
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&g));
        if f.version != self.version {
            let n = f.instances.len() as u64;
            if n > self.capacity {
                self.capacity = n.next_power_of_two();
                self.instances = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("afm instances"),
                    size: self.capacity * std::mem::size_of::<Instance>() as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
            if n > 0 {
                queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&f.instances));
            }
            self.count = n as u32;
            self.opaque = f.opaque.min(n as u32);
            self.version = f.version;
        }
        let Some(t) = self.targets.as_mut() else {
            return;
        };
        let cur = t.current;
        // the scene
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("afm scene"),
                color_attachments: &[color(&t.scene, wgpu::LoadOp::Clear(wgpu::Color::BLACK))],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bg, &[]);
            pass.set_pipeline(&self.sky);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&self.sea);
            pass.draw(0..3, 0..1);
            if self.count > 0 {
                pass.set_vertex_buffer(0, self.instances.slice(..));
                if self.opaque > 0 {
                    pass.set_pipeline(&self.boxes);
                    pass.draw(0..30, 0..self.opaque);
                }
                if self.count > self.opaque {
                    pass.set_pipeline(&self.glow);
                    pass.draw(0..30, self.opaque..self.count);
                }
            }
        }
        let post = |encoder: &mut wgpu::CommandEncoder,
                    label: &str,
                    target: &wgpu::TextureView,
                    pipe: &wgpu::RenderPipeline,
                    bg: &wgpu::BindGroup,
                    load: bool| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[color(
                    target,
                    if load {
                        wgpu::LoadOp::Load
                    } else {
                        wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                    },
                )],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bg, &[]);
            pass.set_bind_group(1, bg, &[]);
            pass.set_pipeline(pipe);
            pass.draw(0..3, 0..1);
        };
        post(
            encoder,
            "afm bright",
            &t.bloom[0],
            &self.bright,
            &t.bright_bg,
            false,
        );
        for i in 0..BLOOM_LEVELS - 1 {
            post(
                encoder,
                "afm down",
                &t.bloom[i + 1],
                &self.down,
                &t.down_bg[i],
                false,
            );
        }
        for i in (0..BLOOM_LEVELS - 1).rev() {
            post(encoder, "afm up", &t.bloom[i], &self.up, &t.up_bg[i], true);
        }
        post(
            encoder,
            "afm final",
            &t.final_view,
            &self.final_pipe,
            &t.final_bg[cur],
            false,
        );
        post(
            encoder,
            "afm history",
            &t.history[1 - cur],
            &self.history,
            &t.history_bg[cur],
            false,
        );
        t.current = 1 - cur;
        if f.guard {
            self.pending_dt += f.dt;
            post(
                encoder,
                "afm luma",
                &t.luma_view,
                &self.luma,
                &t.luma_bg,
                false,
            );
            if let Some(st) = self
                .staging
                .iter_mut()
                .find(|s| s.state.load(Ordering::Acquire) == FREE)
            {
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &t.luma,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &st.buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(LUMA_ROW),
                            rows_per_image: Some(GRID_H as u32),
                        },
                    },
                    wgpu::Extent3d {
                        width: GRID_W as u32,
                        height: GRID_H as u32,
                        depth_or_array_layers: 1,
                    },
                );
                st.dt = self.pending_dt;
                self.pending_dt = 0.0;
                st.state.store(COPIED, Ordering::Release);
            }
        }
        if let Ok(mut s) = self.stats.lock() {
            s.instances = self.count;
            s.size = size;
        }
    }
}

impl Renderer {
    /// The draw calls a frame issues: the sky, the sea, the boxes (one
    /// instanced call for every opaque box, one for the glowing ones), the
    /// bloom chain, the picture, its average, the guard's grid, the copy
    /// into the window.
    pub fn draw_calls(&self, guard: bool) -> u32 {
        let boxes = (self.opaque > 0) as u32 + (self.count > self.opaque) as u32;
        2 + boxes + 1 + 2 * (BLOOM_LEVELS as u32 - 1) + 2 + guard as u32 + 1
    }

    /// The boxes drawn in the last frame.
    pub fn instance_count(&self) -> u32 {
        self.count
    }

    /// A whole frame into `target` (of the format given to [`Renderer::new`]),
    /// as egui's pass would get it: the measurements and headless pictures.
    pub fn bench_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        f: &FrameData,
        size: (u32, u32),
        target: &wgpu::TextureView,
    ) {
        self.prepare_frame(device, queue, encoder, f, size);
        let Some(t) = self.targets.as_ref() else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("afm blit"),
            color_attachments: &[color(target, wgpu::LoadOp::Clear(wgpu::Color::BLACK))],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &self.globals_bg, &[]);
        pass.set_bind_group(1, &t.blit_bg, &[]);
        pass.set_pipeline(&self.blit);
        pass.draw(0..3, 0..1);
    }
}

/// The paint callback: the frame's data and where it is drawn.
pub struct Paint {
    pub frame: Arc<FrameData>,
    pub rect: egui::Rect,
}

impl CallbackTrait for Paint {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(r) = resources.get_mut::<Renderer>() else {
            return Vec::new();
        };
        let ppp = screen.pixels_per_point;
        let w = ((self.rect.width() * ppp).round() as u32).clamp(1, 8192);
        let h = ((self.rect.height() * ppp).round() as u32).clamp(1, 8192);
        r.prepare_frame(device, queue, encoder, &self.frame, (w, h));
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(r) = resources.get::<Renderer>() else {
            return;
        };
        let Some(t) = r.targets.as_ref() else { return };
        pass.set_bind_group(0, &r.globals_bg, &[]);
        pass.set_bind_group(1, &t.blit_bg, &[]);
        pass.set_pipeline(&r.blit);
        pass.draw(0..3, 0..1);
    }
}

/// The callback for egui's painter.
pub fn callback(rect: egui::Rect, frame: Arc<FrameData>) -> egui::PaintCallback {
    egui_wgpu::Callback::new_paint_callback(rect, Paint { frame, rect })
}
