//! Against a real axross MCP server, when one is named:
//! `AFM_AXROSS_MCP="axross-headless-mcp"` (or `"axross --mcp-server"`, or
//! `"python3 path/to/axross/main.py --mcp-server"`); without it the test
//! says so and stops. A folder of its own is served (`--mcp-root`), walked
//! through the local backend, and a file read back.

use axross_files_matrix::mcp::{Client, PathArg, Tool};
use axross_files_matrix::tree::{Listing, Tree};
use axross_files_matrix::walk::{Event, Limits, Source, Walker, walk_blocking};
use std::ffi::OsString;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn walks_a_folder_through_a_real_server() {
    let Some(cmd) = std::env::var_os("AFM_AXROSS_MCP") else {
        eprintln!("SKIPPED: AFM_AXROSS_MCP names no server");
        return;
    };
    let dir = std::env::temp_dir().join(format!("afm-mcp-real-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("a/b/c")).unwrap();
    std::fs::write(dir.join("a/one.txt"), "first file\n").unwrap();
    std::fs::write(dir.join("a/b/c/deep.bin"), vec![7u8; 3000]).unwrap();
    std::fs::write(dir.join("top.md"), "# top\n").unwrap();
    let mut command: Vec<OsString> = cmd
        .to_string_lossy()
        .split_whitespace()
        .map(OsString::from)
        .collect();
    command.push("--mcp-root".into());
    command.push(dir.clone().into_os_string());
    let client =
        Arc::new(Client::spawn(&command, Duration::from_secs(60)).expect("the server starts"));
    assert!(
        client.has(Tool::Walk) && client.has(Tool::ListDirectory) && client.has(Tool::ReadFile)
    );
    let backends = client.backends(Duration::from_secs(30)).unwrap();
    let local = backends
        .iter()
        .find(|b| b.is_default)
        .expect("a default backend")
        .id
        .clone();
    let root = dir.to_string_lossy().into_owned();
    let (tree, status) = walk_blocking(
        Source::Mcp {
            client: client.clone(),
            backend: local.clone(),
            root: root.clone(),
            lazy: false,
        },
        Limits::default(),
        std::ffi::OsStr::new("served"),
    );
    assert!(status.done, "{status:?}");
    let names: Vec<String> = tree
        .nodes
        .iter()
        .map(|n| n.name.to_string_lossy().into_owned())
        .collect();
    for want in ["a", "b", "c", "one.txt", "top.md"] {
        assert!(names.contains(&want.to_string()), "{want} in {names:?}");
    }
    // the fourth level is read when asked (the camera came near)
    let c = tree
        .nodes
        .iter()
        .find(|n| &*n.name == std::ffi::OsStr::new("c"))
        .unwrap();
    assert_eq!(c.listing, Listing::Unlisted);
    assert!(!names.contains(&"deep.bin".to_string()));
    let (tx, rx) = std::sync::mpsc::channel();
    let walker = Walker::start(
        1,
        1,
        Source::Mcp {
            client: client.clone(),
            backend: local.clone(),
            root: root.clone(),
            lazy: false,
        },
        Limits::default(),
        tx,
        Arc::new(|| {}),
    );
    let mut t = Tree::new(std::ffi::OsStr::new("served"), 0);
    let mut asked = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while std::time::Instant::now() < deadline {
        let Ok(m) = rx.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        match m.event {
            Event::Batch(b) => assert!(t.apply(b)),
            Event::Status(s) if s.done => {
                if let Some(d) = t
                    .nodes
                    .iter()
                    .position(|n| &*n.name == std::ffi::OsStr::new("deep.bin"))
                {
                    assert_eq!(t.nodes[d].size, 3000);
                    break;
                }
                if !asked {
                    let c = t
                        .nodes
                        .iter()
                        .position(|n| &*n.name == std::ffi::OsStr::new("c"))
                        .unwrap();
                    walker.list(c as u32);
                    asked = true;
                }
            }
            _ => {}
        }
    }
    assert!(
        t.nodes
            .iter()
            .any(|n| &*n.name == std::ffi::OsStr::new("deep.bin")),
        "read when asked"
    );
    assert_eq!(t.root().total, 11 + 3000 + 6);
    let bytes = client
        .read(
            &local,
            &PathArg::Text(format!("{root}/a/one.txt")),
            5,
            Duration::from_secs(30),
        )
        .unwrap();
    assert_eq!(bytes, b"first");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The systems of a real axross's connection manager, when a home with
/// saved connections is named: `AFM_AXROSS_HOME` holds
/// `.config/axross/profiles.json` with `ram1` (ramfs), `sql` (sqlite, a
/// database with /docs/a.txt, /docs/deep/b.bin, /readme.md) and `box`
/// (sftp, its password not stored). The server is started with that HOME
/// and the remote tool group; the window lists, opens, walks and flies.
#[test]
fn systems_of_a_real_axross() {
    let (Some(cmd), Some(home)) = (
        std::env::var_os("AFM_AXROSS_MCP"),
        std::env::var_os("AFM_AXROSS_HOME"),
    ) else {
        eprintln!("SKIPPED: AFM_AXROSS_MCP and AFM_AXROSS_HOME name no server and home");
        return;
    };
    let mut home_arg = OsString::from("HOME=");
    home_arg.push(&home);
    let mut command: Vec<OsString> = vec!["/usr/bin/env".into(), home_arg];
    command.extend(cmd.to_string_lossy().split_whitespace().map(OsString::from));
    command.extend(["--mcp-tools".into(), "remote".into()]);
    let client =
        Arc::new(Client::spawn(&command, Duration::from_secs(90)).expect("the server starts"));
    let systems = client
        .systems(Duration::from_secs(60))
        .expect("it lists saved connections");
    let names: Vec<&str> = systems.profiles.iter().map(|p| p.name.as_str()).collect();
    assert!(
        names.contains(&"ram1") && names.contains(&"sql") && names.contains(&"box"),
        "{names:?}"
    );
    // every protocol of its registry, at run time
    let protos: Vec<&str> = systems.protocols.iter().map(|p| p.id.as_str()).collect();
    for want in [
        "sftp", "nfs", "smb", "s3", "imap", "sqlite", "git", "adb", "ramfs",
    ] {
        assert!(protos.contains(&want), "{want} in {protos:?}");
    }
    let bx = systems.profiles.iter().find(|p| p.name == "box").unwrap();
    assert!(bx.needs_secret && !bx.openable, "{bx:?}");
    // the window: sql and ram1 opened, walked, reached; box needs unlocking
    let dir = std::env::temp_dir().join(format!("afm-real-systems-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut opts = axross_files_matrix::app::StartOptions {
        mounts: false,
        mcp: true,
        hardware: false,
        synthetic: Some(100),
        config_dir: Some(dir.join("config")),
        cache_dir: Some(dir.join("cache")),
        use_cache: false,
        updates: false,
        clock: Some(1.0),
        step: Some(1.0 / 30.0),
        axross: Some(dir.join("no-axross")),
        ..Default::default()
    };
    opts.mcp_client = Some(client.clone());
    let mut h = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1100.0, 700.0))
        .build_eframe(move |cc| axross_files_matrix::app::App::new(cc, opts));
    let wait = |h: &mut egui_kittest::Harness<'static, axross_files_matrix::app::App>,
                what: &str,
                cond: &dyn Fn(&axross_files_matrix::app::App) -> bool| {
        let start = std::time::Instant::now();
        while !cond(h.state_mut()) {
            assert!(
                start.elapsed() < Duration::from_secs(90),
                "waited for {what}"
            );
            h.state_mut().pump(Duration::from_millis(5));
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    use axross_files_matrix::systems::SysState;
    wait(&mut h, "the systems", &|a| !a.system_profiles().is_empty());
    assert_eq!(h.state_mut().system_state("box"), SysState::NeedsSecret);
    h.state_mut().teleport_system("ram1", Vec::new());
    h.state_mut().teleport_system(
        "sql",
        vec![b"docs".to_vec(), b"deep".to_vec(), b"b.bin".to_vec()],
    );
    wait(&mut h, "both open", &|a| {
        a.system_state("ram1") == SysState::Open && a.system_state("sql") == SysState::Open
    });
    // sqlite: a database, read as approached - the flight asked for docs and deep
    wait(&mut h, "the flight into sql", &|a| a.pending_fly.is_none());
    let (i, n) = h.state_mut().selected.expect("landed on b.bin");
    let app = h.state_mut();
    assert_eq!(
        app.islands[i].tree.nodes[n as usize].name.to_string_lossy(),
        "b.bin"
    );
    assert_eq!(app.islands[i].tree.nodes[n as usize].size, 3000);
    assert!(
        app.island_caption(i).contains("read as approached"),
        "{}",
        app.island_caption(i)
    );
    client.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
