//! The window where things go wrong or are risky: opening files (axross
//! that can or cannot open a path, programs and scripts that need a
//! question first), a start whose mounts hang, keys with a button in focus,
//! the mouse let go when the window loses focus, a name that is no UTF-8 on
//! screen, and a window at its smallest size. No GPU needed.

mod support;

use axross_files_matrix::app::{App, StartOptions};
#[cfg(unix)]
use axross_files_matrix::app::{OpenAsk, OpenHow};
#[cfg(unix)]
use axross_files_matrix::open::{Axross, Launch};
use axross_files_matrix::tree::NodeId;
use egui::Key;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use support::{Scratch, options};

fn window(opts: StartOptions, size: egui::Vec2) -> Harness<'static, App> {
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size(size)
        .with_max_steps(32)
        .build_eframe(move |cc| App::new(cc, opts));
    assert!(h.state_mut().settle(Duration::from_secs(60)), "settled");
    h.run_steps(3);
    h
}

#[cfg(unix)]
fn script(at: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(at, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(at, std::fs::Permissions::from_mode(0o755)).unwrap();
    at.to_path_buf()
}

/// The island and node of the file named `name`.
fn find(app: &App, name: &str) -> (usize, NodeId) {
    for (i, isl) in app.islands.iter().enumerate() {
        if let Some(n) = isl
            .tree
            .nodes
            .iter()
            .position(|n| n.name.to_string_lossy() == name)
        {
            return (i, n as NodeId);
        }
    }
    panic!("{name} not found");
}

#[cfg(unix)]
fn wait_axross(h: &mut Harness<'static, App>) {
    let start = Instant::now();
    while h.state_mut().axross == Axross::Looking {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "the probe answers"
        );
        h.state_mut().pump(Duration::from_millis(5));
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
/// Lets a check on a thread come back (the open question, a launch).
fn pump_a_while(h: &mut Harness<'static, App>) {
    for _ in 0..40 {
        h.state_mut().pump(Duration::from_millis(5));
        std::thread::sleep(Duration::from_millis(10));
    }
    h.run_steps(2);
}

#[cfg(unix)]
struct Opening {
    _s: Scratch,
    h: Harness<'static, App>,
    log: Arc<Mutex<Vec<Launch>>>,
    files: PathBuf,
    opener: PathBuf,
}

#[cfg(unix)]
fn opening(name: &str, axross_help: &str) -> Opening {
    use std::os::unix::fs::PermissionsExt;
    let s = Scratch::new(name);
    let files = s.0.join("files");
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join("notes.txt"), "plain text").unwrap();
    std::fs::write(
        files.join("evil.desktop"),
        "[Desktop Entry]\nExec=sh -c 'touch /tmp/pwned'\n",
    )
    .unwrap();
    std::fs::write(files.join("run.sh"), "echo hi").unwrap();
    std::fs::set_permissions(files.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(files.join("-rf"), "a dash name").unwrap();
    let axross = script(
        &s.0.join("axross"),
        &format!("printf '%s\\n' '{axross_help}'"),
    );
    let opener = s.0.join("opener-that-is-never-run");
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut opts = options(&s);
    opts.paths = vec![files.clone()];
    opts.axross = Some(axross);
    opts.opener = Some(opener.clone());
    opts.launch_log = Some(log.clone());
    let mut h = window(opts, egui::vec2(1100.0, 700.0));
    wait_axross(&mut h);
    Opening {
        _s: s,
        h,
        log,
        files,
        opener,
    }
}

#[cfg(unix)]
#[test]
fn axross_that_opens_paths_gets_the_path_after_a_double_dash() {
    let mut o = opening("open-new", "usage: axross [--profile NAME] [PATH ...]");
    assert!(matches!(o.h.state_mut().axross, Axross::Ready(_)));
    let (i, n) = find(o.h.state_mut(), "-rf");
    o.h.state_mut().select(i, n);
    o.h.run_steps(2);
    o.h.get_by_label("Open in axross");
    o.h.key_press(Key::O);
    o.h.run_steps(2);
    let log = o.log.lock().unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].label, "axross");
    assert_eq!(
        log[0].args,
        vec!["--".into(), o.files.join("-rf").into_os_string()]
    );
}

