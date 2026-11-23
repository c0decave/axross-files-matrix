//! The city as pictures: rendered by wgpu (lavapipe on CI) through the
//! window itself, and checked by what they show - green on black, edges
//! that glow, the search's pillars, the inspect panel with its preview, the
//! machine's island. AFM_SHOTS=DIR saves every picture there (the README's
//! pictures are made so).

mod support;

use axross_files_matrix::app::App;
use axross_files_matrix::camera::Mode;
use axross_files_matrix::city::Ring;
use axross_files_matrix::math::{Vec3, vec3};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use support::{Scratch, harness, options, settle, shot};

const SIZE: egui::Vec2 = egui::vec2(1280.0, 800.0);

/// What a picture shows, in numbers.
#[derive(Debug)]
struct Look {
    /// Mean of each channel (0..255) over the 3D view's middle.
    mean: [f32; 3],
    /// Share of pixels clearly green (g well over r and b, bright enough).
    green: f32,
    /// Share of bright pixels (any channel over 200).
    bright: f32,
    /// Share of pixels near the highlight's white-green.
    white: f32,
}

fn look(img: &image::RgbaImage, area: (f32, f32, f32, f32)) -> Look {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (x0, y0, x1, y1) = (
        (area.0 * w) as u32,
        (area.1 * h) as u32,
        (area.2 * w) as u32,
        (area.3 * h) as u32,
    );
    let mut sum = [0f64; 3];
    let (mut green, mut bright, mut white, mut n) = (0u32, 0u32, 0u32, 0u32);
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.get_pixel(x, y).0;
            let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
            sum[0] += r as f64;
            sum[1] += g as f64;
            sum[2] += b as f64;
            if g > 60.0 && g > r * 1.3 && g > b * 1.15 {
                green += 1;
            }
            if r.max(g).max(b) > 200.0 {
                bright += 1;
            }
            if r > 170.0 && g > 220.0 && b > 170.0 {
                white += 1;
            }
            n += 1;
        }
    }
    let n = n.max(1) as f32;
    Look {
        mean: [
            (sum[0] / n as f64) as f32,
            (sum[1] / n as f64) as f32,
            (sum[2] / n as f64) as f32,
        ],
        green: green as f32 / n,
        bright: bright as f32 / n,
        white: white as f32 / n,
    }
}

/// The Matrix look: dark, green over red and blue, glowing edges.
fn assert_matrix(l: &Look, what: &str) {
    assert!(
        l.mean[1] > l.mean[0] && l.mean[1] > l.mean[2],
        "{what}: green leads {l:?}"
    );
    assert!(l.mean[1] < 120.0, "{what}: dark {l:?}");
    assert!(l.green > 0.01, "{what}: green edges and windows {l:?}");
}

fn synthetic(name: &str, files: usize) -> (Scratch, Harness<'static, App>) {
    let s = Scratch::new(name);
    let mut opts = options(&s);
    opts.synthetic = Some(files);
    let mut h = harness(opts, SIZE);
    settle(&mut h);
    (s, h)
}

fn the_city(app: &mut App) -> (Vec3, f32) {
    app.islands
        .iter()
        .filter_map(|i| i.city.as_ref())
        .find(|c| c.ring != Ring::Hardware)
        .map(|c| (c.center(), c.side()))
        .expect("a city")
}

fn frames(h: &mut Harness<'static, App>, n: usize) {
    for _ in 0..n {
        h.step();
    }
}

#[test]
fn a_small_city_beside_the_machine() {
    if !support::gpu_available() {
        return;
    }
    let (_s, mut h) = synthetic("small", 3000);
    h.state_mut().overview(false);
    frames(&mut h, 3);
    let img = shot(&mut h, "city-small");
    let l = look(&img, (0.1, 0.15, 0.9, 0.9));
    eprintln!("small city: {l:?}");
    assert_matrix(&l, "small city");
    // the labels: the islands' titles were laid out
    assert!(h.state_mut().islands.len() == 2);
}

