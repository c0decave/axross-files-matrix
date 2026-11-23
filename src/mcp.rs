//! A small MCP client for axross's headless server (`axross-headless-mcp`,
//! or `axross --mcp-server`): JSON-RPC 2.0, one JSON object per line over
//! the server's stdin and stdout. Read-only: only the listing, walking,
//! describing and reading tools are called (and the remote profiles opened
//! when asked).
//!
//! The server is found as the settings say, else `axross-headless-mcp` on
//! the PATH, else `axross` on the PATH (`--mcp-server`), else
//! `uvx --offline axross-headless-mcp` (a package uv already has - nothing
//! is fetched unless the setting allows it). Without one: local disks and
//! mounts only, and no message about it but a line in the sources panel.
//! Nothing goes through a shell: the program and each argument are passed
//! as they are.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The longest line read from the server (a 4 MiB read_file page in
/// base64 is under 6 MiB).
pub const MAX_LINE: usize = 32 << 20;
pub const PROTOCOL: &str = "2025-06-18";
/// The most stderr lines kept (shown when the server fails).
const STDERR_LINES: usize = 40;

/// The tools this client uses, by what they do; each with its names (the
/// current public one first, the former ones after: older servers).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    ListDirectory,
    Walk,
    Stat,
    ReadFile,
    ListBackends,
    ListProfiles,
    OpenProfile,
    CloseProfile,
}

impl Tool {
    pub fn names(self) -> &'static [&'static str] {
        match self {
            Tool::ListDirectory => &["list_directory", "list_dir"],
            Tool::Walk => &["walk_tree", "walk"],
            Tool::Stat => &["get_path_info", "stat"],
            Tool::ReadFile => &["read_file"],
            Tool::ListBackends => &["list_backends"],
            Tool::ListProfiles => &["list_remote_profiles", "remote_profiles"],
            Tool::OpenProfile => &["open_remote_profile", "remote_open"],
            Tool::CloseProfile => &["close_remote_profile", "remote_close"],
        }
    }
}

type Waiters = Arc<Mutex<HashMap<u64, Sender<Result<Value, String>>>>>;
type Shared = Arc<Mutex<Box<dyn Write + Send>>>;

pub struct Client {
    writer: Shared,
    waiters: Waiters,
    next: AtomicU64,
    alive: Arc<AtomicBool>,
    child: Mutex<Option<Child>>,
    /// The tools the server lists.
    pub tools: Vec<String>,
    pub server: String,
    pub version: String,
    stderr: Arc<Mutex<Vec<String>>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("server", &self.server)
            .field("version", &self.version)
            .field("tools", &self.tools.len())
            .finish()
    }
}

/// A backend the server can address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backend {
    pub id: String,
    pub class: String,
    pub name: String,
    pub remote: bool,
    pub is_default: bool,
    pub profile: Option<String>,
    pub protocol: Option<String>,
    pub production: bool,
}

/// A saved connection of axross's connection manager, as
/// `list_remote_profiles` gives it (never a secret).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Profile {
    pub name: String,
    pub protocol: String,
    /// The protocol's name in axross's registry.
    pub protocol_name: Option<String>,
    pub host: Option<String>,
    pub port: Option<u64>,
    pub username: Option<String>,
    /// Its backend id while it is open in this session.
    pub open_as: Option<String>,
    pub openable: bool,
    /// Why it cannot be opened (empty when it can).
    pub reason: String,
    /// Only a password or key passphrase to store in axross is missing.
    pub needs_secret: bool,
    pub production: bool,
}

/// A protocol axross has a backend for (its registry at run time).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Protocol {
    pub id: String,
    pub name: String,
    /// How it signs in: none, password, ssh, api_key, oauth.
    pub credential: String,
    pub default_port: u64,
    pub is_local: bool,
}

/// The saved connections and the protocols.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Systems {
    pub profiles: Vec<Profile>,
    /// Empty from a server that does not list them (older axross).
    pub protocols: Vec<Protocol>,
}

/// What opening a profile gave.
#[derive(Clone, Debug, PartialEq)]
pub struct Opened {
    /// Its backend id (`profile:<name>`).
    pub backend: String,
    pub protocol: Option<String>,
    pub class: Option<String>,
}

fn parse_profile(v: &Value) -> Option<Profile> {
    let name = v["name"].as_str()?.to_string();
    if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        return None;
    }
    let text = |k: &str| v[k].as_str().map(String::from).filter(|s| !s.is_empty());
    let openable = v["openable"].as_bool().unwrap_or(true);
    let reason = v["reason"]
        .as_str()
        .unwrap_or("")
        .chars()
        .take(500)
        .collect::<String>();
    // (an older server says it only in the reason)
    let needs_secret = v["needs_secret"].as_bool().unwrap_or_else(|| {
        let r = reason.to_ascii_lowercase();
        !openable && (r.contains("password") || r.contains("passphrase"))
    });
    Some(Profile {
        protocol: v["protocol"].as_str().unwrap_or("").to_string(),
        protocol_name: text("protocol_name"),
        host: text("host"),
        port: v["port"].as_u64(),
        username: text("username"),
        open_as: text("open_as"),
        openable,
        reason,
        needs_secret,
        production: v["production"].as_bool().unwrap_or(false),
        name,
    })
}

fn parse_protocol(v: &Value) -> Option<Protocol> {
    Some(Protocol {
        id: v["protocol"].as_str()?.to_string(),
        name: v["name"].as_str().unwrap_or("").to_string(),
        credential: v["credential"].as_str().unwrap_or("").to_string(),
        default_port: v["default_port"].as_u64().unwrap_or(0),
        is_local: v["is_local"].as_bool().unwrap_or(false),
    })
}

/// Why a server lists no saved connections: said plainly, never an empty
/// list.
pub const NO_PROFILE_TOOLS: &str = "this axross's MCP server cannot list saved connections \
     (no list_remote_profiles: it was started without the remote tool group, or it is \
     older than axross 0.15.1)";

/// One entry of a listing or a walk.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteEntry {
    /// The full path (walks) or empty (listings: the folder's path + name).
    pub path: String,
    /// The name's raw bytes (from `name_b64` when it is no UTF-8).
    pub name: Vec<u8>,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    pub mtime: i64,
    /// The backend told the size (a mail store or a database may not).
    pub size_known: bool,
}

/// A page of a walk.
#[derive(Clone, Debug, Default)]
pub struct WalkPage {
    pub entries: Vec<RemoteEntry>,
    pub unreadable: Vec<String>,
    pub next_offset: Option<u64>,
    pub truncated: bool,
    /// Entries left out: their names were not safe ([`safe_name`]).
    pub rejected: usize,
}

