//! The performance round's measures (docs/PERFORMANCE.md), with limits
//! that hold on a busy machine with a software renderer: medians, and
//! several times what is measured here - a guard against getting much
//! worse, not a benchmark. `PERF name value unit` lines are what the
//! table is made from (`cargo test --release --test perf -- --nocapture`).
//! No GPU needed (the window's own work: no picture is made).

mod support;

use axross_files_matrix::app::App;
use axross_files_matrix::math::vec3;
use egui_kittest::Harness;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use support::{Scratch, fake_mcp, options};

/// Counts the allocations (the frames' own).
struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);

// SAFETY: hands every call to the system allocator unchanged
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: as the caller promised
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: as the caller promised
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: as the caller promised
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static A: Counting = Counting;

/// Debug builds (the tests' own profile) are slower than the release.
fn scale() -> f64 {
    if cfg!(debug_assertions) { 6.0 } else { 1.0 }
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn perf(name: &str, value: f64, unit: &str) {
    println!("PERF {name} {value:.3} {unit}");
}

fn window(s: &Scratch, files: usize) -> Harness<'static, App> {
    let mut o = options(s);
    o.synthetic = Some(files);
    o.hardware = false;
    o.fixed_lod = false;
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size(egui::vec2(1280.0, 800.0))
        .with_max_steps(8)
        .build_eframe(move |cc| App::new(cc, o));
    assert!(h.state_mut().settle(Duration::from_secs(600)));
    h
}

/// The window's own work a frame while it flies over the city (no
/// picture: the level of detail, the instances, the labels, picking, the
/// panels): medians, and the allocations a frame.
fn frame_work(files: usize) {
    let s = Scratch::new(&format!("perf-frame-{files}"));
    let mut h = window(&s, files);
    let (c, side) = {
        let city = h.state_mut().islands[0].city.as_ref().unwrap();
        (city.center(), city.side())
    };
    h.hover_at(egui::pos2(640.0, 400.0));
    let (mut work, mut pick, mut lod, mut labels, mut allocs) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for k in 0..120 {
        // round the city at the first view's height, then low over it
        let a = k as f32 / 120.0 * std::f32::consts::TAU;
        let r = side * if k < 60 { 1.0 } else { 0.3 };
        let eye = c + vec3(
            a.cos() * r,
            if k < 60 { side * 0.3 } else { 80.0 },
            a.sin() * r,
        );
        h.state_mut().look_from(eye, c);
        let before = ALLOCS.load(Ordering::Relaxed);
        let t = Instant::now();
        h.step();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if k >= 10 {
            allocs.push((ALLOCS.load(Ordering::Relaxed) - before) as f64);
            work.push(ms);
            let p = h.state_mut().parts;
            pick.push(p.pick_ms as f64);
            lod.push(p.lod_ms as f64);
            labels.push(p.labels_ms as f64);
        }
    }
    let w = median(&mut work);
    perf(&format!("frame_work_{files}"), w, "ms");
    perf(&format!("frame_pick_{files}"), median(&mut pick), "ms");
    perf(&format!("frame_lod_{files}"), median(&mut lod), "ms");
    perf(&format!("frame_labels_{files}"), median(&mut labels), "ms");
    let a = median(&mut allocs);
    perf(&format!("frame_allocations_{files}"), a, "allocations");
    // (several times what this machine measures: a guard, not a goal)
    assert!(w < 40.0 * scale(), "frame work {w:.2} ms");
    assert!(a < 20_000.0, "{a} allocations a frame");
}

#[test]
fn frame_work_20k() {
    frame_work(20_000);
}

#[test]
fn frame_work_200k() {
    frame_work(200_000);
}

#[test]
fn frame_work_2m() {
    if std::env::var_os("AFM_BIG").is_none() {
        eprintln!("SKIPPED: AFM_BIG=1 measures two million files");
        return;
    }
    frame_work(2_000_000);
}

