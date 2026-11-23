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