/// The entries of an answer and how many were left out.
fn parse_entries(list: Option<&Vec<Value>>) -> (Vec<RemoteEntry>, usize) {
    let Some(list) = list else {
        return (Vec::new(), 0);
    };
    let entries: Vec<RemoteEntry> = list.iter().filter_map(parse_entry).collect();
    let rejected = list.len() - entries.len();
    (entries, rejected)
}

impl Client {
    /// Talks to a server over a reader and a writer (its stdout and stdin):
    /// `initialize`, then `tools/list`.
    pub fn over(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        timeout: Duration,
    ) -> Result<Client, String> {
        let waiters: Waiters = Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));
        let writer: Shared = Arc::new(Mutex::new(Box::new(writer)));
        let mut client = Client {
            writer: writer.clone(),
            waiters: waiters.clone(),
            next: AtomicU64::new(1),
            alive: alive.clone(),
            child: Mutex::new(None),
            tools: Vec::new(),
            server: String::new(),
            version: String::new(),
            stderr: Arc::new(Mutex::new(Vec::new())),
        };
        std::thread::Builder::new()
            .name("afm-mcp-read".into())
            .spawn(move || read_loop(reader, waiters, alive, writer))
            .map_err(|e| e.to_string())?;
        let init = client.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL,
                "capabilities": {},
                "clientInfo": {"name": "axross-files-matrix", "version": env!("CARGO_PKG_VERSION")},
            }),
            timeout,
        )?;
        client.server = init["serverInfo"]["name"]
            .as_str()
            .unwrap_or("")
            .to_string();
        client.version = init["serverInfo"]["version"]
            .as_str()
            .unwrap_or("")
            .to_string();
        client.notify("notifications/initialized", json!({}))?;
        let tools = client.request("tools/list", json!({}), timeout)?;
        client.tools = tools["tools"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| t["name"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        Ok(client)
    }

    /// Starts `command` (the program, then its arguments) and talks to it.
    pub fn spawn(command: &[OsString], timeout: Duration) -> Result<Client, String> {
        let (program, args) = command.split_first().ok_or("no command")?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console window beside the city
            cmd.creation_flags(0x0800_0000);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // a process group of its own: what the server starts ends
            // with it ([`Client::shutdown`] kills the group)
            cmd.process_group(0);
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: prctl is async-signal-safe and touches no memory of
            // the parent; it only asks the kernel to kill the server
            // should this program die without ending it (a crash, SIGKILL).
            // (The signal comes when the thread that started it ends:
            // started from a thread that lasts - spawn_lasting.)
            unsafe {
                cmd.pre_exec(|| {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                    Ok(())
                });
            }
        }
        let mut child =
            spawn_lasting(cmd).map_err(|e| format!("{}: {e}", Path::new(program).display()))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr_lines = Arc::new(Mutex::new(Vec::new()));
        if let Some(err) = child.stderr.take() {
            let lines = stderr_lines.clone();
            let _ = std::thread::Builder::new()
                .name("afm-mcp-stderr".into())
                .spawn(move || {
                    for line in BufReader::new(err.take(1 << 24)).lines() {
                        let Ok(line) = line else { break };
                        let mut l = lines.lock().unwrap_or_else(|p| p.into_inner());
                        if l.len() >= STDERR_LINES {
                            l.remove(0);
                        }
                        l.push(line.chars().take(400).collect());
                    }
                });
        }
        match Client::over(stdout, stdin, timeout) {
            Ok(mut client) => {
                client.stderr = stderr_lines;
                *client.child.lock().unwrap_or_else(|p| p.into_inner()) = Some(child);
                Ok(client)
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let tail = stderr_lines
                    .lock()
                    .map(|l| l.last().cloned().unwrap_or_default())
                    .unwrap_or_default();
                Err(if tail.is_empty() {
                    e
                } else {
                    format!("{e} ({tail})")
                })
            }
        }
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// The server's last stderr lines.
    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr.lock().map(|l| l.clone()).unwrap_or_default()
    }

    fn send(&self, message: &Value) -> Result<(), String> {
        let mut line = serde_json::to_string(message).map_err(|e| e.to_string())?;
        line.push('\n');
        let mut w = self.writer.lock().unwrap_or_else(|p| p.into_inner());
        w.write_all(line.as_bytes())
            .and_then(|_| w.flush())
            .map_err(|e| format!("the server is gone ({e})"))
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// One request and its answer's `result` (an `error` as Err).
    pub fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        if !self.is_alive() {
            return Err("the server has ended".into());
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = channel();
        self.waiters
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, tx);
        let sent =
            self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        if let Err(e) = sent {
            self.waiters
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            return Err(e);
        }
        match rx.recv_timeout(timeout) {
            Ok(r) => r,
            Err(_) => {
                self.waiters
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&id);
                // (the server is told; a late answer is dropped)
                let _ = self.notify(
                    "notifications/cancelled",
                    json!({"requestId": id, "reason": "timeout"}),
                );
                if self.is_alive() {
                    Err(format!(
                        "{method}: no answer within {} s",
                        timeout.as_secs()
                    ))
                } else {
                    Err("the server has ended".into())
                }
            }
        }
    }

    /// The name this server knows `tool` by (None: it has none of them).
    pub fn tool_name(&self, tool: Tool) -> Option<&'static str> {
        tool.names()
            .iter()
            .find(|n| self.tools.iter().any(|t| t == *n))
            .copied()
    }

    pub fn has(&self, tool: Tool) -> bool {
        self.tool_name(tool).is_some()
    }

    /// Calls `tool`: its structured result (or the JSON in its text), an
    /// error result as Err with its text.
    pub fn call(&self, tool: Tool, args: Value, timeout: Duration) -> Result<Value, String> {
        let name = self
            .tool_name(tool)
            .ok_or_else(|| format!("the server has no {} tool", tool.names()[0]))?;
        let r = self.request(
            "tools/call",
            json!({"name": name, "arguments": args}),
            timeout,
        )?;
        let text = r["content"]
            .as_array()
            .and_then(|c| c.iter().find_map(|p| p["text"].as_str()))
            .unwrap_or("");
        if r["isError"].as_bool() == Some(true) {
            return Err(text.chars().take(500).collect());
        }
        if let Some(s) = r.get("structuredContent") {
            return Ok(s.clone());
        }
        serde_json::from_str(text).map_err(|e| format!("{name}: no JSON result ({e})"))
    }

    /// The backends (`list_backends`). A server serving its local disk
    /// alone (no remote group) has no such tool: its one backend is the
    /// default, named "" here (the calls then name none).
    pub fn backends(&self, timeout: Duration) -> Result<Vec<Backend>, String> {
        if !self.has(Tool::ListBackends) {
            return Ok(vec![Backend {
                id: String::new(),
                class: "LocalFS".into(),
                name: "default".into(),
                remote: false,
                is_default: true,
                profile: None,
                protocol: None,
                production: false,
            }]);
        }
        let v = self.call(Tool::ListBackends, json!({}), timeout)?;
        let list = v.get("backends").unwrap_or(&v);
        Ok(list
            .as_array()
            .map(|a| a.iter().filter_map(parse_backend).collect())
            .unwrap_or_default())
    }

    /// The saved remote profiles' names (`list_remote_profiles`; empty when
    /// the server has no such tool).
    pub fn profiles(&self, timeout: Duration) -> Result<Vec<String>, String> {
        match self.systems(timeout) {
            Ok(s) => Ok(s.profiles.into_iter().map(|p| p.name).collect()),
            Err(e) if e == NO_PROFILE_TOOLS => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// The saved connections of axross's connection manager and the
    /// protocols it has backends for (`list_remote_profiles`); Err with
    /// [`NO_PROFILE_TOOLS`] when the server cannot list them.
    pub fn systems(&self, timeout: Duration) -> Result<Systems, String> {
        if !self.has(Tool::ListProfiles) {
            return Err(NO_PROFILE_TOOLS.into());
        }
        let v = self.call(Tool::ListProfiles, json!({}), timeout)?;
        let list = v.get("profiles").unwrap_or(&v);
        let mut profiles: Vec<Profile> = list
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|p| match p.as_str() {
                        // (a server of names only)
                        Some(n) => parse_profile(&json!({"name": n})),
                        None => parse_profile(p),
                    })
                    .take(4096)
                    .collect()
            })
            .unwrap_or_default();
        profiles.sort_by(|a, b| a.name.cmp(&b.name));
        profiles.dedup_by(|a, b| a.name == b.name);
        let protocols = v["protocols"]
            .as_array()
            .map(|a| a.iter().filter_map(parse_protocol).take(1024).collect())
            .unwrap_or_default();
        Ok(Systems {
            profiles,
            protocols,
        })
    }

    /// Opens a saved remote profile: it becomes a backend of this session.
    pub fn open_profile(&self, name: &str, timeout: Duration) -> Result<Opened, String> {
        if !self.has(Tool::OpenProfile) {
            return Err("this axross's MCP server cannot open saved connections".into());
        }
        let v = self.call(Tool::OpenProfile, json!({"profile": name}), timeout)?;
        let backend = v["backend"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| format!("profile:{name}"));
        Ok(Opened {
            backend,
            protocol: v["protocol"].as_str().map(String::from),
            class: v["class"].as_str().map(String::from),
        })
    }

    /// Closes a profile this session opened (best effort).
    pub fn close_profile(&self, backend: &str, timeout: Duration) -> Result<(), String> {
        if !self.has(Tool::CloseProfile) {
            return Ok(());
        }
        self.call(Tool::CloseProfile, json!({"backend": backend}), timeout)
            .map(|_| ())
    }

    /// One folder's entries (`list_directory`).
    pub fn list(
        &self,
        backend: &str,
        path: &PathArg,
        timeout: Duration,
    ) -> Result<Vec<RemoteEntry>, String> {
        let mut args = path.args();
        with_backend(&mut args, backend);
        let v = self.call(Tool::ListDirectory, args, timeout)?;
        let list = v.get("entries").unwrap_or(&v);
        Ok(parse_entries(list.as_array()).0)
    }

    /// One folder's entries a page at a time (`walk_tree` one level deep,
    /// paged by offset: a folder of any size, no answer over the bound);
    /// `list_directory` in one go where the server has no walk.
    pub fn list_page(
        &self,
        backend: &str,
        path: &PathArg,
        offset: u64,
        page: u32,
        timeout: Duration,
    ) -> Result<WalkPage, String> {
        if !self.has(Tool::Walk) {
            if offset > 0 {
                return Ok(WalkPage::default());
            }
            let mut args = path.args();
            with_backend(&mut args, backend);
            let v = self.call(Tool::ListDirectory, args, timeout)?;
            let (entries, rejected) = parse_entries(v.get("entries").unwrap_or(&v).as_array());
            return Ok(WalkPage {
                entries,
                rejected,
                ..Default::default()
            });
        }
        let mut p = self.walk(backend, path, 1, page, offset, timeout)?;
        // (only the folder's own entries: a server that walks deeper than
        // asked gives nothing twice)
        let want = match path {
            PathArg::Text(t) => t.clone(),
            PathArg::Raw(b) => String::from_utf8_lossy(b).into_owned(),
        };
        let want = match want.trim_end_matches('/') {
            "" => "/".to_string(),
            w => w.to_string(),
        };
        p.entries.retain(|e| {
            let full = e.path.trim_end_matches('/');
            full.is_empty()
                || match full.rfind('/') {
                    Some(0) => want == "/",
                    Some(i) => full[..i] == want,
                    None => false,
                }
        });
        Ok(p)
    }

    /// A page of a walk (`walk_tree`).
    pub fn walk(
        &self,
        backend: &str,
        path: &PathArg,
        max_depth: u32,
        max_entries: u32,
        offset: u64,
        timeout: Duration,
    ) -> Result<WalkPage, String> {
        let mut args = path.args();
        with_backend(&mut args, backend);
        args["max_depth"] = json!(max_depth);
        args["max_entries"] = json!(max_entries);
        args["offset"] = json!(offset);
        let v = self.call(Tool::Walk, args, timeout)?;
        let (entries, rejected) = parse_entries(v["entries"].as_array());
        Ok(WalkPage {
            entries,
            rejected,
            unreadable: v["unreadable"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|u| u["path"].as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            next_offset: v["next_offset"].as_u64(),
            truncated: v["truncated"].as_bool().unwrap_or(false),
        })
    }

    /// Up to `max` bytes of a file from its start (`read_file`, base64).
    pub fn read(
        &self,
        backend: &str,
        path: &PathArg,
        max: u32,
        timeout: Duration,
    ) -> Result<Vec<u8>, String> {
        let mut args = path.args();
        with_backend(&mut args, backend);
        args["max_bytes"] = json!(max.max(1));
        args["encoding"] = json!("base64");
        let v = self.call(Tool::ReadFile, args, timeout)?;
        if let Some(b) = v["content_b64"].as_str() {
            let mut bytes = b64_decode(b).ok_or("read_file: bad base64")?;
            bytes.truncate(max as usize);
            Ok(bytes)
        } else if let Some(t) = v["text"].as_str() {
            let mut bytes = t.as_bytes().to_vec();
            bytes.truncate(max as usize);
            Ok(bytes)
        } else {
            Err("read_file: no content".into())
        }
    }

    /// One path's details (`get_path_info`).
    pub fn stat(&self, backend: &str, path: &PathArg, timeout: Duration) -> Result<Value, String> {
        let mut args = path.args();
        with_backend(&mut args, backend);
        self.call(Tool::Stat, args, timeout)
    }
}

