//! Teleporting to the systems of axross's connection manager, against a
//! fake axross MCP server (tests/support/fake_mcp.rs): every protocol of
//! axross's registry listed, opened, shown and flown to; a connect that
//! fails with its reason, one that is cancelled, one that times out; a
//! saved spot inside a system; a system that needs a secret; a backend that
//! tells no sizes; one that cannot be listed. No GPU needed.

mod support;

use axross_files_matrix::app::{App, OpenAsk, StartOptions};
use axross_files_matrix::systems::{Family, SysState};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use std::time::{Duration, Instant};
use support::{Scratch, fake_mcp, options};

fn opts(
    s: &Scratch,
) -> (
    StartOptions,
    std::sync::Arc<std::sync::Mutex<fake_mcp::Seen>>,
) {
    let (client, seen) = fake_mcp::start();
    let mut o = options(s);
    o.mcp = true;
    o.mcp_client = Some(client);
    o.hardware = false;
    o.synthetic = Some(200);
    o.connect_timeout = Some(Duration::from_secs(2));
    (o, seen)
}

fn window(o: StartOptions) -> Harness<'static, App> {
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size(egui::vec2(1200.0, 760.0))
        .with_max_steps(16)
        .build_eframe(move |cc| App::new(cc, o));
    wait(&mut h, "the server's systems", |a| {
        !a.system_profiles().is_empty()
    });
    h
}

