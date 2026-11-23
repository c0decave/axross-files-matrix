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

#[cfg(unix)]
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
    }
}
