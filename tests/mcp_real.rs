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