/// Pumps until `cond` holds (at most 30 s), a frame now and then.
fn wait(h: &mut Harness<'static, App>, what: &str, cond: impl Fn(&App) -> bool) {
    let start = Instant::now();
    let mut k = 0;
    while !cond(h.state_mut()) {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "waited for {what}"
        );
        h.state_mut().pump(Duration::from_millis(5));
        k += 1;
        if k % 10 == 0 {
            h.run_steps(1);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn island_of(app: &App, name: &str) -> usize {
    app.system_island(name)
        .unwrap_or_else(|| panic!("{name}: no island"))
}

/// The camera looks at island `i` (a teleport landed there).
fn camera_over(app: &App, i: usize) -> bool {
    let Some(c) = app.islands[i].city.as_ref() else {
        return false;
    };
    // (the island framed whole: the camera's target on it)
    let b = c.aabb();
    let t = app.camera.target;
    let margin = 20.0;
    t.x > b.min.x - margin
        && t.x < b.max.x + margin
        && t.z > b.min.z - margin
        && t.z < b.max.z + margin
}

#[test]
fn the_protocol_fixture_is_axross_registry() {
    // (the fixture the fake server lists; against axross's registry where a
    // checkout is at hand - AFM_AXROSS_SRC or ../axross)
    let src = std::env::var_os("AFM_AXROSS_SRC")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../axross"));
    let python = std::env::var_os("AFM_AXROSS_PYTHON").unwrap_or_else(|| "python3".into());
    let fixture: Vec<serde_json::Value> = fake_mcp::registry();
    assert!(
        fixture.len() >= 30,
        "every protocol of axross's: {}",
        fixture.len()
    );
    if !src.join("core/mcp_groups.py").exists() {
        eprintln!("SKIPPED: no axross checkout to compare the fixture with");
        return;
    }
    let out = std::process::Command::new(&python)
        .current_dir(&src)
        .args(["-I", "-c", "import json,sys; sys.path.insert(0,'.'); from core.mcp_groups import protocol_registry; print(json.dumps(protocol_registry()))"])
        .output();
    let Some(out) = out.ok().filter(|o| o.status.success()) else {
        eprintln!("SKIPPED: axross's registry could not be read here");
        return;
    };
    let live: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        live, fixture,
        "tests/fixtures/axross-protocols.json is out of date"
    );
}

#[test]
fn every_protocol_axross_registers_is_listed_opened_shown_and_reached() {
    let s = Scratch::new("systems-all");
    let (o, seen) = opts(&s);
    let mut h = window(o);
    let registry = fake_mcp::registry();
    let profiles = h.state_mut().system_profiles();
    // listed: one per protocol, and the one of a protocol nobody knows yet
    for p in &registry {
        let name = format!("{}-box", p["protocol"].as_str().unwrap());
        assert!(profiles.iter().any(|x| x.name == name), "{name} listed");
    }
    assert!(
        profiles
            .iter()
            .any(|x| x.name == "future" && x.protocol == "quantum-ftp")
    );
    // the menu lists them, with protocol, host and state
    h.state_mut().teleport_open = true;
    h.run_steps(3);
    // (only the rows in sight are laid out: the first ones, by name)
    h.get_by_label("adb-box");
    h.state_mut().teleport_filter = "sftp".into();
    h.run_steps(3);
    h.get_by_label("sftp-box");
    h.get_by_label_contains("sftp.example.test");
    // every one opened (all at once: none waits for another) ...
    let mut names: Vec<String> = registry
        .iter()
        .map(|p| format!("{}-box", p["protocol"].as_str().unwrap()))
        .collect();
    names.push("future".into());
    for n in &names {
        h.state_mut().teleport_system(n, Vec::new());
    }
    wait(&mut h, "every system open", |a| {
        names.iter().all(|n| a.system_state(n) == SysState::Open)
    });
    assert!(h.state_mut().settle(Duration::from_secs(60)));
    assert_eq!(seen.lock().unwrap().opened.len(), names.len());
    // ... shown: its files, its family's sign, the generic look for the
    // unknown protocol - and reached: a teleport lands on each
    for n in &names {
        let i = island_of(h.state_mut(), n);
        let app = h.state_mut();
        let isl = &app.islands[i];
        assert!(
            isl.tree
                .nodes
                .iter()
                .any(|x| &*x.name == std::ffi::OsStr::new("readme.md")),
            "{n}: its files"
        );
        let proto = n.trim_end_matches("-box");
        let family = Family::of(if n == "future" { "quantum-ftp" } else { proto });
        assert!(isl.title.starts_with(family.sign()), "{n}: {}", isl.title);
        h.state_mut().teleport_system(n, Vec::new());
        wait(&mut h, "the flight", |a| a.pending_fly.is_none());
        // (teleports wait their turn: at most two a second)
        h.run_steps(26);
        assert!(camera_over(h.state_mut(), i), "{n}: the camera is there");
    }
    assert_eq!(Family::of("quantum-ftp"), Family::Generic);
}

#[test]
fn a_failed_connect_says_why_and_can_be_retried() {
    let s = Scratch::new("systems-broken");
    let (o, seen) = opts(&s);
    let mut h = window(o);
    h.state_mut().teleport_system("broken", Vec::new());
    // the island stands at once, connecting
    let i = island_of(h.state_mut(), "broken");
    wait(&mut h, "the failure", |a| {
        matches!(a.system_state("broken"), SysState::Failed(_))
    });
    let SysState::Failed(why) = h.state_mut().system_state("broken") else {
        unreachable!()
    };
    assert!(why.contains("Connection refused"), "{why}");
    assert!(
        h.state_mut()
            .island_caption(i)
            .contains("failed: cannot open profile")
    );
    // the camera stands back from the empty plot: the island whole, its
    // state readable
    wait(&mut h, "the flight", |a| a.pending_fly.is_none());
    h.run_steps(26);
    {
        let a = h.state_mut();
        let c = a.islands[i].city.as_ref().unwrap().center();
        assert!(
            (a.camera.pos - c).length() > 150.0,
            "{:?} from {:?}",
            a.camera.pos,
            c
        );
    }
    h.state_mut().teleport_open = true;
    h.run_steps(3);
    h.get_by_label("retry").click();
    h.run_steps(2);
    wait(&mut h, "the second failure", |a| {
        seen.lock().unwrap().opened.len() == 2
            && matches!(a.system_state("broken"), SysState::Failed(_))
    });
}

#[test]
fn a_slow_connect_can_be_cancelled_and_a_hung_one_times_out() {
    let s = Scratch::new("systems-slow");
    let (o, seen) = opts(&s);
    let mut h = window(o);
    h.state_mut().teleport_system("slow", Vec::new());
    assert_eq!(h.state_mut().system_state("slow"), SysState::Connecting);
    let i = island_of(h.state_mut(), "slow");
    assert!(h.state_mut().island_caption(i).contains("connecting…"));
    // the window keeps drawing meanwhile
    let t = Instant::now();
    h.run_steps(5);
    assert!(t.elapsed() < Duration::from_secs(2));
    // the menu's "cancel" gives the connect up
    h.state_mut().teleport_open = true;
    h.state_mut().teleport_filter = "slow".into();
    h.run_steps(3);
    h.get_by_label("cancel").click();
    h.run_steps(2);
    assert_eq!(
        h.state_mut().system_state("slow"),
        SysState::Failed("cancelled".into())
    );
    // given up on the server at once - not only when a late answer comes
    let at = Instant::now();
    wait(&mut h, "the close at the cancel", |_| {
        !seen.lock().unwrap().closed.is_empty()
    });
    assert!(
        at.elapsed() < fake_mcp::SLOW,
        "closed at the cancel, before the connect ended"
    );
    assert_eq!(seen.lock().unwrap().closed, ["profile:slow"]);
    // its late answer is dropped, the connection it made closed (again)
    wait(&mut h, "the late answer", |_| {
        seen.lock().unwrap().closed.len() >= 2
    });
    assert_eq!(
        seen.lock().unwrap().closed,
        ["profile:slow", "profile:slow"]
    );
    assert_eq!(
        h.state_mut().system_state("slow"),
        SysState::Failed("cancelled".into())
    );
    assert!(
        h.state_mut().islands[i].walker.is_none(),
        "no walk of a cancelled system"
    );
    // a host that never answers: given up at the timeout, said so
    h.state_mut().teleport_system("hung", Vec::new());
    wait(&mut h, "the timeout", |a| {
        matches!(a.system_state("hung"), SysState::Failed(_))
    });
    let SysState::Failed(why) = h.state_mut().system_state("hung") else {
        unreachable!()
    };
    assert!(why.contains("no answer within 2 s"), "{why}");
    // and given up on the server at its time limit
    wait(&mut h, "the close at the limit", |_| {
        seen.lock()
            .unwrap()
            .closed
            .iter()
            .any(|c| c == "profile:hung")
    });
}

#[test]
fn a_spot_inside_a_system_opens_it_again() {
    let s = Scratch::new("systems-spot");
    {
        let (o, _) = opts(&s);
        let mut h = window(o);
        h.state_mut().teleport_system("sftp-box", Vec::new());
        wait(&mut h, "sftp-box open", |a| {
            a.system_state("sftp-box") == SysState::Open
        });
        let i = island_of(h.state_mut(), "sftp-box");
        let deep = h.state_mut().islands[i]
            .tree
            .nodes
            .iter()
            .position(|x| &*x.name == std::ffi::OsStr::new("deep"))
            .unwrap() as u32;
        h.state_mut().select(i, deep);
        h.state_mut().save_spot(4);
        let spot = h.state_mut().settings.bookmarks[3].clone().unwrap();
        assert_eq!(spot.system.as_deref(), Some("sftp-box"));
        assert!(
            spot.label.contains("sftp-box: /data/deep"),
            "{}",
            spot.label
        );
    }
    // the next start: the system is not open - the spot opens it, and
    // the camera lands on the district once it has been read
    let (o, seen) = opts(&s);
    let mut h = window(o);
    assert!(h.state_mut().system_island("sftp-box").is_none());
    h.key_press(egui::Key::Num4);
    h.run_steps(2);
    wait(&mut h, "the spot", |a| {
        a.pending_fly.is_none() && a.selected.is_some()
    });
    let (i, n) = h.state_mut().selected.unwrap();
    assert_eq!(
        h.state_mut().islands[i].tree.nodes[n as usize]
            .name
            .to_string_lossy(),
        "deep"
    );
    assert_eq!(seen.lock().unwrap().opened, ["sftp-box"]);
}

#[test]
fn a_system_that_needs_a_secret_is_unlocked_in_axross_never_here() {
    let s = Scratch::new("systems-locked");
    let (mut o, seen) = opts(&s);
    // an axross whose --help knows the connection manager; launches written down
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let ax = s.0.join("axross");
        std::fs::write(&ax, "#!/bin/sh\nprintf '%s\\n' 'usage: axross [--profile NAME] [--connection-manager [NAME]] [PATH ...]'\n").unwrap();
        std::fs::set_permissions(&ax, std::fs::Permissions::from_mode(0o755)).unwrap();
        o.axross = Some(ax);
    }
    let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    o.launch_log = Some(log.clone());
    let mut h = window(o);
    assert_eq!(h.state_mut().system_state("locked"), SysState::NeedsSecret);
    h.state_mut().teleport_open = true;
    h.run_steps(3);
    // type to filter: the one system left
    // (the menu's filter: the text field below the bar's search)
    let filter = h
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .expect("the menu's filter");
    filter.focus();
    h.run_steps(1);
    h.get_all_by_role(egui::accesskit::Role::TextInput)
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap()
        .type_text("lock");
    h.run_steps(3);
    assert_eq!(h.state_mut().teleport_filter, "lock");
    assert!(h.query_by_label("sftp-box").is_none(), "filtered out");
    h.get_by_label_contains("needs unlocking in axross");
    h.get_by_label("unlock").click();
    h.run_steps(3);
    assert_eq!(
        h.state_mut().open_ask,
        Some(OpenAsk::Unlock {
            name: "locked".into()
        })
    );
    h.get_by_label_contains("never asks for a secret");
    // nothing was sent to the server for it - no connect, no secret
    assert!(seen.lock().unwrap().opened.is_empty());
    assert!(h.state_mut().system_island("locked").is_none());
    #[cfg(unix)]
    {
        wait(&mut h, "axross probed", |a| {
            !matches!(a.axross, axross_files_matrix::open::Axross::Looking)
        });
        h.run_steps(2);
        h.get_by_label("Open axross's connection manager").click();
        h.run_steps(2);
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(
            log[0].args,
            vec![std::ffi::OsString::from("--connection-manager=locked")]
        );
    }
}

