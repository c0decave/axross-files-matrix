//! A fake axross MCP server (on threads, over pipes): the remote tool
//! group as axross 0.15.186 serves it - `list_remote_profiles` with one
//! saved connection per protocol of axross's registry (the checked-in
//! fixture) and a few odd ones, `open_remote_profile`,
//! `close_remote_profile`, `walk_tree`, `list_directory`, `read_file`.
//! Every request is answered on a thread of its own (a slow connect holds
//! nothing else).
//!
//! The odd profiles: `locked` (needs a secret axross has not stored),
//! `broken` (its connect is refused), `slow` (its connect answers after
//! [`SLOW`]), `hung` (its connect never answers), `nosizes` (a mail store
//! that tells no sizes), `erroring` (opens, then cannot be listed).

use axross_files_matrix::mcp::Client;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long the `slow` profile's connect takes.
pub const SLOW: Duration = Duration::from_millis(1500);

/// What the server saw (the tests look).
#[derive(Default, Debug)]
pub struct Seen {
    pub opened: Vec<String>,
    pub closed: Vec<String>,
    /// This server's own settings (each test its own server).
    pub limit: Option<usize>,
    pub extra: usize,
    pub big: Option<(usize, usize)>,
}

/// The protocols of axross's registry (tests/fixtures/axross-protocols.json).
pub fn registry() -> Vec<Value> {
    let text = include_str!("../fixtures/axross-protocols.json");
    serde_json::from_str::<Vec<Value>>(text).expect("the fixture is JSON")
}

/// The saved connections: one per registered protocol (`<protocol>-box`),
/// one of a protocol this program does not know, and the odd ones.
pub fn profiles() -> Vec<Value> {
    profiles_with(0)
}

fn profiles_with(extra: usize) -> Vec<Value> {
    let mut out: Vec<Value> = registry()
        .iter()
        .map(|p| {
            let proto = p["protocol"].as_str().unwrap();
            json!({"name": format!("{proto}-box"), "protocol": proto, "protocol_name": p["name"],
                   "host": format!("{proto}.example.test"), "port": p["default_port"],
                   "openable": true, "reason": "", "needs_secret": false, "production": false,
                   "open_as": null})
        })
        .collect();
    let odd = |name: &str, proto: &str, openable: bool, reason: &str, secret: bool| {
        json!({"name": name, "protocol": proto, "host": "odd.example.test", "openable": openable,
               "reason": reason, "needs_secret": secret, "production": false, "open_as": null})
    };
    for i in 0..extra {
        out.push(odd(
            &format!("many-{i:04}"),
            ["sftp", "s3", "smb", "imap"][i % 4],
            true,
            "",
            false,
        ));
    }
    out.push(odd("future", "quantum-ftp", true, "", false));
    out.push(odd("big", "sftp", true, "", false));
    out.push(odd("locked", "sftp", false,
        "needs a password that is not stored in the keyring; store it once from the axross GUI (connection dialog: Store password)", true));
    out.push(odd("broken", "sftp", true, "", false));
    out.push(odd("slow", "sftp", true, "", false));
    out.push(odd("hung", "sftp", true, "", false));
    out.push(odd("late", "sftp", true, "", false));
    out.push(odd("stuck", "sftp", true, "", false));
    out.push(odd("nosizes", "imap", true, "", false));
    out.push(odd("erroring", "webdav", true, "", false));
    out
}

/// The tree every system shows: (path, is a folder, size).
const TREE: &[(&str, bool, u64)] = &[
    ("/inbox", true, 0),
    ("/inbox/a.txt", false, 1200),
    ("/inbox/b.bin", false, 5000),
    ("/data", true, 0),
    ("/data/deep", true, 0),
    ("/data/deep/report.pdf", false, 88_000),
    ("/data/deep/notes.md", false, 700),
    ("/readme.md", false, 300),
];

fn depth(p: &str) -> usize {
    p.matches('/').count()
}

fn wrap(v: Value) -> Value {
    json!({"content": [{"type": "text", "text": v.to_string()}], "isError": false})
}

fn error(text: &str) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": true})
}