#[cfg(unix)]
#[test]
fn an_axross_too_old_is_said_and_nothing_falls_through() {
    let mut o = opening("open-old", "usage: axross [-h] [--version] [--mcp-server]");
    assert!(matches!(o.h.state_mut().axross, Axross::TooOld(_)));
    let (i, n) = find(o.h.state_mut(), "notes.txt");
    o.h.state_mut().select(i, n);
    o.h.run_steps(2);
    o.h.key_press(Key::O);
    o.h.run_steps(3);
    assert!(
        o.log.lock().unwrap().is_empty(),
        "nothing started by itself"
    );
    assert!(matches!(
        o.h.state_mut().open_ask,
        Some(OpenAsk::NoAxross { .. })
    ));
    o.h.get_by_label_contains("too old to open a path");
    // the default program, offered - and only when asked
    o.h.get_by_label("Open with the default program").click();
    o.h.run_steps(1);
    pump_a_while(&mut o.h);
    let log = o.log.lock().unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].program, o.opener);
    assert_eq!(
        log[0].args,
        vec![o.files.join("notes.txt").into_os_string()]
    );
}

#[cfg(unix)]
#[test]
fn programs_and_desktop_entries_ask_first_and_the_file_manager_is_safe() {
    let mut o = opening("open-risky", "usage: axross [--profile NAME] [PATH ...]");
    for name in ["evil.desktop", "run.sh"] {
        o.log.lock().unwrap().clear();
        let (i, n) = find(o.h.state_mut(), name);
        o.h.state_mut().select(i, n);
        o.h.run_steps(2);
        o.h.state_mut().open_selected(OpenHow::DefaultProgram);
        pump_a_while(&mut o.h);
        assert!(
            o.log.lock().unwrap().is_empty(),
            "{name}: not opened at once"
        );
        match &o.h.state_mut().open_ask {
            Some(OpenAsk::Risky { why, .. }) => assert!(why.contains("run"), "{why}"),
            other => panic!("{name}: {other:?}"),
        }
        o.h.get_by_label_contains("Careful:");
        // the safe answer: its folder in the file manager
        o.h.get_by_label("Show it in the file manager").click();
        o.h.run_steps(1);
        pump_a_while(&mut o.h);
        let log = o.log.lock().unwrap();
        assert_eq!(log.len(), 1, "{name}");
        assert_eq!(log[0].label, "the file manager");
        assert_eq!(
            log[0].args,
            vec![o.files.clone().into_os_string()],
            "the folder, not the file"
        );
    }
    // asked and answered "anyway": opened, once
    o.log.lock().unwrap().clear();
    o.h.state_mut().open_selected(OpenHow::DefaultProgram);
    pump_a_while(&mut o.h);
    o.h.get_by_label("Open it anyway").click();
    o.h.run_steps(1);
    pump_a_while(&mut o.h);
    let log = o.log.lock().unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].args, vec![o.files.join("run.sh").into_os_string()]);
    drop(log);
    // a plain text file: no question
    o.log.lock().unwrap().clear();
    let (i, n) = find(o.h.state_mut(), "notes.txt");
    o.h.state_mut().select(i, n);
    o.h.state_mut().open_selected(OpenHow::DefaultProgram);
    pump_a_while(&mut o.h);
    assert_eq!(o.log.lock().unwrap().len(), 1);
    assert!(o.h.state_mut().open_ask.is_none());
}

#[test]
fn a_made_up_file_offers_nothing_to_open() {
    let s = Scratch::new("open-synthetic");
    let mut opts = options(&s);
    opts.synthetic = Some(300);
    let log = Arc::new(Mutex::new(Vec::new()));
    opts.launch_log = Some(log.clone());
    let mut h = window(opts, egui::vec2(1100.0, 700.0));
    let i = h
        .state_mut()
        .islands
        .iter()
        .position(|isl| isl.title.starts_with("synthetic"))
        .unwrap();
    let n = h.state_mut().islands[i].tree.nodes.len() as NodeId - 1;
    h.state_mut().select(i, n);
    h.run_steps(3);
    assert!(h.query_by_label("Open in axross").is_none());
    assert!(h.query_by_label("Open").is_none());
    h.key_press(Key::O);
    h.run_steps(2);
    assert!(log.lock().unwrap().is_empty());
    assert!(h.state_mut().open_ask.is_none());
}

/// A system whose mounts hang (an NFS server gone) until let go.
struct HungProbe(Mutex<std::sync::mpsc::Receiver<()>>);