#[test]
fn a_city_of_200000_files() {
    if !support::gpu_available() {
        return;
    }
    let (_s, mut h) = synthetic("200k", 200_000);
    let (c, side) = the_city(h.state_mut());
    // from aside and above, the whole island
    h.state_mut().look_from(
        c + vec3(side * 0.55, side * 0.32, side * 0.62),
        c + vec3(-side * 0.05, 0.0, 0.0),
    );
    frames(&mut h, 3);
    let img = shot(&mut h, "city-200k");
    let l = look(&img, (0.05, 0.1, 0.95, 0.9));
    eprintln!(
        "200k overview: {l:?}, {} boxes",
        h.state_mut().last_frame_instances
    );
    assert_matrix(&l, "200k");
    // inside the city: towers receding under the rain (the Matrix look),
    // a street, the skyline
    h.state_mut().camera.set_mode(Mode::Fly);
    for (name, eye, dir) in [
        (
            "matrix",
            c + vec3(side * 0.05, 140.0, side * 0.45),
            vec3(-0.2, -0.22, -1.0),
        ),
        (
            "street",
            c + vec3(side * 0.32, 60.0, side * 0.36),
            vec3(-0.7, -0.1, -0.7),
        ),
        (
            "skyline",
            c + vec3(-side * 0.38, 35.0, side * 0.1),
            vec3(0.9, -0.05, -0.3),
        ),
    ] {
        h.state_mut().look_from(eye, eye + dir);
        frames(&mut h, 3);
        let img = shot(&mut h, name);
        let l = look(&img, (0.05, 0.1, 0.95, 0.9));
        eprintln!(
            "{name}: {l:?}, {} boxes",
            h.state_mut().last_frame_instances
        );
        assert_matrix(&l, name);
        assert!(
            l.bright > 0.002,
            "{name}: lit edges and the rain's heads: {l:?}"
        );
    }
}

#[test]
fn search_hits_stand_as_pillars_and_the_camera_flies_there() {
    if !support::gpu_available() {
        return;
    }
    let (_s, mut h) = synthetic("search", 30_000);
    h.state_mut().run_search("*.iso");
    h.state_mut().finish_search();
    let hits = h.state_mut().search.as_ref().map_or(0, |s| s.hits.len());
    assert!(hits > 50, "{hits} hits");
    let (c, side) = the_city(h.state_mut());
    h.state_mut()
        .look_from(c + vec3(side * 0.6, side * 0.45, side * 0.7), c);
    frames(&mut h, 2);
    let img = shot(&mut h, "search");
    let pillars = look(&img, (0.05, 0.08, 0.95, 0.6));
    eprintln!("pillars: {pillars:?}");
    assert!(pillars.white > 0.0005, "the pillars' light: {pillars:?}");
    // N: the camera flies to the first hit and chooses it
    h.state_mut().next_hit(false);
    for _ in 0..120 {
        h.step();
        if !h.state_mut().camera.flying() {
            break;
        }
    }
    assert!(!h.state_mut().camera.flying());
    let (i, n) = h.state_mut().selected.expect("the hit is chosen");
    let name = h.state_mut().islands[i]
        .tree
        .get(n)
        .unwrap()
        .name
        .to_string_lossy()
        .into_owned();
    assert!(name.ends_with(".iso"), "{name}");
    assert!(
        h.state_mut()
            .wait_preview(std::time::Duration::from_secs(10))
    );
    frames(&mut h, 3);
    let img = shot(&mut h, "search-hit");
    let l = look(&img, (0.05, 0.1, 0.7, 0.9));
    eprintln!("at the hit: {l:?}");
    assert!(l.white > 0.0002, "the chosen building's highlight: {l:?}");
}

