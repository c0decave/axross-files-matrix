//! The city's frames held to an independent flash meter (tests/support/m11.rs,
//! from axross-amp: WCAG 2.3.1 / 2.3.2 - at most six transitions in any
//! second in any field of a third of the picture): a boosted flight low
//! over a dense city, a fast orbit, teleports pressed every frame, the
//! search's pillars and the chosen building's highlight - and a picture
//! that flashes on purpose, which the guard must veil (and which the meter
//! must catch without the guard).

mod support;

use axross_files_matrix::app::App;
use axross_files_matrix::math::vec3;
use egui_kittest::Harness;
use support::m11::{M11, Reading};
use support::{Scratch, harness, options, settle};

const W: f32 = 480.0;
const H: f32 = 300.0;
const FPS: f32 = 30.0;

fn measure(
    h: &mut Harness<'static, App>,
    frames: usize,
    mut each: impl FnMut(&mut App, usize),
) -> Reading {
    let first = h.render().expect("rendered");
    let mut m = M11::new(first.width() as usize, first.height() as usize, FPS);
    for k in 0..frames {
        each(h.state_mut(), k);
        h.step();
        let img = h.render().expect("rendered");
        m.push_rgba(&img);
    }
    m.reading()
}

fn city(name: &str, files: usize, guard: bool) -> (Scratch, Harness<'static, App>) {
    let s = Scratch::new(name);
    let mut opts = options(&s);
    opts.synthetic = Some(files);
    opts.flash_guard = guard;
    let mut h = harness(opts, egui::vec2(W, H));
    settle(&mut h);
    (s, h)
}

#[test]
fn a_boosted_flight_over_the_roofs_stays_within_the_rule() {
    if !support::gpu_available() {
        return;
    }
    let (_s, mut h) = city("flash-flight", 40_000, true);
    let c = h
        .state_mut()
        .islands
        .iter()
        .find_map(|i| {
            i.city
                .as_ref()
                .filter(|c| c.ring != axross_files_matrix::city::Ring::Hardware)
                .map(|c| (c.center(), c.side()))
        })
        .unwrap();
    let (center, side) = c;
    let start = center + vec3(-side * 0.4, 12.0, -side * 0.1);
    h.state_mut()
        .camera
        .set_mode(axross_files_matrix::camera::Mode::Fly);
    h.state_mut()
        .look_from(start, start + vec3(1.0, -0.15, 0.1));
    // five times the flight speed: 125 m/s, 4 m a frame
    let r = measure(&mut h, 90, |app, _| {
        let f = app.camera.forward();
        app.camera.pos += vec3(f.x, 0.0, f.z).normalized() * (125.0 / FPS);
    });
    eprintln!("boosted flight: {r:?}");
    assert!(!r.fails(), "{r:?}");
}

#[test]
fn a_fast_orbit_stays_within_the_rule() {
    if !support::gpu_available() {
        return;
    }
    let (_s, mut h) = city("flash-orbit", 20_000, true);
    h.state_mut().overview(false);
    let r = measure(&mut h, 90, |app, _| {
        app.camera.yaw += 0.12;
        let t = app.camera.target;
        let d = app.camera.distance;
        app.camera.pos = t - app.camera.forward() * d;
    });
    eprintln!("fast orbit: {r:?}");
    assert!(!r.fails(), "{r:?}");
}

#[test]
fn teleports_pressed_every_frame_stay_within_the_rule() {
    if !support::gpu_available() {
        return;
    }
    let (_s, mut h) = city("flash-teleport", 20_000, true);
    let targets: Vec<(usize, u32)> = {
        let app = h.state_mut();
        let mut t = Vec::new();
        for (i, isl) in app.islands.iter().enumerate() {
            t.push((i, 0));
            for &c in isl.tree.root().children.iter().take(6) {
                t.push((i, c));
            }
        }
        t
    };
    let r = measure(&mut h, 90, |app, k| {
        let (i, n) = targets[k % targets.len()];
        app.teleport_to(i, n);
    });
    eprintln!("teleport storm: {r:?}");
    assert!(!r.fails(), "{r:?}");
    // (with room to spare: the strict share well under its limit of 0.25 -
    // a request waiting its turn changes nothing on screen)
    assert!(r.p[0] < 0.1 && r.p[1] < 0.1, "{r:?}");
}

#[test]
fn search_pillars_and_the_highlight_stay_within_the_rule() {
    if !support::gpu_available() {
        return;
    }
    let (_s, mut h) = city("flash-search", 20_000, true);
    h.state_mut().run_search("*.iso");
    h.state_mut().finish_search();
    h.state_mut().overview(false);
    let hit = h
        .state_mut()
        .search
        .as_ref()
        .and_then(|s| s.hits.first().copied());
    if let Some((i, n)) = hit {
        h.state_mut().select(i, n);
    }
    let r = measure(&mut h, 60, |_, _| {});
    eprintln!("search and highlight: {r:?}");
    assert!(!r.fails(), "{r:?}");
}