#[test]
fn a_backend_without_sizes_and_one_that_cannot_be_listed() {
    let s = Scratch::new("systems-odd");
    let (o, _) = opts(&s);
    let mut h = window(o);
    h.state_mut().teleport_system("erroring", Vec::new());
    // (into a folder of it: read as the flight comes)
    h.state_mut()
        .teleport_system("nosizes", vec![b"inbox".to_vec(), b"a.txt".to_vec()]);
    wait(&mut h, "both", |a| {
        a.system_state("nosizes") == SysState::Open
            && matches!(a.system_state("erroring"), SysState::Failed(_))
    });
    wait(&mut h, "the flight into it", |a| a.pending_fly.is_none());
    assert!(h.state_mut().settle(Duration::from_secs(30)));
    // a mail store: read folder by folder as approached, no sizes - equal
    // footprints, and said so
    let i = island_of(h.state_mut(), "nosizes");
    let caption = h.state_mut().island_caption(i);
    assert!(
        caption.contains("sizes unknown: equal footprints"),
        "{caption}"
    );
    assert!(caption.contains("read as approached"), "{caption}");
    let app = h.state_mut();
    let c = app.islands[i].city.as_ref().unwrap();
    let areas: Vec<f32> = c
        .layout
        .buildings
        .iter()
        .map(|b| b.rect.w() * b.rect.h())
        .collect();
    assert!(!areas.is_empty());
    let (lo, hi) = areas
        .iter()
        .fold((f32::MAX, 0f32), |(a, b), &x| (a.min(x), b.max(x)));
    assert!(hi / lo.max(1e-6) < 4.0, "about equal footprints: {areas:?}");
    // the one that cannot be listed: failed, with the reason
    let SysState::Failed(why) = app.system_state("erroring") else {
        unreachable!()
    };
    assert!(why.contains("closed the connection"), "{why}");
}

