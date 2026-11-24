//! The window driven by keys and clicks (no GPU needed: the 3D view is left
//! black without one): the camera's modes, search, the teleport menu and
//! the saved spots, the read-only badge, a settings file that is no JSON.

mod support;

use axross_files_matrix::app::App;
use axross_files_matrix::camera::Mode;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use std::time::Duration;
use support::{Scratch, options};

fn window(s: &Scratch, files: usize) -> Harness<'static, App> {
    let mut opts = options(s);
    opts.synthetic = Some(files);
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size(egui::vec2(1100.0, 700.0))
        .with_max_steps(32)
        .build_eframe(move |cc| App::new(cc, opts));
    assert!(h.state_mut().settle(Duration::from_secs(60)));
    h.run_steps(3);
    h
}

#[test]
fn keys_switch_the_camera_and_open_the_menus() {
    let s = Scratch::new("keys");
    let mut h = window(&s, 2000);
    assert_eq!(h.state_mut().camera.mode, Mode::Orbit);
    h.key_press(Key::F);
    h.run_steps(2);
    assert_eq!(h.state_mut().camera.mode, Mode::Fly);
    h.key_press(Key::T);
    h.run_steps(2);
    assert!(h.state_mut().teleport_open);
    h.get_by_label("Islands");
    h.key_press(Key::H);
    h.run_steps(2);
    h.get_by_label_contains("capture the mouse");
    // the badge and the write mode, visibly off
    h.get_by_label_contains("READ-ONLY");
    let write = h.get_by_label("write mode");
    assert!(write.accesskit_node().is_disabled());
}

#[test]
fn saved_spots_jump_back() {
    let s = Scratch::new("spots");
    let mut h = window(&s, 2000);
    h.state_mut().overview(false);
    h.run_steps(2);
    let here = h.state_mut().camera.pos;
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num3);
    h.run_steps(2);
    assert!(h.state_mut().settings.bookmarks[2].is_some());
    // elsewhere, then 3: back (a teleport)
    h.state_mut().look_from(
        here + axross_files_matrix::math::vec3(500.0, 50.0, 0.0),
        here,
    );
    h.run_steps(2);
    h.key_press(Key::Num3);
    h.run_steps(30);
    let back = h.state_mut().camera.pos;
    assert!((back - here).length() < 1.0, "{back:?} vs {here:?}");
    // saved in the settings file
    let text = std::fs::read_to_string(s.0.join("config/settings.json")).unwrap();
    assert!(text.contains("bookmarks"));
}

#[test]
fn typing_a_search_finds_and_next_flies() {
    let s = Scratch::new("search-keys");
    let mut h = window(&s, 5000);
    h.key_press(Key::Slash);
    h.run_steps(2);
    let field = h.get_by_role(egui::accesskit::Role::TextInput);
    field.type_text("*.flac");
    h.run_steps(4);
    h.state_mut().finish_search();
    h.run_steps(2);
    let hits = h.state_mut().search.as_ref().unwrap().hits.len();
    assert!(hits > 20, "{hits}");
    h.get_by_label_contains(&format!("{hits} hits"));
    // N (out of the field): fly to the first hit, chosen
    h.state_mut().next_hit(false);
    h.run_steps(5);
    assert!(h.state_mut().selected.is_some());
    assert!(h.state_mut().camera.flying());
}

#[test]
fn a_settings_file_that_is_no_json_is_set_aside() {
    let s = Scratch::new("corrupt");
    std::fs::create_dir_all(s.0.join("config")).unwrap();
    std::fs::write(s.0.join("config/settings.json"), b"{{{ not json \x00").unwrap();
    let mut h = window(&s, 500);
    h.get_by_label_contains("set aside");
    assert!(s.0.join("config/settings.json.corrupt").exists());
    assert!(h.state_mut().settings.read_only);
}

#[test]
fn the_read_only_question_before_the_flight() {
    let s = Scratch::new("ask");
    std::fs::create_dir_all(s.0.join("config")).unwrap();
    std::fs::write(
        s.0.join("config/settings.json"),
        br#"{"ask_mode_before_flight": true}"#,
    )
    .unwrap();
    let mut h = window(&s, 500);
    assert!(h.state_mut().ask_mode_open);
    h.get_by_label("How do you want to fly?");
    assert!(
        h.get_by_label("Read and write")
            .accesskit_node()
            .is_disabled()
    );
    h.get_by_label("Read-only").click();
    h.run_steps(3);
    assert!(!h.state_mut().ask_mode_open);
}

#[test]
fn home_z_and_the_frame_button_see_an_island_whole() {
    let s = support::Scratch::new("frame-island");
    let mut o = support::options(&s);
    o.synthetic = Some(3000);
    let mut h = egui_kittest::Harness::builder()
        .allow_missing_glyphs()
        .with_size(egui::vec2(1200.0, 800.0))
        .with_max_steps(32)
        .build_eframe(move |cc| axross_files_matrix::app::App::new(cc, o));
    assert!(h.state_mut().settle(std::time::Duration::from_secs(60)));
    h.run_steps(3);
    let corners_in_view = |app: &axross_files_matrix::app::App, i: usize| {
        let b = app.islands[i].city.as_ref().unwrap().aabb();
        [b.min.x + 2.0, b.max.x - 2.0].iter().all(|&x| {
            [b.min.z + 2.0, b.max.z - 2.0]
                .iter()
                .all(|&z| app.in_view(axross_files_matrix::math::vec3(x, 0.0, z)))
        })
    };
    let city = h
        .state_mut()
        .islands
        .iter()
        .position(|i| i.hw.is_none())
        .unwrap();
    let machine = h
        .state_mut()
        .islands
        .iter()
        .position(|i| i.hw.is_some())
        .unwrap();
    // the first view: the city whole, 15 degrees down
    assert!(corners_in_view(h.state_mut(), city));
    let pitch = -h.state_mut().camera.pitch.to_degrees();
    assert!((14.0..=16.0).contains(&pitch), "{pitch}");
    // away, then Z on the machine's island (chosen): it whole
    h.state_mut().look_from(
        axross_files_matrix::math::vec3(5000.0, 300.0, 5000.0),
        axross_files_matrix::math::vec3(5100.0, 0.0, 5100.0),
    );
    h.state_mut().select(machine, 0);
    h.key_press(egui::Key::Z);
    for _ in 0..150 {
        h.step();
    }
    assert!(
        corners_in_view(h.state_mut(), machine),
        "Z framed the machine"
    );
    // the inspect panel's button: the city's island
    h.state_mut().select(city, 0);
    h.run_steps(2);
    h.get_by_label("Frame island").click();
    for _ in 0..150 {
        h.step();
    }
    assert!(
        corners_in_view(h.state_mut(), city),
        "the button framed the city"
    );
    // Home: back to the first view (the biggest city), not the map
    h.state_mut().look_from(
        axross_files_matrix::math::vec3(5000.0, 3000.0, 5000.0),
        axross_files_matrix::math::vec3(0.0, 0.0, 0.0),
    );
    h.key_press(egui::Key::Home);
    for _ in 0..150 {
        h.step();
    }
    assert!(corners_in_view(h.state_mut(), city), "Home");
    let pitch = -h.state_mut().camera.pitch.to_degrees();
    assert!((14.0..=16.0).contains(&pitch), "Home is low: {pitch}");
}
