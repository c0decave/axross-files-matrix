//! `--benchmark [FILES]`: the frame time where it matters - on the GPU of
//! the machine it runs on. A synthetic city of FILES files (200 000 by
//! default) is built, a fixed flight is flown over it for a number of
//! frames (from the overview down to the roofs, round the island), each
//! frame rendered offscreen with no vsync and waited for. Reported, as text
//! or JSON: the adapter and its API, the boxes and draw calls per frame,
//! the CPU side of a frame (level of detail, instances, encoding) and the
//! whole frame (CPU and GPU), as median, p95 and p99.

use crate::camera::{Camera, look_angles};
use crate::city::{self, FrameSet, IslandCity, Lod, Ring};
use crate::gpu::{self, Preference};
use crate::layout::{Params, layout, snapshot};
use crate::math::{Frustum, vec3};
use crate::pick::Bvh;
use crate::render::{FrameData, Globals, Renderer};
use crate::walk::{Limits, Source, walk_blocking};
use eframe::wgpu;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct BenchOptions {
    pub files: usize,
    pub frames: usize,
    pub size: (u32, u32),
    /// No level of detail: every building drawn, every frame.
    pub all: bool,
    /// One flat folder of FILES files (the worst case for the level of
    /// detail) instead of a tree of folders.
    pub flat: bool,
    /// Smooth edges (four samples a pixel), as the window draws by default.
    pub msaa: bool,
    pub preference: Preference,
}