/// From the start to the first city on the screen: cold (walked), and
/// from the walk cache.
#[test]
fn time_to_the_first_city_cold_and_cached() {
    let s = Scratch::new("perf-first");
    let first = |s: &Scratch| -> f64 {
        let mut o = options(s);
        o.synthetic = Some(200_000);
        o.hardware = false;
        o.use_cache = true;
        let t = Instant::now();
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size(egui::vec2(1000.0, 700.0))
            .with_max_steps(4)
            .build_eframe(move |cc| App::new(cc, o));
        while !h
            .state_mut()
            .islands
            .first()
            .and_then(|i| i.city.as_ref())
            .is_some_and(|c| c.layout.buildings.len() > 1000)
        {
            assert!(t.elapsed() < Duration::from_secs(300));
            h.state_mut().pump(Duration::from_millis(5));
            h.step();
        }
        let secs = t.elapsed().as_secs_f64();
        assert!(h.state_mut().settle(Duration::from_secs(300)));
        h.state_mut().save_all();
        secs
    };
    let cold = first(&s);
    let cached = first(&s);
    perf("first_city_cold_200k", cold, "s");
    perf("first_city_cached_200k", cached, "s");
    assert!(cold < 30.0 * scale() && cached < 30.0 * scale());
}

/// A local tree walked (entries a second), and a remote folder through the
/// fake server (its 80 ms a page included).
#[test]
fn walk_throughput() {
    use axross_files_matrix::walk::{Limits, Source, walk_blocking};
    let s = Scratch::new("perf-walk");
    let root = s.0.join("tree");
    let mut made = 0;
    for d in 0..100 {
        let dir = root.join(format!("d{d}/e{}", d % 7));
        std::fs::create_dir_all(&dir).unwrap();
        for f in 0..200 {
            std::fs::write(dir.join(format!("f{f}.txt")), b"x").unwrap();
            made += 1;
        }
    }
    let t = Instant::now();
    let (tree, status) = walk_blocking(
        Source::Local {
            root: root.clone(),
            skip: Vec::new(),
            auto_depth: None,
        },
        Limits::default(),
        std::ffi::OsStr::new("tree"),
    );
    let secs = t.elapsed().as_secs_f64();
    assert!(status.error.is_none());
    assert!(tree.len() > made);
    perf(
        "walk_local_entries_per_second",
        tree.len() as f64 / secs,
        "entries/s",
    );
    // remote: one folder of 60 000 entries in pages of 6000
    let (client, _) = fake_mcp::start_with_big_folder(60_000, 6000);
    let t = Instant::now();
    let (tree, _) = walk_blocking(
        Source::Mcp {
            client,
            backend: "profile:big".into(),
            root: "/".into(),
            lazy: false,
        },
        Limits::default(),
        std::ffi::OsStr::new("big"),
    );
    let secs = t.elapsed().as_secs_f64();
    perf(
        "walk_mcp_entries_per_second",
        tree.len() as f64 / secs,
        "entries/s",
    );
    assert!(tree.len() >= 60_000, "{}", tree.len());
}

/// The walk cache of a 200 000-file tree: written, read back, its size.
#[test]
fn the_walk_cache_of_200k() {
    use axross_files_matrix::walk::{Limits, Source, walk_blocking};
    let (tree, _) = walk_blocking(
        Source::Synthetic {
            files: 200_000,
            seed: 7,
        },
        Limits::default(),
        std::ffi::OsStr::new("synthetic"),
    );
    let t = Instant::now();
    let bytes = axross_files_matrix::cache::encode(&tree, "perf", 0);
    let enc = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let back = axross_files_matrix::cache::decode(&bytes).expect("reads back");
    let dec = t.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(back.1.len(), tree.len());
    perf("cache_write_200k", enc, "ms");
    perf("cache_read_200k", dec, "ms");
    perf("cache_size_200k", bytes.len() as f64 / 1_048_576.0, "MiB");
    assert!(enc < 3000.0 * scale() && dec < 3000.0 * scale());
}