impl axross_files_matrix::world::Probe for HungProbe {
    fn mounts(&self) -> Vec<axross_files_matrix::mounts::Mount> {
        let _ = self.0.lock().unwrap().recv_timeout(Duration::from_secs(60));
        Vec::new()
    }
    fn machine(&self) -> axross_files_matrix::hw::Machine {
        axross_files_matrix::hw::demo_machine()
    }
    fn absolute(&self, p: &Path) -> PathBuf {
        let _ = self.0.lock().unwrap().recv_timeout(Duration::from_secs(60));
        p.to_path_buf()
    }
    fn is_dir(&self, _: &Path) -> bool {
        true
    }
}

#[test]
fn a_hung_mount_never_holds_the_window() {
    let s = Scratch::new("hung");
    let (release, rx) = std::sync::mpsc::channel::<()>();
    let mut opts = options(&s);
    opts.paths = vec![s.0.join("on-a-dead-server")];
    opts.machine = None;
    opts.probe = Some(Arc::new(HungProbe(Mutex::new(rx))));
    let start = Instant::now();
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size(egui::vec2(1000.0, 650.0))
        .with_max_steps(8)
        .build_eframe(move |cc| App::new(cc, opts));
    h.run_steps(10);
    // the window: there, drawn, saying what it waits for - within the
    // budget (no frame waited for the mounts)
    let took = start.elapsed();
    assert!(
        took < Duration::from_secs(5),
        "the first frames took {took:?}"
    );
    assert!(h.state_mut().starting);
    h.get_by_label_contains("looking at the mounts");
    h.key_press(Key::T);
    h.run_steps(2);
    assert!(h.state_mut().teleport_open, "the keys work meanwhile");
    // let go: the islands come
    // (every wait ends: the sender gone)
    drop(release);
    assert!(h.state_mut().settle(Duration::from_secs(30)));
    assert!(!h.state_mut().starting);
    assert!(h.state_mut().islands.len() >= 2);
}

#[test]
fn shortcuts_work_with_a_button_in_focus() {
    let s = Scratch::new("tab-focus");
    let mut opts = options(&s);
    opts.synthetic = Some(500);
    let mut h = window(opts, egui::vec2(1100.0, 700.0));
    h.get_by_label("Overview").focus();
    h.run_steps(2);
    let before = h.state_mut().camera.pos;
    h.state_mut()
        .camera
        .set_mode(axross_files_matrix::camera::Mode::Fly);
    h.key_down(Key::W);
    h.run_steps(10);
    h.key_up(Key::W);
    h.run_steps(1);
    let moved = (h.state_mut().camera.pos - before).length();
    assert!(moved > 0.5, "W flew with a button in focus ({moved})");
    h.key_press(Key::T);
    h.run_steps(2);
    assert!(h.state_mut().teleport_open);
}

#[test]
fn the_mouse_is_let_go_when_the_window_loses_focus() {
    let s = Scratch::new("capture");
    let mut opts = options(&s);
    opts.synthetic = Some(300);
    let mut h = window(opts, egui::vec2(1000.0, 650.0));
    h.key_press(Key::M);
    h.run_steps(2);
    assert!(h.state_mut().captured());
    h.event(egui::Event::WindowFocused(false));
    h.run_steps(2);
    assert!(!h.state_mut().captured(), "let go");
}

#[cfg(unix)]
#[test]
fn a_name_that_is_no_utf8_is_shown_escaped() {
    use std::os::unix::ffi::OsStrExt;
    let s = Scratch::new("bad-utf8");
    let files = s.0.join("files");
    std::fs::create_dir_all(&files).unwrap();
    let bad = std::ffi::OsStr::from_bytes(b"bad\xff\xfe.txt");
    if std::fs::write(files.join(bad), "x").is_err() {
        eprintln!("SKIPPED: this file system takes no such name");
        return;
    }
    let mut opts = options(&s);
    opts.paths = vec![files];
    let mut h = window(opts, egui::vec2(1100.0, 700.0));
    let (i, n) = {
        let app = h.state_mut();
        let i = app.islands.iter().position(|isl| isl.hw.is_none()).unwrap();
        let n = app.islands[i]
            .tree
            .nodes
            .iter()
            .position(|x| x.name.as_bytes() == b"bad\xff\xfe.txt")
            .unwrap();
        (i, n as NodeId)
    };
    h.state_mut().select(i, n);
    assert!(h.state_mut().wait_preview(Duration::from_secs(10)));
    h.run_steps(4);
    // the title and the path: exact escapes, drawable with any font
    h.get_by_label("bad\\xff\\xfe.txt");
    h.get_by_label_contains("files/bad\\xff\\xfe.txt");
}

