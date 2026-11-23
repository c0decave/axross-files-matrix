//! The window in a test harness: egui_kittest with wgpu (lavapipe on
//! Linux CI), the program's own App, a made-up machine, its own settings
//! and cache folders.
#![allow(dead_code)]

pub mod fake_mcp;
pub mod m11;

use axross_files_matrix::app::{App, StartOptions};
use egui_kittest::Harness;
use std::path::PathBuf;
use std::time::Duration;

/// A folder of a test's own (removed at the end).
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("afm-it-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Options for a test: nothing from this machine (no mounts, no MCP, no
/// updates), the made-up machine, a still clock.
pub fn options(scratch: &Scratch) -> StartOptions {
    StartOptions {
        mounts: false,
        mcp: false,
        hardware: true,
        machine: Some(axross_files_matrix::hw::demo_machine()),
        config_dir: Some(scratch.0.join("config")),
        cache_dir: Some(scratch.0.join("cache")),
        use_cache: false,
        updates: false,
        fixed_lod: true,
        flash_guard: std::env::var_os("AFM_NO_GUARD").is_none(),
        clock: Some(12.5),
        step: Some(1.0 / 30.0),
        // (no axross of this machine's: the tests give their own)
        axross: Some(PathBuf::from("/nonexistent/axross")),
        ..StartOptions::default()
    }
}

/// Whether a GPU (or lavapipe) is there to render with; without one the
/// picture tests say so and stop (CI on Linux requires it).
pub fn gpu_available() -> bool {
    let instance = eframe::wgpu::Instance::new(
        eframe::wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
    );
    let found = pollster::block_on(
        instance.request_adapter(&eframe::wgpu::RequestAdapterOptions::default()),
    )
    .is_ok();
    if !found {
        let msg = "SKIPPED: no GPU adapter - the pictures were not rendered";
        assert!(std::env::var_os("AFM_REQUIRE_GPU").is_none(), "{msg}");
        eprintln!("{msg}");
    }
    found
}

/// The window at `size`, rendered by wgpu.
pub fn harness(opts: StartOptions, size: egui::Vec2) -> Harness<'static, App> {
    Harness::builder()
        .with_size(size)
        .with_max_steps(64)
        .wgpu()
        .build_eframe(move |cc| App::new(cc, opts))
}

/// Lets the walks and layouts come in, then draws a few frames.
pub fn settle(h: &mut Harness<'static, App>) {
    assert!(
        h.state_mut().settle(Duration::from_secs(120)),
        "the walks and layouts did not finish"
    );
    for _ in 0..4 {
        h.step();
    }
}

/// Saves a frame as PNG where AFM_SHOTS points (to look at them).
pub fn shot(h: &mut Harness<'static, App>, name: &str) -> image::RgbaImage {
    let img = h.render().expect("rendered");
    if let Some(dir) = std::env::var_os("AFM_SHOTS") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        img.save(dir.join(format!("{name}.png"))).unwrap();
    }
    img
}