fn answer(name: &str, args: &Value, seen: &Mutex<Seen>) -> Value {
    let backend = args["backend"].as_str().unwrap_or("");
    let system = backend.strip_prefix("profile:").unwrap_or("");
    match name {
        "list_remote_profiles" => {
            let extra = seen.lock().unwrap().extra;
            wrap(json!({"profiles": profiles_with(extra), "protocols": registry()}))
        }
        "list_backends" => wrap(json!({"backends": []})),
        "open_remote_profile" => {
            let p = args["profile"].as_str().unwrap_or("").to_string();
            {
                let mut s = seen.lock().unwrap();
                s.opened.push(p.clone());
                let open = s
                    .opened
                    .iter()
                    .filter(|o| !s.closed.contains(&format!("profile:{o}")))
                    .collect::<std::collections::HashSet<_>>()
                    .len();
                if s.limit.is_some_and(|l| open > l) {
                    s.opened.pop();
                    return error(&format!(
                        "tool 'open_remote_profile' rejected args: at most {} profiles may be open at once; call close_remote_profile first",
                        s.limit.unwrap_or(0)
                    ));
                }
            }
            match p.as_str() {
                "locked" => error(
                    "profile 'locked' needs a password that is not stored in the keyring; store it once from the axross GUI (connection dialog: Store password) - the MCP server never prompts",
                ),
                "broken" => error(
                    "cannot open profile 'broken': ConnectionRefusedError: [Errno 111] Connection refused",
                ),
                "late" => {
                    std::thread::sleep(Duration::from_millis(3500));
                    wrap(json!({"backend": "profile:late", "profile": "late", "protocol": "sftp"}))
                }
                "hung" => {
                    std::thread::sleep(Duration::from_secs(3600));
                    error("never")
                }
                _ => {
                    if p == "slow" {
                        std::thread::sleep(SLOW);
                    }
                    let extra = seen.lock().unwrap().extra;
                    let proto = profiles_with(extra)
                        .into_iter()
                        .find(|x| x["name"] == p)
                        .map(|x| x["protocol"].clone())
                        .unwrap_or(Value::Null);
                    if proto.is_null() {
                        return error(&format!(
                            "no saved profile named {p:?}; call list_remote_profiles"
                        ));
                    }
                    wrap(
                        json!({"backend": format!("profile:{p}"), "profile": p, "protocol": proto,
                                "class": "Fake", "production": false, "writable": false, "reused": false}),
                    )
                }
            }
        }
        "close_remote_profile" => {
            seen.lock().unwrap().closed.push(backend.to_string());
            wrap(json!({"backend": backend, "closed": true}))
        }
        "walk_tree" | "list_directory" if system == "big" => {
            let (n, page) = seen.lock().unwrap().big.unwrap_or((6012, 2000));
            let path = args["path"]
                .as_str()
                .unwrap_or("/")
                .trim_end_matches('/')
                .to_string();
            if !path.is_empty() {
                return wrap(json!({"entries": [], "next_offset": null}));
            }
            std::thread::sleep(Duration::from_millis(80));
            let offset = args["offset"].as_u64().unwrap_or(0) as usize;
            let max = args["max_entries"]
                .as_u64()
                .unwrap_or(1000)
                .min(page as u64) as usize;
            let end = (offset + max).min(n);
            let entries: Vec<Value> = (offset..end)
                .map(|i| json!({"path": format!("/f{i}"), "name": format!("f{i}"), "size": 10, "is_dir": false}))
                .collect();
            let next = (end < n).then_some(end);
            wrap(json!({"entries": entries, "next_offset": next}))
        }
        "walk_tree" | "list_directory" => {
            if system == "erroring" {
                return error("the server closed the connection");
            }
            let path = args["path"]
                .as_str()
                .unwrap_or("/")
                .trim_end_matches('/')
                .to_string();
            let max_depth = args["max_depth"].as_u64().unwrap_or(1) as usize;
            let base = if path.is_empty() { 0 } else { depth(&path) };
            let prefix = format!("{path}/");
            let entry = |p: &str, d: bool, s: u64| {
                let mut e = json!({"path": p, "name": p.rsplit('/').next().unwrap(), "is_dir": d,
                                   "is_link": false, "modified": "2026-11-20T10:00:00"});
                if system != "nosizes" {
                    e["size"] = json!(s);
                }
                e
            };
            let all: Vec<Value> = TREE
                .iter()
                .filter(|(p, _, _)| {
                    p.starts_with(&prefix) || (path.is_empty() && p.starts_with('/'))
                })
                .filter(|(p, _, _)| {
                    depth(p) - base
                        <= if name == "list_directory" {
                            1
                        } else {
                            max_depth
                        }
                })
                .map(|(p, d, s)| entry(p, *d, *s))
                .collect();
            if name == "list_directory" {
                return wrap(json!(all));
            }
            let offset = args["offset"].as_u64().unwrap_or(0) as usize;
            let max = args["max_entries"].as_u64().unwrap_or(1000) as usize;
            let page: Vec<Value> = all.iter().skip(offset).take(max).cloned().collect();
            let next = (offset + page.len() < all.len()).then_some(offset + page.len());
            wrap(
                json!({"entries": page, "truncated": next.is_some(), "unreadable": [],
                        "complete": next.is_none(), "offset": offset, "next_offset": next}),
            )
        }
        "read_file" | "get_path_info" if system == "stuck" => {
            // (a file on a mount that does not answer)
            std::thread::sleep(Duration::from_secs(3600));
            error("never")
        }
        "read_file" => wrap(json!({"content_b64": "aGVsbG8=", "size": 5, "truncated": false})),
        "get_path_info" => wrap(json!({"size": 5})),
        _ => error("unknown tool"),
    }
}