impl Client {
    /// Ends the server now - its whole process group - and reaps it, also
    /// while calls are still waiting (they fail at once). Called at the
    /// program's end for every client, whoever still holds one.
    pub fn shutdown(&self) {
        self.alive.store(false, Ordering::Relaxed);
        if let Some(mut child) = self.child.lock().unwrap_or_else(|p| p.into_inner()).take() {
            #[cfg(unix)]
            {
                let pid = child.id() as libc::pid_t;
                // SAFETY: a signal to the group this program made for the
                // server (process_group(0): its id is the server's pid)
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        fail_all(&self.waiters, "the server was ended");
    }

    /// The server's process id (tests).
    pub fn pid(&self) -> Option<u32> {
        self.child
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|c| c.id())
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Names the backend in a call's arguments (none for "": the default).
fn with_backend(args: &mut Value, backend: &str) {
    if !backend.is_empty() {
        args["backend"] = json!(backend);
    }
}

/// A path for a tool: as text, or as raw bytes (`path_b64`) when the name
/// is no UTF-8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathArg {
    Text(String),
    Raw(Vec<u8>),
}

impl PathArg {
    pub fn from_bytes(bytes: &[u8]) -> PathArg {
        match std::str::from_utf8(bytes) {
            Ok(s) => PathArg::Text(s.to_string()),
            Err(_) => PathArg::Raw(bytes.to_vec()),
        }
    }
    fn args(&self) -> Value {
        match self {
            PathArg::Text(s) => json!({"path": s}),
            PathArg::Raw(b) => json!({"path_b64": b64_encode(b)}),
        }
    }
    pub fn display(&self) -> String {
        match self {
            PathArg::Text(s) => s.clone(),
            PathArg::Raw(b) => String::from_utf8_lossy(b).into_owned(),
        }
    }
}

/// Starts `cmd` from a thread that lasts as long as the program (Linux:
/// the parent-death signal follows the thread that started the child, and
/// the threads that look for servers end).
#[cfg(target_os = "linux")]
fn spawn_lasting(cmd: Command) -> std::io::Result<std::process::Child> {
    type Job = (
        Command,
        std::sync::mpsc::Sender<std::io::Result<std::process::Child>>,
    );
    static SPAWNER: std::sync::OnceLock<Mutex<std::sync::mpsc::Sender<Job>>> =
        std::sync::OnceLock::new();
    let tx = SPAWNER.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        let _ = std::thread::Builder::new()
            .name("afm-spawner".into())
            .spawn(move || {
                for (mut cmd, answer) in rx {
                    let _ = answer.send(cmd.spawn());
                }
            });
        Mutex::new(tx)
    });
    let (atx, arx) = channel();
    tx.lock()
        .unwrap_or_else(|p| p.into_inner())
        .send((cmd, atx))
        .map_err(|_| std::io::Error::other("the spawner is gone"))?;
    arx.recv()
        .map_err(|_| std::io::Error::other("the spawner is gone"))?
}