#[test]
fn the_smallest_window_keeps_every_control_reachable() {
    for size in [egui::vec2(640.0, 420.0), egui::vec2(800.0, 600.0)] {
        let s = Scratch::new("small-window");
        let mut opts = options(&s);
        opts.synthetic = Some(500);
        let mut h = window(opts, size);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        // the badge always; the rest in the menu
        let badge = h.get_by_label_contains("READ-ONLY").rect();
        assert!(screen.contains_rect(badge), "{size:?}: the badge {badge:?}");
        let menu = h.get_by_label("Menu");
        assert!(screen.contains_rect(menu.rect()));
        menu.click();
        h.run_steps(2);
        h.get_by_label("Settings").click();
        h.run_steps(3);
        assert!(h.state_mut().settings_open);
        // the settings' last control: scrolled to, it is on screen
        let last = h.get_by_label("Check for updates…");
        last.scroll_to_me();
        h.run_steps(3);
        let r = h.get_by_label("Check for updates…").rect();
        assert!(screen.contains_rect(r), "{size:?}: {r:?} in {screen:?}");
        // the Teleport menu: on screen, below the bar, its rows' states in
        // sight (Esc closes it)
        h.state_mut().settings_open = false;
        h.state_mut().teleport_open = true;
        h.run_steps(3);
        let w = h.get_by_label("Teleport").rect();
        assert!(
            w.left() >= 0.0 && w.right() <= size.x,
            "{size:?}: the menu {w:?}"
        );
        let bar = h.get_by_label(" READ-ONLY ").rect();
        assert!(
            w.top() >= bar.bottom() - 1.0,
            "{size:?}: below the bar {w:?}"
        );
        h.key_press(Key::Escape);
        h.run_steps(2);
        assert!(!h.state_mut().teleport_open, "Esc closes the menu");
    }
}