/// The server with one more system, `big`: one folder of `n` files served
/// `page` a page, each page slow (a remote folder that streams in pages).
pub fn start_with_big_folder(n: usize, page: usize) -> (Arc<Client>, Arc<Mutex<Seen>>) {
    start_with(Seen {
        big: Some((n, page)),
        ..Default::default()
    })
}

/// The server with `n` more saved connections.
pub fn start_with_profiles(n: usize) -> (Arc<Client>, Arc<Mutex<Seen>>) {
    start_with(Seen {
        extra: n,
        ..Default::default()
    })
}

/// The server with a limit of open systems, as axross's has.
pub fn start_with_limit(limit: usize) -> (Arc<Client>, Arc<Mutex<Seen>>) {
    start_with(Seen {
        limit: Some(limit),
        ..Default::default()
    })
}

/// The server and what it saw.
pub fn start() -> (Arc<Client>, Arc<Mutex<Seen>>) {
    start_with(Seen::default())
}

fn start_with(settings: Seen) -> (Arc<Client>, Arc<Mutex<Seen>>) {
    let (to_server_r, to_server_w) = std::io::pipe().unwrap();
    let (from_server_r, from_server_w) = std::io::pipe().unwrap();
    let seen = Arc::new(Mutex::new(settings));
    let out = Arc::new(Mutex::new(from_server_w));
    let s2 = seen.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(to_server_r).lines() {
            let Ok(line) = line else { return };
            let Ok(m) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(id) = m.get("id").cloned() else {
                continue;
            };
            let Some(method) = m["method"].as_str().map(String::from) else {
                continue;
            };
            let (out, seen) = (out.clone(), s2.clone());
            std::thread::spawn(move || {
                let result = match method.as_str() {
                    "initialize" => {
                        json!({"protocolVersion": "2025-06-18", "serverInfo": {"name": "axross-mcp", "version": "0.15.186"}, "capabilities": {"tools": {}}})
                    }
                    "tools/list" => {
                        let tools: Vec<Value> = [
                            "list_directory",
                            "walk_tree",
                            "get_path_info",
                            "read_file",
                            "list_backends",
                            "list_remote_profiles",
                            "open_remote_profile",
                            "close_remote_profile",
                        ]
                        .iter()
                        .map(|n| json!({"name": n}))
                        .collect();
                        json!({ "tools": tools })
                    }
                    "tools/call" => answer(
                        m["params"]["name"].as_str().unwrap_or(""),
                        &m["params"]["arguments"],
                        &seen,
                    ),
                    _ => return,
                };
                let mut w = out.lock().unwrap();
                let _ = writeln!(
                    w,
                    "{}",
                    json!({"jsonrpc": "2.0", "id": id, "result": result})
                );
                let _ = w.flush();
            });
        }
    });
    let client =
        Client::over(from_server_r, to_server_w, Duration::from_secs(10)).expect("the fake server");
    (Arc::new(client), seen)
}