#[test]
fn a_flashing_picture_is_caught_and_veiled() {
    if !support::gpu_available() {
        return;
    }
    // the signal: the whole picture light and dark at 7.5 Hz (15 transitions
    // a second)
    let strobe = |app: &mut App, k: usize| {
        app.test_signal = if (k / 2).is_multiple_of(2) { 0.0 } else { 1.0 }
    };
    let (_s, mut h) = city("flash-off", 2000, false);
    h.state_mut().overview(false);
    let without = measure(&mut h, 60, strobe);
    eprintln!("strobe without the guard: {without:?}");
    assert!(
        without.fails(),
        "the meter must see the strobe: {without:?}"
    );
    let (_s2, mut h2) = city("flash-on", 2000, true);
    h2.state_mut().overview(false);
    let with = measure(&mut h2, 60, strobe);
    eprintln!("strobe with the guard: {with:?}");
    assert!(!with.fails(), "the guard must veil it: {with:?}");
}

/// What a scenario of ordinary use does each frame (`k` from 0).
type Act = fn(&mut Harness<'static, App>, usize, f32);

/// Frames in which the guard veiled any part of the picture while `act`
/// ran for `seconds` (after two calm seconds), in a window of 1280 x 800
/// at `fps` (every frame rendered, as the guard sees them).
fn veiled(files: usize, fps: f32, name: &str, seconds: f32, setup: Act, act: Act) -> usize {
    let s = Scratch::new(&format!("veil-{name}-{files}-{fps}"));
    std::fs::create_dir_all(s.0.join("config")).unwrap();
    std::fs::write(s.0.join("config/settings.json"), br#"{"hint_seen": true}"#).unwrap();
    let mut opts = options(&s);
    opts.synthetic = Some(files);
    opts.hardware = false;
    opts.fixed_lod = false;
    opts.flash_guard = true;
    opts.step = Some(1.0 / fps);
    let mut h = harness(opts, egui::vec2(1280.0, 800.0));
    settle(&mut h);
    setup(&mut h, 0, fps);
    for _ in 0..(2.0 * fps) as usize {
        h.step();
        let _ = h.render();
    }
    let calm = !h.state_mut().guard_veiling();
    let mut v = 0;
    for k in 0..(seconds * fps) as usize {
        act(&mut h, k, fps);
        h.step();
        let every = std::env::var("AFM_VEIL_EVERY")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(6);
        if std::env::var_os("AFM_VEIL_SHOTS").is_some() && k % every == 0 {
            let _ = support::shot(&mut h, &format!("veil-{name}-{files}-{k:03}"));
        } else {
            let _ = h.render();
        }
        if h.state_mut().guard_veiling() {
            v += 1;
        }
        if std::env::var_os("AFM_VEIL_SHOTS").is_some() && k % 10 == 0 {
            let app = h.state_mut();
            eprintln!(
                "{name} {k}: motion {:.2} pos {:?} speed {:.1}",
                app.motion,
                app.camera.pos,
                app.camera.vel.length()
            );
        }
    }
    eprintln!(
        "{name} at {files} files, {fps} fps: calm before {calm}; veiled in {v} of {} frames",
        (seconds * fps) as usize
    );
    v
}

fn the_city_of(h: &mut Harness<'static, App>) -> (axross_files_matrix::math::Vec3, f32) {
    h.state_mut()
        .islands
        .iter()
        .find_map(|i| {
            i.city
                .as_ref()
                .filter(|c| c.ring != axross_files_matrix::city::Ring::Hardware)
        })
        .map(|c| (c.center(), c.side()))
        .unwrap()
}

fn high_over_the_city(h: &mut Harness<'static, App>, _: usize, _: f32) {
    let (c, s) = the_city_of(h);
    let app = h.state_mut();
    app.camera.set_mode(axross_files_matrix::camera::Mode::Fly);
    app.look_from(
        c + vec3(-s * 0.45, 150.0, -s * 0.2),
        c + vec3(s * 0.5, 0.0, s * 0.2),
    );
}

fn in_a_street(h: &mut Harness<'static, App>, _: usize, _: f32) {
    let (c, s) = the_city_of(h);
    let app = h.state_mut();
    app.camera.set_mode(axross_files_matrix::camera::Mode::Fly);
    app.look_from(
        c + vec3(s * 0.11, 6.0, -s * 0.07),
        c + vec3(s * 0.4, 6.0, s * 0.2),
    );
}

fn first_view(_: &mut Harness<'static, App>, _: usize, _: f32) {}

fn with_hits(h: &mut Harness<'static, App>, _: usize, _: f32) {
    h.state_mut().run_search("*.iso");
    h.state_mut().finish_search();
}

fn fly(h: &mut Harness<'static, App>, k: usize, _: f32) {
    if k == 0 {
        h.key_down(egui::Key::W);
    }
}

fn boost(h: &mut Harness<'static, App>, k: usize, _: f32) {
    if k == 0 {
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::SHIFT));
        h.key_down(egui::Key::W);
    }
}

fn next_hit(h: &mut Harness<'static, App>, k: usize, _: f32) {
    if k == 0 {
        h.state_mut().next_hit(false);
    }
}

fn teleports(h: &mut Harness<'static, App>, k: usize, fps: f32) {
    // a teleport every 1.5 s to the city's districts
    if k.is_multiple_of((1.5 * fps) as usize) {
        let app = h.state_mut();
        let i = app.islands.iter().position(|x| x.hw.is_none()).unwrap();
        let kids: Vec<u32> = app.islands[i]
            .tree
            .root()
            .children
            .iter()
            .copied()
            .take(8)
            .collect();
        let n = kids[(k / (1.5 * fps) as usize) % kids.len()];
        app.teleport_to(i, n);
    }
}

fn orbit(h: &mut Harness<'static, App>, _: usize, fps: f32) {
    // an orbit drag of 90 degrees a second
    let app = h.state_mut();
    app.auto_frame = false;
    app.camera.mode = axross_files_matrix::camera::Mode::Orbit;
    app.camera.yaw += 90f32.to_radians() / fps;
    let (t, d) = (app.camera.target, app.camera.distance);
    app.camera.pos =
        t - axross_files_matrix::camera::forward_of(app.camera.yaw, app.camera.pitch) * d;
}

fn home_from_the_map(h: &mut Harness<'static, App>, k: usize, _: f32) {
    if k == 0 {
        h.state_mut().overview(false);
    }
    if k == 2 {
        h.state_mut().home();
    }
}

const SCENARIOS: [(&str, f32, Act, Act); 7] = [
    ("fly", 4.0, high_over_the_city, fly),
    ("boost", 3.0, high_over_the_city, boost),
    ("street", 4.0, in_a_street, fly),
    ("next-hit", 4.0, with_hits, next_hit),
    ("teleports", 4.5, first_view, teleports),
    ("orbit", 4.0, first_view, orbit),
    ("home", 4.0, first_view, home_from_the_map),
];

fn ordinary_use_is_never_veiled(files: usize, fps: f32) {
    if !support::gpu_available() {
        return;
    }
    let only = std::env::var("AFM_VEIL_ONLY").ok();
    let mut bad = Vec::new();
    for (name, seconds, setup, act) in SCENARIOS {
        if only
            .as_deref()
            .is_some_and(|o| !o.split(',').any(|x| x == name))
        {
            continue;
        }
        let v = veiled(files, fps, name, seconds, setup, act);
        if v > 0 {
            bad.push(format!("{name}: {v} frames"));
        }
    }
    assert!(
        bad.is_empty(),
        "{files} files at {fps} fps: veiled in {bad:?}"
    );
}

#[test]
fn ordinary_use_is_never_veiled_20k_30fps() {
    ordinary_use_is_never_veiled(20_000, 30.0);
}

#[test]
fn ordinary_use_is_never_veiled_200k_30fps() {
    ordinary_use_is_never_veiled(200_000, 30.0);
}

/// The slower ones (AFM_BIG=1): 60 frames a second, and two million files.
fn big() -> bool {
    let on = std::env::var_os("AFM_BIG").is_some();
    if !on {
        eprintln!("SKIPPED: AFM_BIG=1 runs the 60 fps and 2M flights");
    }
    on
}

#[test]
fn ordinary_use_is_never_veiled_20k_60fps() {
    if big() {
        ordinary_use_is_never_veiled(20_000, 60.0);
    }
}

#[test]
fn ordinary_use_is_never_veiled_200k_60fps() {
    if big() {
        ordinary_use_is_never_veiled(200_000, 60.0);
    }
}

#[test]
fn ordinary_use_is_never_veiled_2m_30fps() {
    if big() {
        ordinary_use_is_never_veiled(2_000_000, 30.0);
    }
}

#[test]
fn ordinary_use_is_never_veiled_2m_60fps() {
    if big() {
        ordinary_use_is_never_veiled(2_000_000, 60.0);
    }
}