#[cfg(not(target_os = "linux"))]
fn spawn_lasting(mut cmd: Command) -> std::io::Result<std::process::Child> {
    cmd.spawn()
}

/// One line read from the server.
#[derive(Debug, PartialEq)]
pub enum Line {
    /// A whole line (without its newline).
    Text(Vec<u8>),
    /// A line longer than the bound: dropped, read past in small pieces.
    TooLong,
    End,
}

/// Reads one line of at most `max` bytes. A longer one is read past in
/// the reader's own pieces and dropped - the memory stays bounded however
/// long it is - and `too_long` hears of it the moment it passes the bound
/// (not at its end, which may never come).
pub fn read_line_bounded(
    r: &mut impl BufRead,
    line: &mut Vec<u8>,
    max: usize,
    mut too_long: impl FnMut(),
) -> std::io::Result<Line> {
    line.clear();
    let mut over = false;
    loop {
        let chunk = r.fill_buf()?;
        if chunk.is_empty() {
            return Ok(if over {
                Line::TooLong
            } else if line.is_empty() {
                Line::End
            } else {
                Line::Text(std::mem::take(line))
            });
        }
        let (part, used, done) = match chunk.iter().position(|&c| c == b'\n') {
            Some(i) => (&chunk[..i], i + 1, true),
            None => (chunk, chunk.len(), false),
        };
        if !over {
            if line.len() + part.len() > max {
                over = true;
                line.clear();
                line.shrink_to(64 << 10);
                too_long();
            } else {
                line.extend_from_slice(part);
            }
        }
        r.consume(used);
        if done {
            return Ok(if over {
                Line::TooLong
            } else {
                Line::Text(std::mem::take(line))
            });
        }
    }
}

/// Answers that could not be read (too long, no JSON) before the server is
/// taken for broken and ended.
const BAD_LINES: u32 = 3;

fn fail_all(waiters: &Waiters, why: &str) {
    let waiting: Vec<_> = waiters
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .drain()
        .collect();
    for (_, tx) in waiting {
        let _ = tx.send(Err(why.to_string()));
    }
}