#[test]
fn names_in_every_script_are_drawn() {
    let s = Scratch::new("scripts");
    let files = s.0.join("files");
    std::fs::create_dir_all(&files).unwrap();
    let names = [
        "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30d5}\u{30a1}\u{30a4}\u{30eb}.txt",
        "\u{d55c}\u{ad6d}\u{c5b4}.md",
        "\u{0645}\u{0644}\u{0641}.txt",
        "\u{092b}\u{093c}\u{093e}\u{0907}\u{0932}.txt",
        "rocket \u{1f680}.txt",
    ];
    for n in names {
        std::fs::write(files.join(n), "x").unwrap();
    }
    let mut opts = options(&s);
    opts.paths = vec![files];
    let mut h = window(opts, egui::vec2(1100.0, 700.0));
    // the fonts come from a thread: the window draws meanwhile (missing
    // glyphs as boxes, never a stop)
    let start = Instant::now();
    while !h.state_mut().fonts_ready && start.elapsed() < Duration::from_secs(15) {
        h.state_mut().pump(Duration::from_millis(5));
        h.run_steps(1);
    }
    let required = std::env::var_os("AFM_REQUIRE_FONTS").is_some();
    if !h.state_mut().fonts_ready {
        assert!(!required, "no fonts for these scripts were found");
        eprintln!("SKIPPED: no fonts for these scripts on this machine (AFM_FONT_DIRS names some)");
        return;
    }
    h.run_steps(3);
    for n in names {
        let (i, node) = find(h.state_mut(), n);
        h.state_mut().select(i, node);
        h.run_steps(3);
        h.get_by_label(n);
        let ctx = h.ctx.clone();
        let id = egui::FontId::proportional(14.0);
        for c in n.chars().filter(|c| !c.is_ascii()) {
            let drawn = ctx.fonts_mut(|f| f.has_glyph(&id, c));
            if required {
                assert!(drawn, "{c:?} of {n} has a glyph");
            } else if !drawn {
                eprintln!("(no font here draws {c:?}: a box)");
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn an_unknown_type_can_be_trusted_and_settings_ask_again() {
    let mut o = opening("open-trust", "usage: axross [--profile NAME] [PATH ...]");
    // (made after the start: the walk reads it when asked again)
    std::fs::write(o.files.join("tune.xyz"), "plain data").unwrap();
    let i =
        o.h.state_mut()
            .islands
            .iter()
            .position(|x| x.hw.is_none())
            .unwrap();
    o.h.state_mut().stop_walk(i);
    o.h.state_mut().islands[i].dirty = true;
    o.h.state_mut().sources_open = false;
    // walk it again to see the new file
    {
        let app = o.h.state_mut();
        let root = o.files.clone();
        app.remove_island(i);
        app.add_source(root).unwrap();
        assert!(app.settle(Duration::from_secs(30)));
    }
    let (i, n) = find(o.h.state_mut(), "tune.xyz");
    o.h.state_mut().select(i, n);
    o.h.run_steps(2);
    o.h.state_mut().open_selected(OpenHow::DefaultProgram);
    pump_a_while(&mut o.h);
    match &o.h.state_mut().open_ask {
        Some(OpenAsk::Risky { why, unknown, .. }) => {
            assert!(why.contains("does not know (.xyz)"), "{why}");
            assert_eq!(unknown.as_deref(), Some("xyz"));
        }
        other => panic!("{other:?}"),
    }
    o.h.get_by_label("Do not ask again for .xyz files (Settings can undo it)")
        .click();
    o.h.run_steps(1);
    o.h.get_by_label("Open it anyway").click();
    o.h.run_steps(1);
    pump_a_while(&mut o.h);
    assert_eq!(o.log.lock().unwrap().len(), 1);
    assert_eq!(o.h.state_mut().settings.open_types, vec!["xyz".to_string()]);
    // the next time: opened at once
    o.log.lock().unwrap().clear();
    o.h.state_mut().open_selected(OpenHow::DefaultProgram);
    pump_a_while(&mut o.h);
    assert!(o.h.state_mut().open_ask.is_none());
    assert_eq!(o.log.lock().unwrap().len(), 1);
    // Settings undoes it
    o.h.state_mut().settings_open = true;
    o.h.run_steps(2);
    o.h.get_by_label_contains("Opened without asking: .xyz");
    o.h.get_by_label("Ask again for every type").click();
    o.h.run_steps(2);
    assert!(o.h.state_mut().settings.open_types.is_empty());
    // what runs never offers the box
    o.log.lock().unwrap().clear();
    let (i, n) = find(o.h.state_mut(), "evil.desktop");
    o.h.state_mut().select(i, n);
    o.h.state_mut().open_selected(OpenHow::DefaultProgram);
    pump_a_while(&mut o.h);
    assert!(matches!(
        &o.h.state_mut().open_ask,
        Some(OpenAsk::Risky { unknown: None, .. })
    ));
    assert!(o.h.query_by_label_contains("Do not ask again").is_none());
}

#[test]
fn walks_stop_and_sources_come_and_go_without_a_restart() {
    let s = Scratch::new("sources");
    let a = s.0.join("first");
    let b = s.0.join("second");
    for d in [&a, &b] {
        std::fs::create_dir_all(d.join("sub")).unwrap();
        for k in 0..20 {
            std::fs::write(d.join(format!("sub/f{k}.txt")), "x").unwrap();
        }
    }
    let mut opts = options(&s);
    opts.paths = vec![a.clone()];
    opts.hardware = false;
    let mut h = window(opts, egui::vec2(1100.0, 700.0));
    h.state_mut().sources_open = true;
    h.run_steps(2);
    // add the second folder by its path
    h.state_mut().add_source_text = b.to_string_lossy().into_owned();
    h.run_steps(1);
    h.get_by_label("add").click();
    h.run_steps(2);
    let start = Instant::now();
    while h.state_mut().islands.len() < 2 && start.elapsed() < Duration::from_secs(10) {
        h.state_mut().pump(Duration::from_millis(5));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(h.state_mut().settle(Duration::from_secs(30)));
    assert_eq!(h.state_mut().islands.len(), 2, "added");
    assert!(h.state_mut().islands[1].tree.len() > 20, "walked");
    // the same again, or a relative path: said why not
    assert!(h.state_mut().add_source(b.clone()).is_err());
    assert!(h.state_mut().add_source(PathBuf::from("rel/path")).is_err());
    // a walk under way can be stopped: what it found stays
    let before = h.state_mut().islands.len();
    let third = s.0.join("third");
    std::fs::create_dir_all(third.join("sub")).unwrap();
    {
        let app = h.state_mut();
        let i = app.add_source(third.clone()).unwrap();
        app.stop_walk(i);
        let isl = &app.islands[i];
        assert!(isl.walker.is_none());
        assert!(isl.status.done);
        assert_eq!(isl.status.note.as_deref(), Some("stopped by hand"));
    }
    h.run_steps(2);
    h.get_by_label_contains("stopped by hand");
    // and removed (the last one's button)
    h.get_all_by_label("remove").last().unwrap().click();
    h.run_steps(2);
    assert_eq!(h.state_mut().islands.len(), before);
    assert_eq!(h.state_mut().islands.len(), 2);
}

#[test]
fn no_sources_say_what_to_do() {
    let s = Scratch::new("zero");
    let mut opts = options(&s);
    opts.hardware = false;
    let mut h = window(opts, egui::vec2(1000.0, 700.0));
    h.run_steps(2);
    h.get_by_label("No folders to show yet");
    h.get_by_label("Sources…").click();
    h.run_steps(2);
    assert!(h.state_mut().sources_open);
    // and the counts say 0 islands, never "1 islands"
    h.get_by_label_contains("0 islands · 0 entries");
}

#[test]
fn the_first_start_shows_the_keys_until_a_key_moves() {
    let s = Scratch::new("hint");
    let mut opts = options(&s);
    opts.synthetic = Some(800);
    let mut h = window(opts, egui::vec2(1100.0, 700.0));
    h.get_by_label(axross_files_matrix::app::KEY_HINT);
    h.key_down(Key::W);
    h.run_steps(2);
    h.key_up(Key::W);
    h.run_steps(2);
    assert!(
        h.query_by_label(axross_files_matrix::app::KEY_HINT)
            .is_none()
    );
    assert!(h.state_mut().settings.hint_seen);
    // remembered: not shown at the next start
    drop(h);
    let mut opts = options(&s);
    opts.synthetic = Some(800);
    let mut h = window(opts, egui::vec2(1100.0, 700.0));
    assert!(
        h.query_by_label(axross_files_matrix::app::KEY_HINT)
            .is_none()
    );
    // the Help window keeps it
    h.key_press(Key::H);
    h.run_steps(2);
    h.get_by_label(axross_files_matrix::app::KEY_HINT);
}

#[test]
fn frame_stats_make_no_frames_of_their_own() {
    let s = Scratch::new("frame-stats");
    std::fs::create_dir_all(s.0.join("config")).unwrap();
    std::fs::write(
        s.0.join("config/settings.json"),
        br#"{"rain": false, "idle_fps": 0, "hint_seen": true}"#,
    )
    .unwrap();
    let mut opts = options(&s);
    opts.frame_stats = true;
    opts.hardware = false;
    opts.synthetic = Some(500);
    let mut h = window(opts, egui::vec2(900.0, 600.0));
    for _ in 0..10 {
        h.step();
    }
    assert!(
        !h.ctx.has_requested_repaint(),
        "an idle window with --frame-stats asks for no frames"
    );
}

#[test]
fn the_hardware_legend_shows_near_the_machine() {
    let s = Scratch::new("hw-legend");
    let mut opts = options(&s);
    opts.synthetic = Some(800);
    let mut h = window(opts, egui::vec2(1200.0, 800.0));
    let (c, side) = {
        let app = h.state_mut();
        let m = app.islands.iter().find(|i| i.hw.is_some()).unwrap();
        let c = m.city.as_ref().unwrap();
        (c.center(), c.side())
    };
    h.state_mut().look_from(
        c + axross_files_matrix::math::vec3(0.0, side * 0.6, side * 0.9),
        c,
    );
    h.run_steps(2);
    assert!(h.state_mut().hardware_in_view(), "the machine's legend");
    // over the city: the file types' legend again
    h.state_mut().home();
    for _ in 0..120 {
        h.step();
    }
    assert!(!h.state_mut().hardware_in_view());
}

#[test]
fn teleport_and_sources_open_side_by_side_and_help_is_tall() {
    let s = Scratch::new("windows");
    let mut opts = options(&s);
    opts.synthetic = Some(500);
    let size = egui::vec2(1280.0, 800.0);
    let mut h = window(opts, size);
    h.state_mut().teleport_open = true;
    h.state_mut().sources_open = true;
    h.run_steps(4);
    let wins: Vec<egui::Rect> = h
        .get_all_by_role(egui::accesskit::Role::Window)
        .map(|n| n.rect())
        .collect();
    assert_eq!(wins.len(), 2, "{wins:?}");
    assert!(
        !wins[0].intersects(wins[1]),
        "Teleport and Sources on top of each other: {wins:?}"
    );
    // Help: tall enough to read without scrolling much
    h.state_mut().teleport_open = false;
    h.state_mut().sources_open = false;
    h.state_mut().help_open = true;
    h.run_steps(4);
    let help = h
        .get_all_by_role(egui::accesskit::Role::Window)
        .map(|n| n.rect())
        .next()
        .expect("the help window");
    assert!(help.height() > 600.0, "{help:?}");
}

#[test]
fn a_font_that_fails_while_drawn_is_dropped_and_the_window_goes_on() {
    let s = Scratch::new("font-fails");
    let mut opts = options(&s);
    opts.synthetic = Some(300);
    let mut h = window(opts, egui::vec2(1200.0, 700.0));
    // (as if fonts had been found and one failed in a frame)
    h.state_mut().fonts_ready = true;
    h.state_mut().test_font_failure = true;
    h.run_steps(2);
    assert!(h.state_mut().fonts_dropped);
    assert!(!h.state_mut().fonts_ready);
    h.get_by_label_contains("built-in fonts are used");
    // and it goes on drawing
    h.run_steps(3);
    h.get_by_label("Sources");
}

#[cfg(unix)]
#[test]
fn a_file_swapped_for_a_link_after_the_question_is_not_opened() {
    let mut o = opening("open-swap", "usage: axross [--profile NAME] [PATH ...]");
    let target = o.files.join("evil.desktop");
    let (i, n) = find(o.h.state_mut(), "evil.desktop");
    o.h.state_mut().select(i, n);
    o.h.state_mut().open_selected(OpenHow::DefaultProgram);
    pump_a_while(&mut o.h);
    assert!(matches!(
        o.h.state_mut().open_ask,
        Some(OpenAsk::Risky { .. })
    ));
    // between the question and the answer: another file in its place
    std::fs::remove_file(&target).unwrap();
    std::os::unix::fs::symlink("/etc/hostname", &target).unwrap();
    o.h.get_by_label("Open it anyway").click();
    o.h.run_steps(1);
    pump_a_while(&mut o.h);
    assert!(o.log.lock().unwrap().is_empty(), "not opened");
    o.h.get_by_label_contains("changed since it was checked");
}

#[cfg(unix)]
#[test]
fn an_open_on_a_disk_that_does_not_answer_says_so() {
    let s = Scratch::new("open-dead");
    let files = s.0.join("files");
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join("notes.txt"), "x").unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut opts = options(&s);
    opts.paths = vec![files.clone()];
    opts.opener = Some(s.0.join("opener"));
    opts.launch_log = Some(log.clone());
    opts.open_check_delay = Some(Duration::from_secs(4));
    opts.step = None;
    let mut h = window(opts, egui::vec2(1000.0, 650.0));
    let (i, n) = find(h.state_mut(), "notes.txt");
    h.state_mut().select(i, n);
    h.state_mut().open_selected(OpenHow::DefaultProgram);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        h.state_mut().pump(Duration::from_millis(5));
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(50));
    }
    h.get_by_label_contains("No answer from the disk");
    assert!(log.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn sources_are_one_island_a_folder_kept_and_said_why_not() {
    let s = Scratch::new("sources-canon");
    let a = s.0.join("a");
    std::fs::create_dir_all(a.join("child")).unwrap();
    std::fs::write(a.join("child/f.txt"), "x").unwrap();
    std::fs::write(s.0.join("file.txt"), "x").unwrap();
    std::os::unix::fs::symlink(&a, s.0.join("link")).unwrap();
    let mut opts = options(&s);
    opts.hardware = false;
    let mut h = window(opts, egui::vec2(1000.0, 650.0));
    let add = |h: &mut Harness<'static, App>, text: &str| {
        h.state_mut().request_source(text);
        for _ in 0..40 {
            h.state_mut().pump(Duration::from_millis(5));
            std::thread::sleep(Duration::from_millis(10));
        }
        h.run_steps(2);
    };
    add(&mut h, &a.to_string_lossy());
    assert_eq!(h.state_mut().islands.len(), 1);
    // the same folder again: by `..`, by a link - one island
    add(&mut h, &format!("{}/child/..", a.display()));
    add(&mut h, &s.0.join("link").to_string_lossy());
    assert_eq!(h.state_mut().islands.len(), 1, "one island a folder");
    // a folder inside it, and a file: refused, said why
    add(&mut h, &a.join("child").to_string_lossy());
    assert_eq!(h.state_mut().islands.len(), 1);
    assert!(h.get_all_by_label_contains("lies inside").next().is_some());
    add(&mut h, &s.0.join("file.txt").to_string_lossy());
    assert_eq!(h.state_mut().islands.len(), 1);
    // kept for the next start
    assert_eq!(
        h.state_mut().settings.sources,
        vec![a.to_string_lossy().into_owned()]
    );
    drop(h);
    let mut opts = options(&s);
    opts.hardware = false;
    let mut h = window(opts, egui::vec2(1000.0, 650.0));
    for _ in 0..60 {
        h.state_mut().pump(Duration::from_millis(5));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(h.state_mut().settle(Duration::from_secs(30)));
    assert_eq!(
        h.state_mut().islands.len(),
        1,
        "the added folder at the next start"
    );
    // a choice on another island stays when one is removed
    let b = s.0.join("b");
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(b.join("g.txt"), "x").unwrap();
    add(&mut h, &b.to_string_lossy());
    assert!(h.state_mut().settle(Duration::from_secs(30)));
    let (i, n) = find(h.state_mut(), "g.txt");
    h.state_mut().select(i, n);
    let other = if i == 0 { 1 } else { 0 };
    h.state_mut().remove_island(other);
    let (j, m) = h.state_mut().selected.expect("the choice stays");
    assert_eq!(
        h.state_mut().islands[j].tree.nodes[m as usize]
            .name
            .to_string_lossy(),
        "g.txt"
    );
    assert!(
        h.state_mut()
            .settings
            .sources
            .iter()
            .all(|p| !p.ends_with("/a")),
        "taken out: not kept"
    );
}

#[test]
fn the_key_hint_goes_for_a_mouse_user_too() {
    let s = Scratch::new("hint-mouse");
    let mut opts = options(&s);
    opts.synthetic = Some(500);
    let mut h = window(opts, egui::vec2(1000.0, 650.0));
    h.get_by_label(axross_files_matrix::app::KEY_HINT);
    // the Teleport menu opened from the bar (no key that moves)
    h.state_mut().teleport_open = true;
    h.run_steps(2);
    assert!(h.state_mut().settings.hint_seen);
    assert!(
        h.query_by_label(axross_files_matrix::app::KEY_HINT)
            .is_none()
    );
}

#[test]
fn settings_show_the_cache_as_it_is_now() {
    let s = Scratch::new("cache-size");
    let mut opts = options(&s);
    opts.synthetic = Some(300);
    opts.use_cache = true;
    let mut h = window(opts, egui::vec2(1400.0, 1400.0));
    // a cache file written after the start (as the walks' are)
    let dir = s.0.join("cache");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("one.cache"), vec![1u8; 200_000]).unwrap();
    h.state_mut().settings_open = true;
    for _ in 0..30 {
        h.state_mut().pump(Duration::from_millis(5));
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(10));
    }
    let (bytes, files) = h.state_mut().cache_usage().expect("measured");
    assert!(
        bytes >= 200_000 && files >= 1,
        "measured when Settings opened: {bytes} in {files}"
    );
}

#[test]
fn sources_say_what_axross_is_here() {
    let s = Scratch::new("sources-axross");
    let mut opts = options(&s);
    opts.synthetic = Some(300);
    let mut h = window(opts, egui::vec2(1200.0, 800.0));
    for _ in 0..30 {
        h.state_mut().pump(Duration::from_millis(5));
        std::thread::sleep(Duration::from_millis(10));
    }
    h.state_mut().sources_open = true;
    h.run_steps(3);
    // (this test's axross does not exist)
    h.get_by_label_contains("axross not found");
    h.get_by_label_contains("0.15.186 or later");
}