impl Default for BenchOptions {
    fn default() -> BenchOptions {
        BenchOptions {
            files: 200_000,
            frames: 600,
            size: (1920, 1080),
            all: false,
            flat: false,
            msaa: true,
            preference: Preference::Auto,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Spread {
    pub median: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

impl Spread {
    fn of(v: &mut [f64]) -> Spread {
        if v.is_empty() {
            return Spread::default();
        }
        v.sort_by(f64::total_cmp);
        let at = |q: f64| v[((v.len() as f64 * q) as usize).min(v.len() - 1)];
        let r = |x: f64| (x * 1000.0).round() / 1000.0;
        Spread {
            median: r(at(0.5)),
            p95: r(at(0.95)),
            p99: r(at(0.99)),
            max: r(v[v.len() - 1]),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Counts {
    pub min: u64,
    pub median: u64,
    pub max: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct BenchReport {
    pub adapter: String,
    pub backend: String,
    pub device_type: String,
    pub software: bool,
    pub files: usize,
    pub nodes: usize,
    pub buildings: usize,
    pub districts: usize,
    pub build_seconds: f64,
    pub frames: usize,
    pub size: [u32; 2],
    pub level_of_detail: bool,
    pub flat: bool,
    /// Samples a pixel (4: smooth edges).
    pub samples: u32,
    /// The most boxes a frame may draw (the level of detail's budget).
    pub budget: usize,
    /// Boxes per frame (buildings, blocks, floors, islands).
    pub instances: Counts,
    /// Of them, buildings.
    pub buildings_drawn: Counts,
    pub draw_calls: u32,
    /// Draw calls that draw the boxes (instanced).
    pub instanced_draw_calls: u32,
    /// Level of detail, instances and encoding (milliseconds).
    pub cpu_ms: Spread,
    /// The whole frame, CPU and GPU, waited for (milliseconds).
    pub frame_ms: Spread,
    pub fps_median: f64,
}

impl BenchReport {
    pub fn text(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "axross-files-matrix benchmark: {} files{}, {} frames at {} x {}, {} samples a pixel{}\n",
            self.files,
            if self.flat { " in one folder" } else { "" },
            self.frames,
            self.size[0],
            self.size[1],
            self.samples,
            if self.level_of_detail {
                ""
            } else {
                ", every building drawn"
            }
        ));
        s.push_str(&format!(
            "adapter: {} ({}, {})\n",
            self.adapter, self.backend, self.device_type
        ));
        if self.software {
            s.push_str("note: a software renderer - these are no GPU numbers\n");
        }
        s.push_str(&format!(
            "city: {} nodes, {} buildings, {} districts, built in {:.2} s\n",
            self.nodes, self.buildings, self.districts, self.build_seconds
        ));
        s.push_str(&format!(
            "per frame: {} - {} boxes (median {}), of them {} - {} buildings; {} draw calls, {} of them instanced\n",
            self.instances.min,
            self.instances.max,
            self.instances.median,
            self.buildings_drawn.min,
            self.buildings_drawn.max,
            self.draw_calls,
            self.instanced_draw_calls
        ));
        s.push_str(&format!(
            "CPU side (level of detail, instances, encoding): median {:.2} ms, p95 {:.2} ms, p99 {:.2} ms\n",
            self.cpu_ms.median, self.cpu_ms.p95, self.cpu_ms.p99
        ));
        s.push_str(&format!(
            "frame (CPU and GPU, waited for): median {:.2} ms, p95 {:.2} ms, p99 {:.2} ms - {:.0} frames a second\n",
            self.frame_ms.median, self.frame_ms.p95, self.frame_ms.p99, self.fps_median
        ));
        s
    }
}

/// A tree of one folder holding `files` files (sizes spread over three
/// orders of magnitude).
pub fn flat_tree(files: usize) -> crate::tree::Tree {
    use crate::tree::{Entry, Kind, Tree};
    let mut t = Tree::new(std::ffi::OsStr::new("flat"), 0);
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    for i in 0..files {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let size = 1_000 + x % 10_000_000;
        t.push(Entry {
            parent: 0,
            name: std::ffi::OsString::from(format!("file-{i:07}.dat")).into_boxed_os_str(),
            kind: Kind::File,
            size,
            alloc: size.div_ceil(4096) * 4096,
            mtime: 1_700_000_000 - (x % 10_000_000) as i64,
        });
    }
    t.recount();
    t
}

/// The camera at frame `k` of `n`: round the island, from high above down
/// to the roofs.
fn camera_at(k: usize, n: usize, center: crate::math::Vec3, side: f32) -> Camera {
    let s = k as f32 / n.max(1) as f32;
    let angle = std::f32::consts::TAU * s;
    let radius = side * (0.85 - 0.55 * s);
    let height = side * (0.55 - 0.5 * s) + 30.0;
    let eye = center + vec3(angle.cos() * radius, height, angle.sin() * radius);
    let at = center + vec3(angle.cos() * radius * 0.2, 0.0, angle.sin() * radius * 0.2);
    let (yaw, pitch) = look_angles(eye, at);
    Camera {
        pos: eye,
        yaw,
        pitch,
        ..Default::default()
    }
}

/// Runs the benchmark (headless: no window, no vsync).
pub fn run(opts: &BenchOptions) -> Result<BenchReport, String> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    let name = std::env::var("WGPU_ADAPTER_NAME").ok();
    let power = std::env::var("WGPU_POWER_PREF").ok();
    let adapter = gpu::choose(
        &adapters,
        None,
        opts.preference,
        name.as_deref(),
        power.as_deref(),
    )
    .map_err(|e| format!("no GPU adapter ({e})"))?;
    let info = adapter.get_info();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("afm benchmark"),
        ..Default::default()
    }))
    .map_err(|e| format!("the GPU device could not be made: {e}"))?;
    // the city
    let t = Instant::now();
    let tree = if opts.flat {
        flat_tree(opts.files)
    } else {
        walk_blocking(
            Source::Synthetic {
                files: opts.files,
                seed: 7,
            },
            Limits::default(),
            std::ffi::OsStr::new("synthetic"),
        )
        .0
    };
    let l = layout(&snapshot(&tree), &Params::default());
    let boxes = city::boxes_of(&l);
    let bvh = Bvh::build(&boxes);
    let (nodes, buildings, districts) = (tree.len(), l.buildings.len(), l.districts.len());
    let mut c = IslandCity::new(Ring::Local, Arc::new(l), Arc::new(bvh), Arc::new(boxes));
    let theme = &crate::theme::MATRIX;
    c.rebuild(theme);
    let build_seconds = t.elapsed().as_secs_f64();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let msaa_ok = crate::render::msaa_supported(&adapter);
    let mut renderer = Renderer::new(&device, format, msaa_ok);
    let samples = if opts.msaa && msaa_ok { 4 } else { 1 };
    let budget = if opts.all {
        usize::MAX
    } else {
        city::FRAME_BUDGET
    };
    let (w, h) = (opts.size.0.clamp(16, 8192), opts.size.1.clamp(16, 8192));
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("afm benchmark target"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let lod = Lod {
        collapse: if opts.all {
            0.0
        } else {
            Lod::default().collapse
        },
        raise: crate::render::Raise {
            look: crate::render::far_look(c.side(), c.layout.top),
            ..crate::render::Raise::NONE
        },
        ..Lod::default()
    };
    let v4 = |c: [f32; 3], w: f32| [c[0], c[1], c[2], w];
    let (center, side) = (c.center(), c.side());
    let mut set = FrameSet::default();
    let frames = opts.frames.max(1);
    let warm = 5.min(frames);
    let (mut cpu, mut total) = (Vec::new(), Vec::new());
    let (mut inst, mut blds) = (Vec::new(), Vec::new());
    let mut draws = (0, 0);
    let mut buffer: Option<Arc<Vec<city::Instance>>> = None;
    for k in 0..frames + warm {
        let t0 = Instant::now();
        let cam = camera_at(k.saturating_sub(warm), frames, center, side);
        let aspect = w as f32 / h as f32;
        let vp = cam.proj(aspect).mul(&cam.view());
        let frustum = Frustum::from_view_proj(&vp);
        if let Some(old) = buffer.take().and_then(|b| Arc::try_unwrap(b).ok()) {
            set.opaque = old;
        }
        let lod = Lod {
            raise: crate::render::Raise {
                eye: cam.pos,
                lens: 2.0 * (cam.fov_y * 0.5).tan(),
                height: opts.size.1 as f32,
                ..lod.raise
            },
            ..lod
        };
        city::select_within(&mut [&mut c], cam.pos, &frustum, &lod, budget, &mut set);
        let opaque = set.opaque.len() as u32;
        let built = set
            .opaque
            .iter()
            .filter(|i| i.shape() == city::Shape::Building as u32)
            .count();
        let instances = Arc::new(std::mem::take(&mut set.opaque));
        let data = FrameData {
            globals: Globals {
                view_proj: vp.to_cols(),
                eye: [cam.pos.x, cam.pos.y, cam.pos.z, k as f32 / 60.0],
                fog: v4(theme.fog, 0.00055),
                sky_top: v4(theme.sky_top, theme.rain_amount),
                sky_horizon: v4(theme.sky_horizon, theme.scanlines),
                rain: v4(theme.rain, theme.bloom),
                edge: v4(theme.edge, 1.0),
                grid: v4(theme.grid, 1.0),
                highlight: v4(theme.highlight, 0.0),
                params: [0.0, 1.35, 1.0, 0.1],
                look: crate::render::far_look(side, c.layout.top),
                lens: [2.0 * (cam.fov_y * 0.5).tan(), 0.0, 0.0, 0.0],
                ..Default::default()
            },
            instances: instances.clone(),
            opaque,
            version: k as u64,
            guard: true,
            dt: 1.0 / 60.0,
            msaa: opts.msaa,
            cut: false,
        };
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("afm benchmark"),
        });
        renderer.bench_frame(&device, &queue, &mut enc, &data, (w, h), &view);
        let cmd = enc.finish();
        let t1 = Instant::now();
        queue.submit([cmd]);
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        let t2 = Instant::now();
        drop(data);
        buffer = Some(instances);
        if k >= warm {
            cpu.push((t1 - t0).as_secs_f64() * 1000.0);
            total.push((t2 - t0).as_secs_f64() * 1000.0);
            inst.push(renderer.instance_count() as u64);
            blds.push(built as u64);
            draws = (renderer.draw_calls(true), (opaque > 0) as u32);
        }
    }
    let counts = |v: &mut Vec<u64>| {
        v.sort_unstable();
        Counts {
            min: *v.first().unwrap_or(&0),
            median: v.get(v.len() / 2).copied().unwrap_or(0),
            max: *v.last().unwrap_or(&0),
        }
    };
    let frame_ms = Spread::of(&mut total);
    Ok(BenchReport {
        adapter: info.name.clone(),
        backend: format!("{:?}", info.backend),
        device_type: format!("{:?}", info.device_type),
        software: gpu::is_software(&info.name, info.device_type),
        files: opts.files,
        nodes,
        buildings,
        districts,
        build_seconds: (build_seconds * 1000.0).round() / 1000.0,
        frames,
        size: [w, h],
        level_of_detail: !opts.all,
        flat: opts.flat,
        samples,
        budget: if opts.all { 0 } else { budget },
        instances: counts(&mut inst),
        buildings_drawn: counts(&mut blds),
        draw_calls: draws.0,
        instanced_draw_calls: draws.1,
        cpu_ms: Spread::of(&mut cpu),
        fps_median: if frame_ms.median > 0.0 {
            (1000.0 / frame_ms.median * 10.0).round() / 10.0
        } else {
            0.0
        },
        frame_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spreads_and_the_flight() {
        let mut v: Vec<f64> = (1..=100).map(|x| x as f64).collect();
        let s = Spread::of(&mut v);
        assert_eq!((s.median, s.p95, s.p99, s.max), (51.0, 96.0, 100.0, 100.0));
        assert_eq!(Spread::of(&mut []).median, 0.0);
        let c = crate::math::Vec3::ZERO;
        let first = camera_at(0, 100, c, 1000.0);
        let last = camera_at(99, 100, c, 1000.0);
        assert!(first.pos.y > last.pos.y, "it comes down");
        assert!(last.pos.y >= 30.0);
    }
}