#[test]
fn every_failed_or_timed_out_connect_is_closed_on_the_server() {
    let s = Scratch::new("systems-close-failed");
    let (o, seen) = opts(&s);
    let mut h = window(o);
    h.state_mut().teleport_system("broken", Vec::new());
    h.state_mut().teleport_system("late", Vec::new());
    wait(&mut h, "both given up", |a| {
        matches!(a.system_state("broken"), SysState::Failed(_))
            && matches!(a.system_state("late"), SysState::Failed(_))
    });
    let SysState::Failed(why) = h.state_mut().system_state("late") else {
        unreachable!()
    };
    assert!(why.contains("no answer within 2 s"), "{why}");
    // the late success, heard after the window gave up: closed on the server
    wait(&mut h, "the closes", |_| {
        let c = &seen.lock().unwrap().closed;
        c.contains(&"profile:late".to_string()) && c.contains(&"profile:broken".to_string())
    });
}

#[test]
fn systems_are_closed_and_the_servers_limit_offers_the_least_recently_visited() {
    let s = Scratch::new("systems-limit");
    let (client, seen) = fake_mcp::start_with_limit(2);
    let (mut o, _) = opts(&s);
    o.mcp_client = Some(client);
    let mut h = window(o);
    h.state_mut().teleport_system("sftp-box", Vec::new());
    wait(&mut h, "sftp-box", |a| {
        a.system_state("sftp-box") == SysState::Open
    });
    h.state_mut().teleport_system("s3-box", Vec::new());
    wait(&mut h, "s3-box", |a| {
        a.system_state("s3-box") == SysState::Open
    });
    // a third: the server's limit - said, with the open ones, oldest first
    h.state_mut().teleport_system("nfs-box", Vec::new());
    wait(&mut h, "the question", |a| {
        matches!(a.open_ask, Some(OpenAsk::TooMany { .. }))
    });
    assert_eq!(
        h.state_mut().open_ask,
        Some(OpenAsk::TooMany {
            name: "nfs-box".into(),
            open: vec!["sftp-box".into(), "s3-box".into()]
        })
    );
    h.run_steps(2);
    h.get_by_label_contains("Close sftp-box (visited longest ago) and open nfs-box")
        .click();
    h.run_steps(2);
    wait(&mut h, "nfs-box open", |a| {
        a.system_state("nfs-box") == SysState::Open
    });
    assert!(
        seen.lock()
            .unwrap()
            .closed
            .contains(&"profile:sftp-box".to_string())
    );
    assert!(
        h.state_mut().system_island("sftp-box").is_none(),
        "its island is gone"
    );
    // the menu's "close"
    h.state_mut().teleport_open = true;
    h.state_mut().teleport_filter = "s3-box".into();
    h.run_steps(3);
    h.get_by_label("close").click();
    h.run_steps(2);
    wait(&mut h, "closed", |_| {
        seen.lock()
            .unwrap()
            .closed
            .contains(&"profile:s3-box".to_string())
    });
    assert!(h.state_mut().system_island("s3-box").is_none());
    assert_eq!(h.state_mut().system_state("s3-box"), SysState::NotOpen);
}