#[test]
fn the_inspect_panel_previews_text_and_images() {
    if !support::gpu_available() {
        return;
    }
    let s = Scratch::new("preview");
    let data = s.0.join("demo-project");
    std::fs::create_dir_all(data.join("src")).unwrap();
    std::fs::create_dir_all(data.join("assets")).unwrap();
    let code = "//! The city's main loop.\n\nfn main() {\n    let city = City::walk(\"/\");\n    for island in city.islands() {\n        println!(\"{} files\", island.count);\n    }\n}\n";
    std::fs::write(data.join("src/main.rs"), code).unwrap();
    for k in 0..30 {
        std::fs::write(data.join(format!("src/module{k}.rs")), code.repeat(k + 1)).unwrap();
    }
    let img = image::RgbaImage::from_fn(320, 200, |x, y| {
        image::Rgba([0, ((x ^ y) & 0xff) as u8, (y / 2) as u8, 255])
    });
    img.save(data.join("assets/pattern.png")).unwrap();
    std::fs::write(
        data.join("assets/blob.bin"),
        (0..4096u32).map(|v| (v * 7) as u8).collect::<Vec<u8>>(),
    )
    .unwrap();
    let mut opts = options(&s);
    opts.paths = vec![data.clone()];
    let mut h = harness(opts, SIZE);
    settle(&mut h);
    // choose src/main.rs
    let (i, n) = {
        let app = h.state_mut();
        let i = app.islands.iter().position(|isl| isl.hw.is_none()).unwrap();
        let n = app.islands[i]
            .tree
            .nodes
            .iter()
            .position(|x| &*x.name == std::ffi::OsStr::new("main.rs"))
            .unwrap();
        (i, n as u32)
    };
    h.state_mut().fly_to(i, n);
    for _ in 0..120 {
        h.step();
        if !h.state_mut().camera.flying() {
            break;
        }
    }
    assert!(
        h.state_mut()
            .wait_preview(std::time::Duration::from_secs(10))
    );
    frames(&mut h, 3);
    let img = shot(&mut h, "preview");
    // the panel (right) is lit with text; the code is in the window's text
    let panel = look(&img, (0.75, 0.1, 0.99, 0.9));
    eprintln!("panel: {panel:?}");
    assert!(panel.green > 0.005, "the panel's text: {panel:?}");
    h.get_by_label_contains("City::walk");
    // the picture
    let png = h.state_mut().islands[i]
        .tree
        .nodes
        .iter()
        .position(|x| &*x.name == std::ffi::OsStr::new("pattern.png"))
        .unwrap() as u32;
    h.state_mut().select(i, png);
    assert!(
        h.state_mut()
            .wait_preview(std::time::Duration::from_secs(10))
    );
    frames(&mut h, 3);
    let img = shot(&mut h, "preview-image");
    h.get_by_label_contains("320 × 200 pixels");
    let _ = img;
    // bytes: a hex dump
    let bin = h.state_mut().islands[i]
        .tree
        .nodes
        .iter()
        .position(|x| &*x.name == std::ffi::OsStr::new("blob.bin"))
        .unwrap() as u32;
    h.state_mut().select(i, bin);
    assert!(
        h.state_mut()
            .wait_preview(std::time::Duration::from_secs(10))
    );
    frames(&mut h, 3);
    h.get_by_label_contains("00000000  00 07 0e 15");
}

#[test]
fn the_machine_island_up_close() {
    if !support::gpu_available() {
        return;
    }
    let (_s, mut h) = synthetic("hardware", 2000);
    let c = h
        .state_mut()
        .islands
        .iter()
        .find(|i| i.hw.is_some())
        .and_then(|i| i.city.as_ref())
        .map(|c| (c.center(), c.side()))
        .unwrap();
    let (center, side) = c;
    h.state_mut().look_from(
        center + vec3(side * 0.15, side * 0.42, side * 0.62),
        center + vec3(0.0, 0.0, -side * 0.05),
    );
    // a busy processor: its towers lit
    let loads: Vec<f32> = (0..16).map(|k| (k as f32 * 0.37).sin().abs()).collect();
    for _ in 0..6 {
        h.state_mut().apply_load(&loads, 0.4);
    }
    frames(&mut h, 3);
    let img = shot(&mut h, "hardware");
    let l = look(&img, (0.1, 0.1, 0.9, 0.9));
    eprintln!("hardware: {l:?}");
    assert_matrix(&l, "hardware");
    // the processor's details in the inspect panel
    let (i, cpu) = {
        let app = h.state_mut();
        let i = app.islands.iter().position(|x| x.hw.is_some()).unwrap();
        let n = app.islands[i]
            .tree
            .nodes
            .iter()
            .position(|x| &*x.name == std::ffi::OsStr::new("CPU"))
            .unwrap();
        (i, n as u32)
    };
    h.state_mut().select(i, cpu);
    frames(&mut h, 2);
    h.get_by_label_contains("Example CPU 16-core");
}

