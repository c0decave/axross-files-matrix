//! The numbers: a synthetic tree of N files (default 200 000) walked,
//! laid out and drawn - the walk's speed, the layout's time, the level of
//! detail's CPU time per frame and the boxes it chooses from several
//! places, the GPU's time per frame (headless, on whatever adapter wgpu
//! finds), and the memory.
//!
//!     cargo run --release --example bench [-- FILES [WIDTH HEIGHT]]

use axross_files_matrix::camera::{Camera, look_angles};
use axross_files_matrix::city::{self, FrameSet, IslandCity, Lod, Ring};
use axross_files_matrix::layout::{Params, layout, snapshot};
use axross_files_matrix::math::{Frustum, Vec3, vec3};
use axross_files_matrix::pick::Bvh;
use axross_files_matrix::render::{FrameData, Globals, Renderer};
use axross_files_matrix::walk::{Limits, Source, walk_blocking};
use eframe::wgpu;
use std::sync::Arc;
use std::time::Instant;

fn rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok())
        })
        .map_or(0.0, |kb| kb / 1024.0)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let files: usize = args.first().and_then(|v| v.parse().ok()).unwrap_or(200_000);
    let (w, h): (u32, u32) = (
        args.get(1).and_then(|v| v.parse().ok()).unwrap_or(1920),
        args.get(2).and_then(|v| v.parse().ok()).unwrap_or(1080),
    );
    let rss0 = rss_mb();
    let t = Instant::now();
    let (tree, _) = walk_blocking(
        Source::Synthetic { files, seed: 7 },
        Limits::default(),
        std::ffi::OsStr::new("synthetic"),
    );
    let walk_s = t.elapsed().as_secs_f64();
    println!(
        "tree: {} nodes ({} files) streamed in {:.3} s ({:.0} entries/s)",
        tree.len(),
        tree.root().count,
        walk_s,
        tree.len() as f64 / walk_s
    );
    let t = Instant::now();
    let l = layout(&snapshot(&tree), &Params::default());
    let layout_s = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let boxes = city::boxes_of(&l);
    let bvh = Bvh::build(&boxes);
    println!(
        "layout: {} districts, {} buildings, side {:.0} m in {:.3} s; BVH in {:.3} s",
        l.districts.len(),
        l.buildings.len(),
        l.side,
        layout_s,
        t.elapsed().as_secs_f64()
    );
    let mut c = IslandCity::new(Ring::Local, Arc::new(l), Arc::new(bvh), Arc::new(boxes));
    c.rebuild(&axross_files_matrix::theme::MATRIX);
    println!(
        "memory: {:.0} MB resident with the tree and the city ({:.0} MB before)",
        rss_mb(),
        rss0
    );
    let center = c.center();
    let side = c.side();
    let views = [
        (
            "overview, all of it",
            center + vec3(0.0, side * 0.6, side * 0.8),
            center,
        ),
        (
            "street level",
            center + vec3(side * 0.1, 25.0, side * 0.1),
            center + vec3(side * 0.3, 0.0, side * 0.3),
        ),
        (
            "over the roofs",
            center + vec3(-side * 0.2, 120.0, -side * 0.2),
            center,
        ),
        ("straight down", center + vec3(0.0, side * 1.2, 1.0), center),
    ];
    // the GPU
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok();
    let gpu = adapter.as_ref().map(|a| {
        let (device, queue) =
            pollster::block_on(a.request_device(&wgpu::DeviceDescriptor::default()))
                .expect("a device");
        (device, queue, a.get_info())
    });
    if let Some((_, _, info)) = &gpu {
        println!(
            "adapter: {} ({:?}, {:?})",
            info.name, info.backend, info.device_type
        );
    } else {
        println!("adapter: none (CPU numbers only)");
    }
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let msaa_ok = adapter
        .as_ref()
        .is_some_and(axross_files_matrix::render::msaa_supported);
    let mut renderer = gpu
        .as_ref()
        .map(|(d, _, _)| Renderer::new(d, format, msaa_ok));
    for lod_label in [
        "fixed (collapse below 0.03 rad)",
        "all opened (no collapse)",
    ] {
        let lod = if lod_label.starts_with("all") {
            Lod {
                collapse: 0.0,
                ..Lod::default()
            }
        } else {
            Lod::default()
        };
        println!("\nlevel of detail {lod_label}:");
        for (name, eye, at) in views {
            let (yaw, pitch) = look_angles(eye, at);
            let cam = Camera {
                pos: eye,
                yaw,
                pitch,
                ..Default::default()
            };
            let aspect = w as f32 / h as f32;
            let vp = cam.proj(aspect).mul(&cam.view());
            let frustum = Frustum::from_view_proj(&vp);
            let mut set = FrameSet::default();
            let runs = 20;
            let t = Instant::now();
            for _ in 0..runs {
                city::select(&mut [&mut c], cam.pos, &frustum, &lod, &mut set);
            }
            let cpu_ms = t.elapsed().as_secs_f64() * 1000.0 / runs as f64;
            let buildings = set
                .opaque
                .iter()
                .filter(|i| i.shape() == city::Shape::Building as u32)
                .count();
            let mut line = format!(
                "  {name:22} {:>7} boxes ({buildings} buildings, {} blocks), select {cpu_ms:.2} ms",
                set.opaque.len(),
                set.collapsed
            );
            if let (Some((device, queue, _)), Some(r)) = (gpu.as_ref(), renderer.as_mut()) {
                let target = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("bench target"),
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
                // (the Matrix theme's values, as the window sets them)
                let t = &axross_files_matrix::theme::MATRIX;
                let v4 = |c: [f32; 3], w: f32| [c[0], c[1], c[2], w];
                let g = Globals {
                    view_proj: vp.to_cols(),
                    eye: [eye.x, eye.y, eye.z, 1.0],
                    fog: v4(t.fog, 0.00055),
                    sky_top: v4(t.sky_top, t.rain_amount),
                    sky_horizon: v4(t.sky_horizon, t.scanlines),
                    rain: v4(t.rain, t.bloom),
                    edge: v4(t.edge, 1.0),
                    grid: v4(t.grid, 1.0),
                    highlight: v4(t.highlight, 0.0),
                    params: [0.0, 1.35, 1.0, 0.1],
                    ..Default::default()
                };
                let mut frame_ms = Vec::new();
                for k in 0..12 {
                    let data = FrameData {
                        globals: g,
                        instances: Arc::new(set.opaque.clone()),
                        opaque: set.opaque.len() as u32,
                        version: k as u64,
                        guard: true,
                        dt: 1.0 / 60.0,
                        msaa: true,
                        cut: false,
                    };
                    let t = Instant::now();
                    let mut enc =
                        device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                    r.bench_frame(device, queue, &mut enc, &data, (w, h), &view);
                    queue.submit([enc.finish()]);
                    let _ = device.poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    });
                    if k >= 2 {
                        frame_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                    }
                }
                frame_ms.sort_by(f64::total_cmp);
                line.push_str(&format!(
                    ", GPU frame {:.1} ms (median of 10, upload included)",
                    frame_ms[frame_ms.len() / 2]
                ));
            }
            println!("{line}");
        }
    }
    println!("\nmemory at the end: {:.0} MB resident", rss_mb());
    let _ = Vec3::ZERO;
}