#[test]
fn one_dead_file_never_holds_the_previews_of_others() {
    let s = Scratch::new("systems-dead-preview");
    let files = s.0.join("files");
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join("alive.txt"), "a living file").unwrap();
    let (mut o, _) = opts(&s);
    o.synthetic = None;
    o.paths = vec![files];
    let mut h = window(o);
    h.state_mut().teleport_system("stuck", Vec::new());
    wait(&mut h, "stuck open", |a| {
        a.system_state("stuck") == SysState::Open
    });
    // its file's preview hangs (the server never answers) ...
    for _ in 0..6 {
        let i = island_of(h.state_mut(), "stuck");
        let n = h.state_mut().islands[i]
            .tree
            .nodes
            .iter()
            .position(|x| &*x.name == std::ffi::OsStr::new("readme.md"))
            .unwrap() as u32;
        h.state_mut().select(i, n);
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(150));
    }
    // (each a request of its own, each stuck)
    std::thread::sleep(Duration::from_millis(300));
    // ... and a file elsewhere still gets its preview (in seconds, not
    // never)
    let (i, n) = {
        let a = h.state_mut();
        let i = a
            .islands
            .iter()
            .position(|x| {
                matches!(
                    x.source,
                    axross_files_matrix::world::IslandSource::Local { .. }
                )
            })
            .unwrap();
        let n = a.islands[i]
            .tree
            .nodes
            .iter()
            .position(|x| &*x.name == std::ffi::OsStr::new("alive.txt"))
            .unwrap() as u32;
        (i, n)
    };
    h.state_mut().select(i, n);
    assert!(
        h.state_mut().wait_preview(Duration::from_secs(10)),
        "the preview came"
    );
}

#[test]
fn a_spot_still_works_after_its_profile_left_axross_list() {
    let s = Scratch::new("systems-spot-gone");
    let (o, _) = opts(&s);
    let mut h = window(o);
    h.state_mut().teleport_system("sftp-box", Vec::new());
    wait(&mut h, "sftp-box open", |a| {
        a.system_state("sftp-box") == SysState::Open
    });
    let i = island_of(h.state_mut(), "sftp-box");
    let deep = h.state_mut().islands[i]
        .tree
        .nodes
        .iter()
        .position(|x| &*x.name == std::ffi::OsStr::new("deep"))
        .unwrap() as u32;
    h.state_mut().select(i, deep);
    h.state_mut().save_spot(5);
    h.state_mut().selected = None;
    // axross's list no longer names it (the island still stands)
    if let axross_files_matrix::app::McpState::Connected {
        systems: Ok(sys), ..
    } = &mut h.state_mut().mcp
    {
        sys.profiles.retain(|p| p.name != "sftp-box");
    } else {
        panic!("connected");
    }
    assert!(h.state_mut().system_island("sftp-box").is_some());
    h.key_press(egui::Key::Num5);
    h.run_steps(2);
    wait(&mut h, "the spot", |a| {
        a.pending_fly.is_none() && a.selected.is_some()
    });
    let (j, n) = h.state_mut().selected.unwrap();
    assert_eq!(j, i);
    assert_eq!(
        h.state_mut().islands[j].tree.nodes[n as usize]
            .name
            .to_string_lossy(),
        "deep"
    );
}