fn read_loop(reader: impl Read, waiters: Waiters, alive: Arc<AtomicBool>, writer: Shared) {
    let mut r = BufReader::with_capacity(64 << 10, reader);
    let mut line = Vec::new();
    let mut bad = 0u32;
    loop {
        // (an answer over the bound fails what waits for it at once - no
        // waiting out its timeout - and is read past without keeping it)
        let read = read_line_bounded(&mut r, &mut line, MAX_LINE, || {
            fail_all(&waiters, "the server's answer is over 32 MiB: refused")
        });
        let text = match read {
            Ok(Line::Text(t)) => t,
            Ok(Line::TooLong) => {
                bad += 1;
                if bad >= BAD_LINES {
                    break;
                }
                continue;
            }
            Ok(Line::End) | Err(_) => break,
        };
        if text.iter().all(|c| c.is_ascii_whitespace()) {
            continue;
        }
        let Ok(msg) = serde_json::from_slice::<Value>(&text) else {
            // no JSON: whatever waits fails now, not at its timeout
            fail_all(
                &waiters,
                "the server answered with something that is no JSON",
            );
            bad += 1;
            if bad >= BAD_LINES {
                break;
            }
            continue;
        };
        if let (Some(method), Some(id)) = (msg.get("method").and_then(Value::as_str), msg.get("id"))
        {
            // a request from the server: a ping is answered, anything else
            // refused (this client offers nothing)
            let answer = if method == "ping" {
                json!({"jsonrpc": "2.0", "id": id, "result": {}})
            } else {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "not offered"}})
            };
            let mut w = writer.lock().unwrap_or_else(|p| p.into_inner());
            let _ = writeln!(w, "{answer}").and_then(|_| w.flush());
            continue;
        }
        let Some(id) = msg.get("id").and_then(Value::as_u64) else {
            // a notification (progress, a log line): nothing to do
            continue;
        };
        let tx = waiters
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id);
        if let Some(tx) = tx {
            let answer = if let Some(e) = msg.get("error") {
                Err(e["message"]
                    .as_str()
                    .unwrap_or("error")
                    .chars()
                    .take(500)
                    .collect())
            } else {
                Ok(msg.get("result").cloned().unwrap_or(Value::Null))
            };
            let _ = tx.send(answer);
        }
    }
    alive.store(false, Ordering::Relaxed);
    // everyone still waiting hears it now, not at their timeout
    fail_all(&waiters, "the server has ended");
}

fn parse_backend(v: &Value) -> Option<Backend> {
    Some(Backend {
        id: v["id"].as_str()?.to_string(),
        class: v["class"].as_str().unwrap_or("").to_string(),
        name: v["name"].as_str().unwrap_or("").to_string(),
        remote: v["remote"].as_bool().unwrap_or(false),
        is_default: v["is_default"].as_bool().unwrap_or(false),
        profile: v["profile"].as_str().map(String::from),
        protocol: v["protocol"].as_str().map(String::from),
        production: v["production"].as_bool().unwrap_or(false),
    })
}

fn parse_entry(v: &Value) -> Option<RemoteEntry> {
    let name = match v["name_b64"].as_str().and_then(b64_decode) {
        Some(raw) => raw,
        None => v["name"].as_str()?.as_bytes().to_vec(),
    };
    let path = match v["path_b64"].as_str().and_then(b64_decode) {
        Some(raw) => String::from_utf8_lossy(&raw).into_owned(),
        None => v["path"].as_str().unwrap_or("").to_string(),
    };
    if !safe_name(&name) {
        return None;
    }
    Some(RemoteEntry {
        path,
        name,
        is_dir: v["is_dir"].as_bool().unwrap_or(false),
        is_link: v["is_link"].as_bool().unwrap_or(false),
        size: v["size"].as_u64().unwrap_or(0),
        size_known: v["size"].is_u64(),
        mtime: v["modified"].as_str().map_or(0, parse_iso8601),
    })
}

/// A name a server may give for an entry: not empty, not `.` or `..`, no
/// `/` (it would reach outside its folder), no NUL and no control
/// character. Others are left out of the listing (and counted).
pub fn safe_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name != b"."
        && name != b".."
        && name.len() <= 4096
        && !name.iter().any(|&c| c == b'/' || c < 0x20 || c == 0x7f)
}

/// Seconds since 1970 of an ISO 8601 time (`2026-11-23T10:45:55.03`,
/// with `Z` or `+01:00`, or none: taken as UTC); 0 when it does not parse.
pub fn parse_iso8601(s: &str) -> i64 {
    let b = s.as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> {
        let part = s.get(from..from + len)?;
        if !part.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let parse = || -> Option<i64> {
        let (y, mo, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
        if b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') {
            return None;
        }
        let (mut h, mut mi, mut se) = (0, 0, 0);
        let mut rest = 10;
        if matches!(b.get(10), Some(b'T') | Some(b' ')) {
            h = num(11, 2)?;
            mi = num(14, 2)?;
            se = num(17, 2).unwrap_or(0);
            rest = if num(17, 2).is_some() { 19 } else { 16 };
        }
        // fractions
        if b.get(rest) == Some(&b'.') {
            rest += 1;
            while b.get(rest).is_some_and(|c| c.is_ascii_digit()) {
                rest += 1;
            }
        }
        let mut offset = 0;
        match b.get(rest) {
            Some(b'+') | Some(b'-') => {
                let sign = if b[rest] == b'-' { -1 } else { 1 };
                let oh = num(rest + 1, 2)?;
                let om = num(rest + 4, 2).or_else(|| num(rest + 3, 2)).unwrap_or(0);
                offset = sign * (oh * 3600 + om * 60);
            }
            _ => {}
        }
        if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
            return None;
        }
        Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se - offset)
    };
    parse().unwrap_or(0)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let mut val = [255u8; 256];
    for (i, &c) in B64.iter().enumerate() {
        val[c as usize] = i as u8;
    }
    let bytes: Vec<u8> = text.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (k, chunk) in bytes.chunks(4).enumerate() {
        let last = k == bytes.len() / 4 - 1;
        let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for &c in &chunk[..4 - pad] {
            let v = val[c as usize];
            if v == 255 {
                return None;
            }
            n = n << 6 | v as u32;
        }
        n <<= 6 * pad as u32;
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// A program on the PATH (with `.exe` on Windows), or None.
pub fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    std::env::split_paths(&path)
        .filter(|d| d.is_absolute())
        .map(|d| d.join(&exe))
        .find(|p| p.is_file())
}

/// How the server is started (the first that exists), with what it is
/// called in the sources panel.
pub fn find_server(
    configured: Option<&[String]>,
    allow_uvx_download: bool,
) -> Option<(Vec<OsString>, String)> {
    if let Some(cmd) = configured.filter(|c| !c.is_empty() && !c[0].trim().is_empty()) {
        let label = cmd.join(" ");
        return Some((cmd.iter().map(OsString::from).collect(), label));
    }
    let tools: [OsString; 2] = ["--mcp-tools".into(), "remote".into()];
    if let Some(p) = on_path("axross-headless-mcp") {
        let mut c = vec![p.into_os_string()];
        c.extend(tools.iter().cloned());
        return Some((c, "axross-headless-mcp".into()));
    }
    if let Some(p) = on_path("axross") {
        let mut c = vec![p.into_os_string(), "--mcp-server".into()];
        c.extend(tools.iter().cloned());
        return Some((c, "axross --mcp-server".into()));
    }
    if let Some(p) = on_path("uvx") {
        let mut c = vec![p.into_os_string()];
        if !allow_uvx_download {
            c.push("--offline".into());
        }
        c.push("axross-headless-mcp".into());
        c.extend(tools.iter().cloned());
        return Some((c, "uvx axross-headless-mcp".into()));
    }
    None
}