#[test]
fn the_teleport_menu() {
    if !support::gpu_available() {
        return;
    }
    let (_s, mut h) = synthetic("teleport", 20_000);
    h.state_mut().overview(false);
    h.state_mut().run_search("backups");
    h.state_mut().finish_search();
    h.state_mut().save_spot(1);
    h.state_mut().teleport_open = true;
    frames(&mut h, 3);
    shot(&mut h, "teleport");
    h.get_by_label("Spots (1-9 jump, Ctrl+1-9 save)");
    // a district's "go": a jump, with the warp
    let before = h.state_mut().camera.pos;
    let go = h.get_all_by_label("go").nth(1).expect("a district's go");
    go.click();
    frames(&mut h, 2);
    let after = h.state_mut().camera.pos;
    assert!((after - before).length() > 1.0, "{before:?} {after:?}");
}

#[test]
fn small_windows_keep_their_bars_and_panels() {
    if !support::gpu_available() {
        return;
    }
    for (name, size) in [
        ("small-640x420", egui::vec2(640.0, 420.0)),
        ("small-800x600", egui::vec2(800.0, 600.0)),
    ] {
        let s = Scratch::new(name);
        let mut opts = options(&s);
        opts.synthetic = Some(20_000);
        let mut h = harness(opts, size);
        settle(&mut h);
        h.state_mut().overview(false);
        h.state_mut().run_search("*.iso");
        h.state_mut().finish_search();
        // a file chosen: the inspect panel beside the view
        let (i, n) = h.state_mut().search.as_ref().unwrap().hits[0];
        h.state_mut().select(i, n);
        frames(&mut h, 3);
        let img = shot(&mut h, name);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        for label in [" READ-ONLY ", "Menu", "Fly", "Orbit"] {
            let r = h.get_by_label(label).rect();
            assert!(screen.contains_rect(r), "{name}: {label} at {r:?}");
        }
        let l = look(&img, (0.02, 0.15, 0.5, 0.85));
        eprintln!("{name}: {l:?}");
        assert_matrix(&l, name);
        // the settings, scrolled to their end
        h.state_mut().settings_open = true;
        frames(&mut h, 2);
        h.get_by_label("Check for updates…").scroll_to_me();
        h.run_steps(6);
        let r = h.get_by_label("Check for updates…").rect();
        assert!(screen.contains_rect(r), "{name}: the settings' end {r:?}");
        shot(&mut h, &format!("{name}-settings"));
    }
}

#[test]
fn the_teleport_menu_with_the_systems_of_axross() {
    if !support::gpu_available() {
        return;
    }
    let s = Scratch::new("teleport-systems");
    let (client, _seen) = support::fake_mcp::start();
    let mut opts = options(&s);
    opts.synthetic = Some(3000);
    opts.mcp = true;
    opts.mcp_client = Some(client);
    let mut h = harness(opts, SIZE);
    let wait = |h: &mut Harness<'static, App>, cond: &dyn Fn(&App) -> bool| {
        let start = std::time::Instant::now();
        while !cond(h.state_mut()) {
            assert!(start.elapsed() < std::time::Duration::from_secs(60));
            h.state_mut().pump(std::time::Duration::from_millis(5));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    wait(&mut h, &|a| !a.system_profiles().is_empty());
    use axross_files_matrix::systems::SysState;
    for n in [
        "sftp-box", "s3-box", "nfs-box", "adb-box", "imap-box", "git-box", "broken",
    ] {
        h.state_mut().teleport_system(n, Vec::new());
    }
    wait(&mut h, &|a| {
        [
            "sftp-box", "s3-box", "nfs-box", "adb-box", "imap-box", "git-box",
        ]
        .iter()
        .all(|n| a.system_state(n) == SysState::Open)
            && matches!(a.system_state("broken"), SysState::Failed(_))
    });
    assert!(h.state_mut().settle(std::time::Duration::from_secs(60)));
    // one still connecting while the picture is taken
    h.state_mut().teleport_system("slow", Vec::new());
    for _ in 0..10 {
        h.state_mut().pump(std::time::Duration::from_millis(5));
    }
    h.state_mut().pending_fly = None;
    h.state_mut().selected = None;
    h.state_mut().overview(false);
    h.state_mut().teleport_filter = String::new();
    h.state_mut().teleport_open = true;
    frames(&mut h, 4);
    shot(&mut h, "teleport-systems");
    h.get_by_label("Systems (axross's connection manager)");
    h.get_by_label_contains("connecting…");
    h.get_by_label_contains("needs unlocking in axross");
    h.get_by_label_contains("failed: cannot open");
}