#[test]
fn the_menu_rows_keep_their_order_when_states_change() {
    let s = Scratch::new("systems-order");
    let (o, _) = opts(&s);
    let mut h = window(o);
    h.state_mut().teleport_open = true;
    h.run_steps(3);
    let names: Vec<String> = h
        .state_mut()
        .system_profiles()
        .iter()
        .map(|p| p.name.clone())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "by name");
    let order = |h: &mut Harness<'static, App>| -> Vec<String> {
        let mut seen: Vec<(f32, String)> = Vec::new();
        for n in &names {
            if let Some(node) = h.query_by_label(n) {
                seen.push((node.rect().top(), n.clone()));
            }
        }
        seen.sort_by(|a, b| a.0.total_cmp(&b.0));
        seen.into_iter().map(|(_, n)| n).collect()
    };
    let before = order(&mut h);
    assert!(before.len() >= 3, "{before:?}");
    // one opens, one fails: no row moves
    h.state_mut().teleport_system("sftp-box", Vec::new());
    wait(&mut h, "sftp-box open", |a| {
        a.system_state("sftp-box") == SysState::Open
    });
    h.run_steps(3);
    std::thread::sleep(Duration::from_millis(300));
    h.run_steps(2);
    let after = order(&mut h);
    assert_eq!(before, after);
}

#[test]
fn a_system_that_grows_after_its_connect_is_framed_again() {
    let s = Scratch::new("systems-reframe");
    let (o, _) = opts(&s);
    let mut h = window(o);
    // teleported to while it connects: a small plot
    h.state_mut().teleport_system("sftp-box", Vec::new());
    wait(&mut h, "sftp-box open", |a| {
        a.system_state("sftp-box") == SysState::Open
    });
    wait(&mut h, "its walk", |a| {
        a.system_island("sftp-box")
            .is_some_and(|i| a.islands[i].status.done && a.islands[i].city.is_some())
    });
    // the flights and re-framings play out
    for _ in 0..240 {
        h.step();
        h.state_mut().pump(Duration::from_millis(2));
    }
    let app = h.state_mut();
    let i = island_of(app, "sftp-box");
    let b = app.islands[i].city.as_ref().unwrap().aabb();
    for x in [b.min.x + 2.0, b.max.x - 2.0] {
        for z in [b.min.z + 2.0, b.max.z - 2.0] {
            let p = axross_files_matrix::math::vec3(x, 0.0, z);
            assert!(app.in_view(p), "the grown island's corner {p:?} is in view");
        }
    }
    assert!(
        camera_over(app, i),
        "target {:?} pos {:?} pitch {} island {:?} mode {:?} flying {}",
        app.camera.target,
        app.camera.pos,
        app.camera.pitch,
        b,
        app.camera.mode,
        app.camera.flying()
    );
}

#[test]
fn a_server_that_ends_is_noticed_and_started_again() {
    let s = Scratch::new("systems-server-dies");
    let (o, seen) = opts(&s);
    let mut h = window(o);
    h.state_mut().teleport_open = true;
    h.run_steps(2);
    let before = h.state_mut().system_profiles().len();
    assert!(before > 3);
    // the server ends (the next message it is sent is never answered)
    seen.lock().unwrap().die = true;
    h.state_mut().teleport_system("sftp-box", Vec::new());
    wait(&mut h, "the end noticed", |a| {
        matches!(a.mcp, axross_files_matrix::app::McpState::Failed(_))
    });
    h.run_steps(3);
    // said, the last list kept and marked as old
    assert!(
        h.get_all_by_label_contains("axross's server ended")
            .next()
            .is_some()
    );
    let rows = h.state_mut().teleport_rows();
    assert_eq!(rows.len(), before, "the last known list");
    assert!(
        rows.iter()
            .all(|(_, st)| matches!(st, SysState::Unavailable(_)))
    );
    // started again by itself (here: a new server is at hand)
    let (client, _) = fake_mcp::start();
    h.state_mut().opts.mcp_client = Some(client);
    wait(&mut h, "started again", |a| {
        matches!(a.mcp, axross_files_matrix::app::McpState::Connected { .. })
    });
    let rows = h.state_mut().teleport_rows();
    assert!(
        rows.iter()
            .all(|(_, st)| !matches!(st, SysState::Unavailable(_))),
        "{rows:?}"
    );
}