/// The same command without `--mcp-tools remote` (a server that does not
/// know the group ends at once: tried again without it).
pub fn without_groups(command: &[OsString]) -> Vec<OsString> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in command {
        if skip {
            skip = false;
            continue;
        }
        if a == "--mcp-tools" {
            skip = true;
            continue;
        }
        out.push(a.clone());
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    /// A fake axross server on a thread: answers initialize, tools/list
    /// and the read-only tools from a small tree, with a progress
    /// notification and a server request thrown in.
    pub fn fake_server(public_names: bool) -> Client {
        let (to_server_r, to_server_w) = std::io::pipe().unwrap();
        let (from_server_r, from_server_w) = std::io::pipe().unwrap();
        std::thread::spawn(move || serve(to_server_r, from_server_w, public_names));
        Client::over(from_server_r, to_server_w, Duration::from_secs(5)).unwrap()
    }

    fn serve(input: std::io::PipeReader, mut out: std::io::PipeWriter, public_names: bool) {
        let names: Vec<&str> = if public_names {
            vec![
                "list_directory",
                "walk_tree",
                "get_path_info",
                "read_file",
                "list_backends",
                "list_remote_profiles",
                "open_remote_profile",
            ]
        } else {
            vec!["list_dir", "walk", "stat", "read_file", "list_backends"]
        };
        let tree: Vec<(&str, bool, u64)> = vec![
            ("/a", true, 0),
            ("/a/x.txt", false, 11),
            ("/a/deep", true, 0),
            ("/a/deep/y.bin", false, 2048),
            ("/z.png", false, 99),
        ];
        let entry = |p: &str, d: bool, s: u64| {
            let name = p.rsplit('/').next().unwrap();
            json!({"path": p, "name": name, "is_dir": d, "is_link": false, "size": s, "modified": "2026-11-23T10:00:00"})
        };
        for line in BufReader::new(input).lines() {
            let Ok(line) = line else { break };
            let m: Value = serde_json::from_str(&line).unwrap();
            let Some(id) = m.get("id").cloned() else {
                continue;
            };
            // (the client's answer to the ping)
            let Some(method) = m["method"].as_str() else {
                continue;
            };
            let wrap = |v: Value| json!({"content": [{"type": "text", "text": v.to_string()}], "isError": false});
            let result = match method {
                "initialize" => {
                    json!({"protocolVersion": PROTOCOL, "serverInfo": {"name": "axross-mcp", "version": "9.9.9"}, "capabilities": {"tools": {}}})
                }
                "tools/list" => {
                    json!({"tools": names.iter().map(|n| json!({"name": n})).collect::<Vec<_>>()})
                }
                "tools/call" => {
                    let name = m["params"]["name"].as_str().unwrap_or("");
                    let args = &m["params"]["arguments"];
                    // noise first: a progress notification and a ping
                    writeln!(
                        out,
                        "{}",
                        json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {}})
                    )
                    .unwrap();
                    writeln!(
                        out,
                        "{}",
                        json!({"jsonrpc": "2.0", "id": 9999, "method": "ping"})
                    )
                    .unwrap();
                    match name {
                        "list_backends" => {
                            let list = json!([{"id": "local", "class": "LocalFS", "name": "Local", "is_default": true, "remote": false},
                                              {"id": "sftp-1", "class": "SFTPBackend", "name": "box", "is_default": false, "remote": true, "profile": "box", "protocol": "sftp", "production": true}]);
                            let mut r = wrap(list.clone());
                            if public_names {
                                r["structuredContent"] = json!({"backends": list});
                            }
                            r
                        }
                        "list_directory" | "list_dir" => {
                            let path = args["path"].as_str().unwrap_or("/");
                            if path == "/locked" {
                                json!({"content": [{"type": "text", "text": "permission denied"}], "isError": true})
                            } else {
                                let prefix = if path == "/" {
                                    "/".to_string()
                                } else {
                                    format!("{path}/")
                                };
                                let list: Vec<Value> = tree
                                    .iter()
                                    .filter(|(p, _, _)| {
                                        p.starts_with(&prefix) && !p[prefix.len()..].contains('/')
                                    })
                                    .map(|(p, d, s)| {
                                        let mut e = entry(p, *d, *s);
                                        e.as_object_mut().unwrap().remove("path");
                                        e
                                    })
                                    .collect();
                                wrap(json!(list))
                            }
                        }
                        "walk_tree" | "walk" => {
                            let max_entries = args["max_entries"].as_u64().unwrap_or(1000) as usize;
                            let offset = args["offset"].as_u64().unwrap_or(0) as usize;
                            let all: Vec<Value> =
                                tree.iter().map(|(p, d, s)| entry(p, *d, *s)).collect();
                            let page: Vec<Value> =
                                all.iter().skip(offset).take(max_entries).cloned().collect();
                            let next = if offset + page.len() < all.len() {
                                json!(offset + page.len())
                            } else {
                                Value::Null
                            };
                            wrap(
                                json!({"entries": page, "truncated": !next.is_null(), "unreadable": [{"path": "/locked", "error": "denied"}], "complete": false, "offset": offset, "next_offset": next}),
                            )
                        }
                        "read_file" => {
                            let max = args["max_bytes"].as_u64().unwrap_or(4 << 20) as usize;
                            let data = b"hello\x00world, a file of some bytes";
                            wrap(
                                json!({"content_b64": b64_encode(&data[..max.min(data.len())]), "size": max.min(data.len()), "truncated": max < data.len()}),
                            )
                        }
                        "list_remote_profiles" => wrap(json!({"profiles": [{"name": "box"}]})),
                        _ => {
                            json!({"content": [{"type": "text", "text": "unknown"}], "isError": true})
                        }
                    }
                }
                _ => {
                    writeln!(out, "{}", json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "no such method"}})).unwrap();
                    continue;
                }
            };
            writeln!(
                out,
                "{}",
                json!({"jsonrpc": "2.0", "id": id, "result": result})
            )
            .unwrap();
        }
    }

    #[test]
    fn talks_to_a_server_with_the_current_names() {
        let c = fake_server(true);
        assert_eq!(c.server, "axross-mcp");
        assert_eq!(c.version, "9.9.9");
        assert_eq!(c.tool_name(Tool::Walk), Some("walk_tree"));
        let b = c.backends(Duration::from_secs(5)).unwrap();
        assert_eq!(b.len(), 2);
        assert!(b[1].remote && b[1].production);
        assert_eq!(b[1].profile.as_deref(), Some("box"));
        let list = c
            .list("local", &PathArg::Text("/a".into()), Duration::from_secs(5))
            .unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, b"x.txt");
        assert_eq!(list[0].mtime, parse_iso8601("2026-11-23T10:00:00"));
        let page = c
            .walk(
                "local",
                &PathArg::Text("/".into()),
                2,
                3,
                0,
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(page.entries.len(), 3);
        assert_eq!(page.next_offset, Some(3));
        assert_eq!(page.unreadable, vec!["/locked".to_string()]);
        let rest = c
            .walk(
                "local",
                &PathArg::Text("/".into()),
                2,
                3,
                3,
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(rest.entries.len(), 2);
        assert_eq!(rest.next_offset, None);
        let bytes = c
            .read(
                "local",
                &PathArg::Text("/z.png".into()),
                7,
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(bytes, b"hello\x00w");
        let err = c
            .list(
                "local",
                &PathArg::Text("/locked".into()),
                Duration::from_secs(5),
            )
            .unwrap_err();
        assert!(err.contains("permission denied"));
        assert_eq!(
            c.profiles(Duration::from_secs(5)).unwrap(),
            vec!["box".to_string()]
        );
        // an unknown method is an error, not a hang
        assert!(
            c.request("nope", json!({}), Duration::from_secs(5))
                .is_err()
        );
    }

    #[test]
    fn talks_to_an_older_server_by_the_former_names() {
        let c = fake_server(false);
        assert_eq!(c.tool_name(Tool::ListDirectory), Some("list_dir"));
        assert_eq!(c.tool_name(Tool::Walk), Some("walk"));
        assert!(!c.has(Tool::ListProfiles));
        assert!(c.profiles(Duration::from_secs(5)).unwrap().is_empty());
        let list = c
            .list("local", &PathArg::Text("/".into()), Duration::from_secs(5))
            .unwrap();
        let names: Vec<_> = list
            .iter()
            .map(|e| String::from_utf8_lossy(&e.name).into_owned())
            .collect();
        assert_eq!(names, ["a", "z.png"]);
        let b = c.backends(Duration::from_secs(5)).unwrap();
        assert_eq!(b[0].id, "local");
    }

    #[test]
    fn a_server_that_ends_fails_the_waiting_calls_at_once() {
        let (to_server_r, to_server_w) = std::io::pipe().unwrap();
        let (from_server_r, from_server_w) = std::io::pipe().unwrap();
        std::thread::spawn(move || {
            // answers initialize and tools/list, then ends
            let mut out = from_server_w;
            let mut lines = BufReader::new(to_server_r).lines();
            for _ in 0..3 {
                let Some(Ok(line)) = lines.next() else { return };
                let m: Value = serde_json::from_str(&line).unwrap();
                if let Some(id) = m.get("id") {
                    let result = if m["method"] == "initialize" {
                        json!({"serverInfo": {"name": "x"}})
                    } else {
                        json!({"tools": [{"name": "list_backends"}]})
                    };
                    writeln!(
                        out,
                        "{}",
                        json!({"jsonrpc": "2.0", "id": id, "result": result})
                    )
                    .unwrap();
                }
            }
        });
        let c = Client::over(from_server_r, to_server_w, Duration::from_secs(5)).unwrap();
        let started = std::time::Instant::now();
        let r = c.backends(Duration::from_secs(30));
        assert!(r.is_err());
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_program_that_is_no_server_fails_quietly() {
        let missing = [OsString::from("/nonexistent/axross-headless-mcp")];
        assert!(Client::spawn(&missing, Duration::from_secs(2)).is_err());
    }

    #[test]
    fn iso_times() {
        assert_eq!(parse_iso8601("1970-01-01T00:00:00"), 0);
        assert_eq!(parse_iso8601("2000-03-01T00:00:00Z"), 951_868_800);
        assert_eq!(parse_iso8601("2000-03-01T01:00:00+01:00"), 951_868_800);
        assert_eq!(parse_iso8601("2026-11-23T10:45:55.036757"), 1_795_430_755);
        assert_eq!(parse_iso8601("2026-11-23"), 1_795_392_000);
        assert_eq!(parse_iso8601("garbage"), 0);
        assert_eq!(parse_iso8601("2026-13-01T00:00:00"), 0);
    }

    #[test]
    fn base64_round_trips_and_refuses_junk() {
        for data in [&b""[..], b"a", b"ab", b"abc", b"\xff\xfe\x00 caf\xe9"] {
            assert_eq!(b64_decode(&b64_encode(data)).unwrap(), data);
        }
        assert_eq!(b64_encode(b"abc"), "YWJj");
        assert!(b64_decode("YW=j").is_none());
        assert!(b64_decode("Y").is_none());
        assert!(b64_decode("YW*j").is_none());
    }

    #[test]
    fn groups_can_be_left_out() {
        let c: Vec<OsString> = ["x", "--mcp-tools", "remote", "--y"]
            .iter()
            .map(OsString::from)
            .collect();
        assert_eq!(
            without_groups(&c),
            vec![OsString::from("x"), OsString::from("--y")]
        );
    }

    /// A server whose tools answer as `answer` says (by the tool's name and
    /// arguments; a `tools/call` result).
    pub fn tool_server(answer: fn(&str, &Value) -> Value) -> Client {
        let (to_server_r, to_server_w) = std::io::pipe().unwrap();
        let (from_server_r, from_server_w) = std::io::pipe().unwrap();
        std::thread::spawn(move || {
            let mut out = from_server_w;
            for line in BufReader::new(to_server_r).lines() {
                let Ok(line) = line else { return };
                let m: Value = serde_json::from_str(&line).unwrap();
                let Some(id) = m.get("id").cloned() else {
                    continue;
                };
                let result = match m["method"].as_str() {
                    Some("initialize") => json!({"serverInfo": {"name": "tools"}}),
                    Some("tools/list") => {
                        json!({"tools": [{"name": "walk_tree"}, {"name": "list_directory"}, {"name": "list_backends"}, {"name": "read_file"}, {"name": "get_path_info"}]})
                    }
                    _ => answer(
                        m["params"]["name"].as_str().unwrap_or(""),
                        &m["params"]["arguments"],
                    ),
                };
                if writeln!(
                    out,
                    "{}",
                    json!({"jsonrpc": "2.0", "id": id, "result": result})
                )
                .is_err()
                {
                    return;
                }
            }
        });
        Client::over(from_server_r, to_server_w, Duration::from_secs(5)).unwrap()
    }

    /// A reader of `n` bytes of `x` and then `tail` (made as it is read).
    struct Endless {
        left: u64,
        tail: std::io::Cursor<Vec<u8>>,
    }

    impl Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.left == 0 {
                return self.tail.read(buf);
            }
            let n = (buf.len() as u64).min(self.left) as usize;
            buf[..n].fill(b'x');
            self.left -= n as u64;
            Ok(n)
        }
    }

    #[test]
    fn an_endless_line_is_read_past_in_bounded_memory() {
        let mut r = BufReader::with_capacity(
            64 << 10,
            Endless {
                left: 50 << 20,
                tail: std::io::Cursor::new(b"\n{\"ok\": 1}\n".to_vec()),
            },
        );
        let mut line = Vec::new();
        let mut heard = 0;
        let first = read_line_bounded(&mut r, &mut line, 1 << 20, || heard += 1).unwrap();
        assert_eq!(first, Line::TooLong);
        assert_eq!(heard, 1, "said the moment it passed the bound");
        assert!(
            line.capacity() <= (1 << 20) + (64 << 10),
            "{}",
            line.capacity()
        );
        let next = read_line_bounded(&mut r, &mut line, 1 << 20, || {}).unwrap();
        assert_eq!(next, Line::Text(b"{\"ok\": 1}".to_vec()));
        assert_eq!(
            read_line_bounded(&mut r, &mut line, 1 << 20, || {}).unwrap(),
            Line::End
        );
    }

    /// A server that answers the handshake, then answers every call with
    /// `answer` (bytes written as they are).
    fn server_answering(answer: fn(u64) -> Vec<u8>) -> Client {
        let (to_server_r, to_server_w) = std::io::pipe().unwrap();
        let (from_server_r, from_server_w) = std::io::pipe().unwrap();
        std::thread::spawn(move || {
            let mut out = from_server_w;
            for line in BufReader::new(to_server_r).lines() {
                let Ok(line) = line else { return };
                let m: Value = serde_json::from_str(&line).unwrap();
                let Some(id) = m.get("id").and_then(Value::as_u64) else {
                    continue;
                };
                let reply = match m["method"].as_str() {
                    Some("initialize") => json!({"jsonrpc": "2.0", "id": id, "result": {"serverInfo": {"name": "x"}}}).to_string().into_bytes(),
                    Some("tools/list") => json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [{"name": "list_backends"}]}}).to_string().into_bytes(),
                    _ => answer(id),
                };
                if out
                    .write_all(&reply)
                    .and_then(|_| out.write_all(b"\n"))
                    .is_err()
                {
                    return;
                }
            }
        });
        Client::over(from_server_r, to_server_w, Duration::from_secs(5)).unwrap()
    }

    #[test]
    fn an_oversize_answer_fails_the_call_at_once() {
        let c = server_answering(|_| vec![b'x'; MAX_LINE + 4096]);
        let t = std::time::Instant::now();
        let e = c.backends(Duration::from_secs(60)).unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
        assert!(e.contains("32 MiB"), "{e}");
    }

    #[test]
    fn an_answer_that_is_no_json_fails_the_call_at_once() {
        let c = server_answering(|_| b"this is { not json".to_vec());
        let t = std::time::Instant::now();
        let e = c.backends(Duration::from_secs(60)).unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
        assert!(e.contains("no JSON"), "{e}");
    }

    #[test]
    fn names_that_reach_outside_are_refused() {
        for bad in [
            &b"../../etc/passwd"[..],
            b"/etc/shadow",
            b"a/b/c",
            b"..",
            b".",
            b"",
            b"a\nb",
            b"nul\0x",
            b"del\x7f",
        ] {
            assert!(!safe_name(bad), "{bad:?}");
        }
        for good in [
            &b"x.txt"[..],
            b"..hidden",
            b"bad\xff\xfe.txt",
            "café".as_bytes(),
        ] {
            assert!(safe_name(good), "{good:?}");
        }
        let list = json!([
            {"name": "../../etc/passwd", "path": "/x"},
            {"name": "ok.txt", "path": "/ok.txt", "size": 3},
            {"name": "a/b", "path": "/a/b"},
        ]);
        let (entries, rejected) = parse_entries(list.as_array());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, b"ok.txt");
        assert_eq!(rejected, 2);
    }

    /// A server as a program: answers the handshake (ids 1 and 2), then
    /// hangs in every call - with a child of its own.
    #[cfg(unix)]
    fn hanging_server() -> Vec<OsString> {
        let script = r#"read a; printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"hang"}}}'; read b; read c; printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"list_backends"}]}}'; sleep 600 & wait"#;
        vec!["/bin/sh".into(), "-c".into(), script.into()]
    }

    #[cfg(unix)]
    fn gone(pid: i32) -> bool {
        // SAFETY: signal 0 only asks whether the process (group) exists
        unsafe { libc::kill(pid, 0) != 0 }
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_reaps_the_server_with_a_call_pending() {
        let c = Arc::new(Client::spawn(&hanging_server(), Duration::from_secs(10)).unwrap());
        let pid = c.pid().unwrap() as i32;
        let c2 = c.clone();
        let call = std::thread::spawn(move || {
            let t = std::time::Instant::now();
            (c2.backends(Duration::from_secs(120)), t.elapsed())
        });
        std::thread::sleep(Duration::from_millis(300));
        c.shutdown();
        let (r, took) = call.join().unwrap();
        assert!(r.is_err());
        assert!(took < Duration::from_secs(10), "{took:?}");
        assert!(gone(pid), "the server is reaped");
        // (the group's other members are killed too; they are reaped by
        // init, which may take a moment)
        let start = std::time::Instant::now();
        while !gone(-pid) && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(gone(-pid), "and its group (the sleep it started)");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_server_outlives_the_thread_that_started_it() {
        // (the parent-death signal follows the thread: started from one
        // that ends, the server must still live)
        let c = std::thread::spawn(|| {
            Client::spawn(&hanging_server(), Duration::from_secs(10)).unwrap()
        })
        .join()
        .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let pid = c.pid().unwrap() as i32;
        assert!(!gone(pid), "still running");
        assert!(c.is_alive());
        c.shutdown();
        assert!(gone(pid));
    }
}
