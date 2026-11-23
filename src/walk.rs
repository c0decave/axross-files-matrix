//! The walks: one coordinator thread per island, streaming what it finds to
//! the window in batches (the window applies them in order; the
//! coordinator gives out the node ids). Local disks are read by a few
//! worker threads at once, breadth-first; symbolic links are listed, never
//! followed (no loops); another file system's mountpoint is not entered
//! (its own island); a folder that cannot be read is marked, not an error
//! message; a huge folder is read up to a cap; names are kept as the system
//! gives them. Past the entry budget - and on network file systems and
//! remote backends past the first levels - folders are read when asked
//! ([`Walker::list`]): when the camera comes near.

use crate::mcp::{Client, PathArg, RemoteEntry};
use crate::tree::{Batch, Entry, Kind, Listing, ListingUpdate, NodeId};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What a walk reads.
#[derive(Clone, Debug)]
pub enum Source {
    /// A local folder (a mountpoint): `skip` are the mountpoints below it
    /// (islands of their own); `auto_depth` how deep it is read by itself
    /// (None: all of it, within the budget).
    Local {
        root: PathBuf,
        skip: Vec<PathBuf>,
        auto_depth: Option<u32>,
    },
    /// A backend of axross's MCP server.
    Mcp {
        client: Arc<Client>,
        backend: String,
        root: String,
        /// Folder by folder, only as they are approached (a backend that
        /// lists slowly or cannot walk: mail, databases, ...).
        lazy: bool,
    },
    /// A made-up tree of `files` files (the frame-time measurements, the
    /// screenshots, `--synthetic`).
    Synthetic { files: usize, seed: u64 },
}

/// How much a walk reads by itself.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Entries read without being asked (past it: on demand).
    pub budget: usize,
    /// Entries of one folder at most (past it: read in part).
    pub per_dir: usize,
    /// Entries in all, asked for or not.
    pub hard_cap: usize,
    /// Folders deeper than this are not entered.
    pub max_depth: u32,
    /// Worker threads reading local folders.
    pub workers: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            budget: 1_500_000,
            // (no folder is cut: a huge one is read in pieces)
            per_dir: usize::MAX,
            hard_cap: 8_000_000,
            max_depth: u32::MAX,
            workers: 4,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub entries: u64,
    pub folders_left: usize,
    pub done: bool,
    /// The walk could not start (the root is gone, the server ended).
    pub error: Option<String>,
    pub seconds: f32,
    /// Entries a server named unsafely (`/`, `..`, control characters):
    /// left out, and said so.
    pub skipped_names: u64,
    /// The walk stopped short of everything (the entry limit): said so.
    pub note: Option<String>,
    /// Files whose size the backend did not tell (they get equal
    /// footprints; said on the island).
    pub unknown_sizes: u64,
    /// Read folder by folder as they are approached (a slow backend).
    pub lazy: bool,
}

#[derive(Debug)]
pub enum Event {
    Batch(Batch),
    Status(Status),
}

#[derive(Debug)]
pub struct Msg {
    pub island: u64,
    pub generation: u32,
    pub event: Event,
}

pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// A running walk; dropped, it stops.
pub struct Walker {
    requests: Sender<NodeId>,
    cancel: Arc<AtomicBool>,
}

impl Walker {
    pub fn start(
        island: u64,
        generation: u32,
        source: Source,
        limits: Limits,
        tx: Sender<Msg>,
        wake: Wake,
    ) -> Walker {
        let (req_tx, req_rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let out = Out {
            island,
            generation,
            tx,
            wake,
            batch: Batch {
                first_id: 1,
                ..Default::default()
            },
            next_id: 1,
            last_flush: Instant::now(),
            last_status: Instant::now(),
            started: Instant::now(),
            entries: 0,
            cancel: cancel.clone(),
            skipped_names: 0,
            note: None,
            unknown_sizes: 0,
            lazy: false,
        };
        let c = cancel.clone();
        let _ = std::thread::Builder::new()
            .name(format!("afm-walk-{island:x}"))
            .spawn(move || match source {
                Source::Local {
                    root,
                    skip,
                    auto_depth,
                } => walk_local(root, skip, auto_depth, limits, out, req_rx, c),
                Source::Mcp {
                    client,
                    backend,
                    root,
                    lazy,
                } => walk_mcp(client, backend, root, lazy, limits, out, req_rx, c),
                Source::Synthetic { files, seed } => synthetic(files, seed, out),
            });
        Walker {
            requests: req_tx,
            cancel,
        }
    }

    /// Asks for a folder to be read (when the camera came near).
    pub fn list(&self, dir: NodeId) {
        let _ = self.requests.send(dir);
    }

    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for Walker {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The stream to the window: entries batched, ids given out here.
struct Out {
    island: u64,
    generation: u32,
    tx: Sender<Msg>,
    wake: Wake,
    batch: Batch,
    next_id: NodeId,
    last_flush: Instant,
    last_status: Instant,
    started: Instant,
    entries: u64,
    cancel: Arc<AtomicBool>,
    skipped_names: u64,
    note: Option<String>,
    unknown_sizes: u64,
    lazy: bool,
}

const BATCH: usize = 4096;
const FLUSH_EVERY: Duration = Duration::from_millis(40);

impl Out {
    fn push(&mut self, e: Entry) -> NodeId {
        let id = self.next_id;
        self.next_id += 1;
        self.entries += 1;
        self.batch.entries.push(e);
        id
    }
    fn listing(&mut self, dir: NodeId, listing: Listing) {
        self.batch.listings.push(ListingUpdate { dir, listing });
    }
    /// Sends the batch when it is big or old enough (`force`: now).
    fn flush(&mut self, force: bool) -> bool {
        let due = self.batch.entries.len() >= BATCH || self.last_flush.elapsed() >= FLUSH_EVERY;
        if (force || due) && (!self.batch.entries.is_empty() || !self.batch.listings.is_empty()) {
            let next = Batch {
                first_id: self.next_id,
                entries: Vec::with_capacity(256),
                listings: Vec::new(),
            };
            let batch = std::mem::replace(&mut self.batch, next);
            if self
                .tx
                .send(Msg {
                    island: self.island,
                    generation: self.generation,
                    event: Event::Batch(batch),
                })
                .is_err()
            {
                self.cancel.store(true, Ordering::Relaxed);
                return false;
            }
            self.last_flush = Instant::now();
            (self.wake)();
        }
        true
    }
    fn status(&mut self, folders_left: usize, done: bool, error: Option<String>, force: bool) {
        if !force && self.last_status.elapsed() < Duration::from_millis(250) {
            return;
        }
        self.last_status = Instant::now();
        let _ = self.tx.send(Msg {
            island: self.island,
            generation: self.generation,
            event: Event::Status(Status {
                entries: self.entries,
                folders_left,
                done,
                error,
                seconds: self.started.elapsed().as_secs_f32(),
                skipped_names: self.skipped_names,
                note: self.note.clone(),
                unknown_sizes: self.unknown_sizes,
                lazy: self.lazy,
            }),
        });
        (self.wake)();
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

fn mtime_of(m: &std::fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(unix)]
fn device_of(m: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    m.dev()
}
#[cfg(not(unix))]
fn device_of(_m: &std::fs::Metadata) -> u64 {
    0
}

/// One entry of a local folder.
struct Found {
    name: OsString,
    kind: Kind,
    size: u64,
    alloc: u64,
    mtime: i64,
    /// A folder on another device (a mountpoint).
    foreign: bool,
}

#[cfg(unix)]
fn alloc_of(m: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    m.blocks().saturating_mul(512)
}
#[cfg(not(unix))]
fn alloc_of(m: &std::fs::Metadata) -> u64 {
    m.len()
}

/// The entries of a local folder are handed on in pieces of this many: a
/// folder of millions streams like a tree of them.
const PIECE: usize = 8192;

/// Reads one local folder, handing its entries on in pieces (`piece(found,
/// more)`; false from it stops): at most `cap`, and whether it was cut.
fn read_local(
    path: &Path,
    cap: usize,
    device: Option<u64>,
    mut piece: impl FnMut(Vec<Found>, bool) -> bool,
) -> std::io::Result<bool> {
    let mut out = Vec::new();
    let mut seen = 0usize;
    let mut cut = false;
    for e in std::fs::read_dir(path)? {
        let Ok(e) = e else { continue };
        if seen >= cap {
            cut = true;
            break;
        }
        seen += 1;
        let name = e.file_name();
        // (lstat: a link is the link)
        let meta = e.metadata();
        let ft = match &meta {
            Ok(m) => Some(m.file_type()),
            Err(_) => e.file_type().ok(),
        };
        let kind = match ft {
            Some(t) if t.is_symlink() => Kind::Link,
            Some(t) if t.is_dir() => Kind::Dir,
            Some(t) if t.is_file() => Kind::File,
            Some(_) => Kind::Other,
            None => Kind::Other,
        };
        let (size, alloc, mtime, foreign) = match &meta {
            Ok(m) => (
                if kind == Kind::Dir { 0 } else { m.len() },
                if kind == Kind::Dir { 0 } else { alloc_of(m) },
                mtime_of(m),
                kind == Kind::Dir && device.is_some_and(|d| d != device_of(m)),
            ),
            Err(_) => (0, 0, 0, false),
        };
        out.push(Found {
            name,
            kind,
            size,
            alloc,
            mtime,
            foreign,
        });
        if out.len() >= PIECE && !piece(std::mem::take(&mut out), true) {
            return Ok(true);
        }
    }
    piece(out, false);
    Ok(cut)
}

fn listing_of(e: &std::io::Error) -> Listing {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        Listing::Denied
    } else {
        Listing::Failed
    }
}

struct Job {
    dir: NodeId,
    path: PathBuf,
}

struct Done {
    dir: NodeId,
    /// A piece of its entries, and whether more follow; or how it ended
    /// (Ok(cut) after the last piece, or the error).
    found: Vec<Found>,
    more: bool,
    result: Option<std::io::Result<bool>>,
}

#[allow(clippy::too_many_arguments)]
fn walk_local(
    root: PathBuf,
    skip: Vec<PathBuf>,
    auto_depth: Option<u32>,
    limits: Limits,
    mut out: Out,
    requests: Receiver<NodeId>,
    cancel: Arc<AtomicBool>,
) {
    let device = match std::fs::metadata(&root) {
        Ok(m) if m.is_dir() => cfg!(unix).then(|| device_of(&m)),
        Ok(_) => {
            out.status(0, true, Some("not a folder".into()), true);
            return;
        }
        Err(e) => {
            out.listing(0, listing_of(&e));
            out.flush(true);
            out.status(0, true, Some(e.to_string()), true);
            return;
        }
    };
    let skip: HashSet<PathBuf> = skip.into_iter().collect();
    // the folders known: their path and depth (for reading them later)
    let mut dirs: HashMap<NodeId, (PathBuf, u32)> = HashMap::new();
    dirs.insert(0, (root.clone(), 0));
    let mut queue: VecDeque<NodeId> = VecDeque::from([0]);
    let (job_tx, job_rx) = channel::<Job>();
    let job_rx = Arc::new(Mutex::new(job_rx));
    let (done_tx, done_rx) = channel::<Done>();
    let workers = limits.workers.clamp(1, 16);
    for k in 0..workers {
        let rx = job_rx.clone();
        let tx = done_tx.clone();
        let cap = limits.per_dir;
        let c = cancel.clone();
        let _ = std::thread::Builder::new()
            .name(format!("afm-read-{k}"))
            .spawn(move || {
                loop {
                    let job = {
                        let guard = rx.lock().unwrap_or_else(|p| p.into_inner());
                        guard.recv()
                    };
                    let Ok(job) = job else { break };
                    if c.load(Ordering::Relaxed) {
                        break;
                    }
                    let dir = job.dir;
                    let result = read_local(&job.path, cap, device, |found, more| {
                        // (the last piece goes with the result below)
                        if !more {
                            let _ = tx.send(Done {
                                dir,
                                found,
                                more: true,
                                result: None,
                            });
                            return true;
                        }
                        tx.send(Done {
                            dir,
                            found,
                            more,
                            result: None,
                        })
                        .is_ok()
                            && !c.load(Ordering::Relaxed)
                    });
                    if tx
                        .send(Done {
                            dir,
                            found: Vec::new(),
                            more: false,
                            result: Some(result),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            });
    }
    drop(done_tx);
    let mut inflight = 0usize;
    let mut reported_done = false;
    loop {
        if out.cancelled() {
            break;
        }
        if out.entries >= limits.hard_cap as u64 {
            queue.clear();
        }
        // folders asked for: first in line
        while let Ok(dir) = requests.try_recv() {
            if dirs.contains_key(&dir) && out.entries < limits.hard_cap as u64 {
                queue.push_front(dir);
                out.listing(dir, Listing::Pending);
                reported_done = false;
            }
        }
        while inflight < workers * 4 {
            let Some(dir) = queue.pop_front() else { break };
            let Some((path, _)) = dirs.get(&dir) else {
                continue;
            };
            if job_tx
                .send(Job {
                    dir,
                    path: path.clone(),
                })
                .is_err()
            {
                break;
            }
            inflight += 1;
        }
        if inflight == 0 {
            if !out.flush(true) {
                break;
            }
            if !reported_done {
                out.status(0, true, None, true);
                reported_done = true;
            }
            // idle: wait for a folder to be asked for
            match requests.recv_timeout(Duration::from_millis(200)) {
                Ok(dir) => {
                    if dirs.contains_key(&dir) && out.entries < limits.hard_cap as u64 {
                        queue.push_front(dir);
                        out.listing(dir, Listing::Pending);
                        reported_done = false;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            continue;
        }
        let done = match done_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(d) => d,
            Err(RecvTimeoutError::Timeout) => {
                out.flush(false);
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if !done.more {
            inflight -= 1;
        }
        let Some((path, depth)) = dirs.get(&done.dir).cloned() else {
            continue;
        };
        for f in done.found {
            if out.entries >= limits.hard_cap as u64 {
                // (the limit of all: said so, the folder read in part)
                out.note = Some(format!(
                    "stopped at the limit of {} entries: the rest is not shown",
                    limits.hard_cap
                ));
                out.listing(done.dir, Listing::Partial);
                break;
            }
            let child_path = path.join(&f.name);
            let is_dir = f.kind == Kind::Dir;
            let id = out.push(Entry {
                parent: done.dir,
                name: f.name.into_boxed_os_str(),
                kind: f.kind,
                size: f.size,
                alloc: f.alloc,
                mtime: f.mtime,
            });
            if !is_dir {
                continue;
            }
            if f.foreign || skip.contains(&child_path) {
                out.listing(id, Listing::Skipped);
                continue;
            }
            let d = depth.saturating_add(1);
            if d > limits.max_depth {
                continue;
            }
            dirs.insert(id, (child_path, d));
            let auto = auto_depth.is_none_or(|a| d < a);
            if auto && out.entries < limits.budget as u64 {
                queue.push_back(id);
            }
        }
        match done.result {
            None => {}
            Some(Err(e)) => out.listing(done.dir, listing_of(&e)),
            Some(Ok(cut)) => {
                let full = !cut && out.note.is_none();
                out.listing(
                    done.dir,
                    if full {
                        Listing::Listed
                    } else {
                        Listing::Partial
                    },
                );
            }
        }
        if !out.flush(false) {
            break;
        }
        out.status(queue.len() + inflight, false, None, false);
    }
    drop(job_tx);
}

/// A remote child's path: the folder's path, '/', the name.
fn remote_child(parent: &[u8], name: &[u8]) -> Vec<u8> {
    let mut p = parent.to_vec();
    if !p.ends_with(b"/") {
        p.push(b'/');
    }
    p.extend_from_slice(name);
    p
}

const MCP_TIMEOUT: Duration = Duration::from_secs(120);
/// How deep a remote backend is read by itself (then on demand).
const MCP_AUTO_DEPTH: u32 = 3;
/// And how many entries at most, by itself.
const MCP_BUDGET: u64 = 20_000;
/// Entries a page of a remote folder.
const MCP_PAGE: u32 = 5000;

fn remote_kind(e: &RemoteEntry) -> Kind {
    if e.is_link {
        Kind::Link
    } else if e.is_dir {
        Kind::Dir
    } else {
        Kind::File
    }
}

/// A first walk slower than this: the backend lists slowly - the rest is
/// read folder by folder as it is approached.
const MCP_SLOW: Duration = Duration::from_secs(8);

#[allow(clippy::too_many_arguments)]
fn walk_mcp(
    client: Arc<Client>,
    backend: String,
    root: String,
    lazy: bool,
    limits: Limits,
    mut out: Out,
    requests: Receiver<NodeId>,
    cancel: Arc<AtomicBool>,
) {
    let mut dirs: HashMap<NodeId, (Vec<u8>, u32)> = HashMap::new();
    dirs.insert(0, (root.as_bytes().to_vec(), 0));
    let mut queue: VecDeque<NodeId> = VecDeque::new();
    let mut lazy = lazy;
    out.lazy = lazy;
    // first a shallow walk in one call (not for a backend read lazily);
    // whatever it does not settle is read folder by folder
    let asked = Instant::now();
    let first = (!lazy).then(|| {
        client.walk(
            &backend,
            &PathArg::Text(root.clone()),
            2,
            5000,
            0,
            MCP_TIMEOUT,
        )
    });
    if asked.elapsed() > MCP_SLOW {
        // (a slow backend: the rest as it is approached)
        lazy = true;
        out.lazy = true;
    }
    // (the walk's error, kept: said when the root cannot be read either)
    let mut first_error: Option<String> = None;
    match first {
        None => queue.push_back(0),
        Some(Ok(page)) if page.next_offset.is_none() => {
            out.skipped_names += page.rejected as u64;
            // (keyed by the paths as the server shows them; the folders'
            // real paths are their parent's and their raw names)
            let mut by_path: HashMap<String, NodeId> = HashMap::new();
            by_path.insert(root.trim_end_matches('/').to_string(), 0);
            let unreadable: HashSet<String> = page.unreadable.iter().cloned().collect();
            for e in page.entries {
                let full = e.path.trim_end_matches('/').to_string();
                let parent_path = match full.rfind('/') {
                    Some(0) => String::new(),
                    Some(i) => full[..i].to_string(),
                    None => continue,
                };
                let Some(&parent) = by_path.get(&parent_path) else {
                    continue;
                };
                // (deeper than asked: a server that ignores the depth)
                let Some((parent_raw, parent_depth)) = dirs.get(&parent).cloned() else {
                    continue;
                };
                if parent_depth >= 2 {
                    continue;
                }
                let kind = remote_kind(&e);
                if kind == Kind::File && !e.size_known {
                    out.unknown_sizes += 1;
                }
                let id = out.push(Entry {
                    parent,
                    name: crate::tree::os_from_bytes(&e.name).into_boxed_os_str(),
                    kind,
                    size: e.size,
                    alloc: e.size,
                    mtime: e.mtime,
                });
                if kind == Kind::Dir {
                    by_path.insert(full.clone(), id);
                    if unreadable.contains(&full) {
                        out.listing(id, Listing::Failed);
                    }
                    dirs.insert(id, (remote_child(&parent_raw, &e.name), parent_depth + 1));
                }
            }
            for (id, (_, depth)) in &dirs {
                if *depth < 2 {
                    out.listing(*id, Listing::Listed);
                } else if *depth < MCP_AUTO_DEPTH && !lazy {
                    queue.push_back(*id);
                }
            }
        }
        Some(Ok(_)) => queue.push_back(0),
        Some(Err(e)) if !client.is_alive() => {
            out.listing(0, Listing::Failed);
            out.flush(true);
            out.status(0, true, Some(e), true);
            return;
        }
        Some(Err(e)) => {
            first_error = Some(e);
            queue.push_back(0);
        }
    }
    out.flush(true);
    let mut reported_done = false;
    loop {
        if cancel.load(Ordering::Relaxed) || !client.is_alive() {
            break;
        }
        while let Ok(dir) = requests.try_recv() {
            if dirs.contains_key(&dir) {
                queue.push_front(dir);
                out.listing(dir, Listing::Pending);
            }
        }
        let Some(dir) = queue.pop_front() else {
            out.flush(true);
            if !reported_done {
                out.status(0, true, None, true);
                reported_done = true;
            }
            match requests.recv_timeout(Duration::from_millis(300)) {
                Ok(dir) if dirs.contains_key(&dir) => {
                    queue.push_front(dir);
                    out.listing(dir, Listing::Pending);
                    reported_done = false;
                }
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            continue;
        };
        let Some((path, depth)) = dirs.get(&dir).cloned() else {
            continue;
        };
        // the folder a page at a time: a folder of any size, no answer
        // over the bound, each page shown as it comes
        let mut offset = 0u64;
        let mut ended = Listing::Listed;
        loop {
            if out.entries >= limits.hard_cap as u64 {
                out.note = Some(format!(
                    "stopped at the limit of {} entries: the rest is not shown",
                    limits.hard_cap
                ));
                ended = Listing::Partial;
                break;
            }
            let asked = Instant::now();
            let got = client.list_page(
                &backend,
                &PathArg::from_bytes(&path),
                offset,
                MCP_PAGE,
                MCP_TIMEOUT,
            );
            if asked.elapsed() > MCP_SLOW && !lazy {
                // (a slow backend: from here on folder by folder as approached)
                lazy = true;
                out.lazy = true;
            }
            match got {
                Err(e) => {
                    let denied = e.to_ascii_lowercase().contains("permission");
                    ended = if denied {
                        Listing::Denied
                    } else {
                        Listing::Failed
                    };
                    if dir == 0 {
                        // the root cannot be read: said on the island
                        let why = match first_error.take() {
                            Some(w) if w != e => format!("{w}; {e}"),
                            _ => e,
                        };
                        out.listing(0, ended);
                        out.flush(true);
                        out.status(0, true, Some(why), true);
                        reported_done = true;
                    }
                    break;
                }
                Ok(page) => {
                    out.skipped_names += page.rejected as u64;
                    let n = page.entries.len() as u64;
                    for e in page.entries {
                        let kind = remote_kind(&e);
                        if kind == Kind::File && !e.size_known {
                            out.unknown_sizes += 1;
                        }
                        let child = remote_child(&path, &e.name);
                        let id = out.push(Entry {
                            parent: dir,
                            name: crate::tree::os_from_bytes(&e.name).into_boxed_os_str(),
                            kind,
                            size: e.size,
                            alloc: e.size,
                            mtime: e.mtime,
                        });
                        if kind == Kind::Dir && depth < limits.max_depth {
                            dirs.insert(id, (child, depth + 1));
                            if depth + 1 < MCP_AUTO_DEPTH && out.entries < MCP_BUDGET && !lazy {
                                queue.push_back(id);
                            }
                        }
                    }
                    out.flush(false);
                    match page.next_offset {
                        Some(next) if next > offset && n > 0 => offset = next,
                        _ => break,
                    }
                }
            }
        }
        if dir != 0 || ended == Listing::Listed || ended == Listing::Partial {
            out.listing(dir, ended);
        }
        out.flush(false);
        out.status(queue.len(), false, None, false);
    }
}

/// A small random number generator (xorshift64*), seeded.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }
}

const SYN_DIRS: &[&str] = &[
    "src",
    "assets",
    "build",
    "data",
    "docs",
    "lib",
    "tests",
    "vendor",
    "cache",
    "photos",
    "music",
    "videos",
    "backups",
    "logs",
    "projects",
    "archive",
    "models",
    "config",
    "tools",
    "scripts",
    "share",
    "bin",
    "include",
    "target",
    "node_modules",
    "renders",
    "raw",
    "export",
];
const SYN_EXT: &[&str] = &[
    "rs", "py", "c", "h", "js", "md", "txt", "pdf", "png", "jpg", "mp4", "mkv", "mp3", "flac",
    "zip", "tar.gz", "so", "o", "json", "csv", "toml", "yaml", "log", "db", "bin", "iso",
];

/// A made-up tree of `files` files, streamed like a walk: folders of
/// varied fan-out, names with many extensions, sizes spread over orders of
/// magnitude, times over the last years.
pub fn synthetic_entries(files: usize, seed: u64) -> Vec<Entry> {
    let mut rng = Rng::new(seed);
    let now = crate::layout::now_secs();
    let mut out: Vec<Entry> = Vec::with_capacity(files + files / 8);
    let mut queue: VecDeque<(NodeId, u32)> = VecDeque::from([(0, 0)]);
    let mut made = 0usize;
    let mut next_id: NodeId = 1;
    while made < files {
        let Some((dir, depth)) = queue.pop_front() else {
            queue.push_back((0, 0));
            continue;
        };
        let subdirs = if depth < 7 {
            rng.below(if depth == 0 { 12 } else { 7 }) + u64::from(depth < 2)
        } else {
            0
        };
        for _ in 0..subdirs {
            let name = format!(
                "{}-{}",
                SYN_DIRS[rng.below(SYN_DIRS.len() as u64) as usize],
                rng.below(1000)
            );
            out.push(Entry {
                parent: dir,
                name: OsString::from(name).into_boxed_os_str(),
                kind: Kind::Dir,
                size: 4096,
                alloc: 4096,
                mtime: now - rng.below(86_400 * 900) as i64,
            });
            queue.push_back((next_id, depth + 1));
            next_id += 1;
        }
        let n = (rng.below(60) + 2) as usize;
        for _ in 0..n.min(files - made) {
            let ext = SYN_EXT[rng.below(SYN_EXT.len() as u64) as usize];
            // log-uniform sizes: 100 B .. 4 GB, the big ones rare
            let size = (10f64.powf(2.0 + 7.6 * rng.unit().powf(2.2))) as u64;
            let age = (rng.unit().powf(3.0) * 86_400.0 * 1200.0) as i64;
            out.push(Entry {
                parent: dir,
                name: OsString::from(format!("file{:05}.{ext}", rng.below(100_000)))
                    .into_boxed_os_str(),
                kind: Kind::File,
                size,
                alloc: size,
                mtime: now - age,
            });
            next_id += 1;
            made += 1;
        }
    }
    out
}

fn synthetic(files: usize, seed: u64, mut out: Out) {
    let entries = synthetic_entries(files, seed);
    let dirs: Vec<NodeId> = std::iter::once(0)
        .chain(
            entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.kind == Kind::Dir)
                .map(|(i, _)| i as NodeId + 1),
        )
        .collect();
    for e in entries {
        if out.cancelled() {
            return;
        }
        out.push(e);
        if out.batch.entries.len() >= BATCH && !out.flush(true) {
            return;
        }
    }
    for d in dirs {
        out.listing(d, Listing::Listed);
    }
    out.flush(true);
    out.status(0, true, None, true);
}

/// Walks `source` to the end on this thread (the headless modes): the
/// tree, and the status at the end.
pub fn walk_blocking(
    source: Source,
    limits: Limits,
    root_name: &std::ffi::OsStr,
) -> (crate::tree::Tree, Status) {
    let (tx, rx) = channel();
    let walker = Walker::start(1, 1, source, limits, tx, Arc::new(|| {}));
    let mut tree = crate::tree::Tree::new(root_name, 0);
    let mut status = Status::default();
    loop {
        match rx.recv_timeout(Duration::from_secs(600)) {
            Ok(Msg {
                event: Event::Batch(b),
                ..
            }) => {
                tree.apply(b);
            }
            Ok(Msg {
                event: Event::Status(s),
                ..
            }) => {
                let done = s.done;
                status = s;
                if done {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    walker.stop();
    // whatever was sent before the status
    while let Ok(m) = rx.try_recv() {
        if let Event::Batch(b) = m.event {
            tree.apply(b);
        }
    }
    (tree, status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::Scratch;
    use serde_json::json;
    use std::ffi::OsStr;

    fn local(root: &Path) -> Source {
        Source::Local {
            root: root.to_path_buf(),
            skip: Vec::new(),
            auto_depth: None,
        }
    }

    fn find<'a>(t: &'a crate::tree::Tree, name: &str) -> Option<&'a crate::tree::Node> {
        t.nodes.iter().find(|n| &*n.name == OsStr::new(name))
    }

    #[test]
    fn walks_a_folder_with_links_loops_and_odd_names() {
        let dir = Scratch::new("walk");
        let root = dir.path();
        std::fs::create_dir_all(root.join("a/b/c")).unwrap();
        std::fs::write(root.join("a/one.txt"), b"12345").unwrap();
        std::fs::write(root.join("a/b/c/two.rs"), vec![0u8; 1000]).unwrap();
        std::fs::write(root.join("new\nline.txt"), b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            std::fs::write(root.join(OsStr::from_bytes(b"caf\xe9.bin")), b"yz").unwrap();
            // a loop: a link back up, never followed
            std::os::unix::fs::symlink(root, root.join("a/b/loop")).unwrap();
            std::os::unix::fs::symlink("/nonexistent", root.join("dangling")).unwrap();
        }
        let (t, status) = walk_blocking(local(root), Limits::default(), OsStr::new("root"));
        assert!(status.done, "{status:?}");
        assert!(status.error.is_none());
        assert_eq!(find(&t, "one.txt").unwrap().size, 5);
        assert_eq!(find(&t, "two.rs").unwrap().size, 1000);
        assert!(find(&t, "new\nline.txt").is_some());
        #[cfg(unix)]
        {
            assert_eq!(find(&t, "loop").unwrap().kind, Kind::Link);
            assert_eq!(find(&t, "dangling").unwrap().kind, Kind::Link);
            let raw = crate::tree::os_from_bytes(b"caf\xe9.bin");
            assert!(t.nodes.iter().any(|n| *n.name == *raw));
        }
        // folders are read; the totals add up
        assert_eq!(find(&t, "c").unwrap().listing, Listing::Listed);
        // (a link's size is its target's length)
        let links: u64 = t
            .nodes
            .iter()
            .filter(|n| n.kind == Kind::Link && &*n.name == OsStr::new("loop"))
            .map(|n| n.size)
            .sum();
        assert_eq!(find(&t, "a").unwrap().total, 1005 + links);
        assert!(t.root().total >= 1006);
        // paths come back from the chain
        let two = t
            .nodes
            .iter()
            .position(|n| &*n.name == OsStr::new("two.rs"))
            .unwrap();
        assert_eq!(t.local_path(root, two as NodeId), root.join("a/b/c/two.rs"));
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_without_permission_is_marked() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Scratch::new("denied");
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("secret"), b"x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let (t, _) = walk_blocking(local(dir.path()), Limits::default(), OsStr::new("r"));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        let l = find(&t, "locked").unwrap();
        // (as root the folder can be read anyway)
        if !running_as_root() {
            assert_eq!(l.listing, Listing::Denied);
        }
    }

    #[cfg(unix)]
    fn running_as_root() -> bool {
        std::fs::metadata("/proc/self")
            .map(|m| {
                use std::os::unix::fs::MetadataExt;
                m.uid() == 0
            })
            .unwrap_or(false)
    }

    #[test]
    fn huge_folders_are_read_in_part_and_budgets_leave_the_rest_unread() {
        let dir = Scratch::new("huge");
        let root = dir.path();
        for i in 0..300 {
            std::fs::write(root.join(format!("f{i}")), b"").unwrap();
        }
        std::fs::create_dir_all(root.join("deep/er/est")).unwrap();
        let limits = Limits {
            per_dir: 100,
            ..Limits::default()
        };
        let (t, _) = walk_blocking(local(root), limits, OsStr::new("r"));
        assert_eq!(t.root().listing, Listing::Partial);
        assert!(t.len() <= 1 + 100 + 3);
        // a budget: the first level only
        let limits = Limits {
            budget: 1,
            ..Limits::default()
        };
        let dir2 = Scratch::new("budget");
        std::fs::create_dir_all(dir2.path().join("a/b")).unwrap();
        let (t, _) = walk_blocking(local(dir2.path()), limits, OsStr::new("r"));
        assert_eq!(find(&t, "a").unwrap().listing, Listing::Unlisted);
    }

    #[test]
    fn a_folder_bigger_than_a_piece_is_read_whole() {
        // (a big folder streams in pieces of PIECE entries: none is cut)
        let dir = Scratch::new("big-folder");
        let n = PIECE * 2 + 500;
        for i in 0..n {
            std::fs::File::create(dir.path().join(format!("f{i:06}"))).unwrap();
        }
        let (t, status) = walk_blocking(local(dir.path()), Limits::default(), OsStr::new("r"));
        assert_eq!(t.root().children.len(), n);
        assert_eq!(t.root().listing, Listing::Listed);
        assert_eq!(status.note, None);
    }

    #[test]
    fn the_limit_of_all_is_said() {
        let dir = Scratch::new("hard-cap");
        for i in 0..3000 {
            std::fs::File::create(dir.path().join(format!("f{i}"))).unwrap();
        }
        let limits = Limits {
            hard_cap: 1000,
            ..Limits::default()
        };
        let (t, status) = walk_blocking(local(dir.path()), limits, OsStr::new("r"));
        assert!(t.len() <= 1001);
        assert_eq!(t.root().listing, Listing::Partial);
        assert!(
            status.note.as_deref().is_some_and(|n| n.contains("1000")),
            "{:?}",
            status.note
        );
    }

    #[test]
    fn folders_hundreds_deep_are_read_to_the_bottom() {
        let dir = Scratch::new("deep");
        let mut p = dir.path().to_path_buf();
        for _ in 0..400 {
            p.push("d");
        }
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("bottom.txt"), b"x").unwrap();
        let (t, _) = walk_blocking(local(dir.path()), Limits::default(), OsStr::new("r"));
        let b = t
            .nodes
            .iter()
            .position(|n| n.name.as_ref() == OsStr::new("bottom.txt"))
            .expect("found at the bottom");
        assert_eq!(t.depth(b as NodeId), 401);
        assert!(
            t.nodes
                .iter()
                .all(|n| n.kind != Kind::Dir || n.listing == Listing::Listed)
        );
    }

    #[test]
    fn a_folder_is_read_when_asked() {
        let dir = Scratch::new("lazy");
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("a/b/f"), b"abc").unwrap();
        let (tx, rx) = channel();
        let w = Walker::start(
            7,
            1,
            Source::Local {
                root: dir.path().to_path_buf(),
                skip: vec![],
                auto_depth: Some(1),
            },
            Limits::default(),
            tx,
            Arc::new(|| {}),
        );
        let mut t = crate::tree::Tree::new(OsStr::new("r"), 0);
        let mut asked = false;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let Ok(m) = rx.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            assert_eq!(m.island, 7);
            match m.event {
                Event::Batch(b) => assert!(t.apply(b)),
                Event::Status(s) if s.done => {
                    let a = t
                        .nodes
                        .iter()
                        .position(|n| &*n.name == OsStr::new("a"))
                        .unwrap();
                    if !asked {
                        assert_eq!(t.nodes[a].listing, Listing::Unlisted);
                        w.list(a as NodeId);
                        asked = true;
                    } else if let Some(b) = t.nodes.iter().position(|n| &*n.name == OsStr::new("b"))
                    {
                        if t.nodes[b].listing == Listing::Unlisted {
                            w.list(b as NodeId);
                        } else {
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        assert_eq!(find(&t, "f").map(|n| n.size), Some(3));
    }

    #[test]
    fn a_missing_root_is_said_not_thrown() {
        let (t, status) = walk_blocking(
            local(Path::new("/nonexistent/afm")),
            Limits::default(),
            OsStr::new("x"),
        );
        assert!(status.error.is_some());
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn walks_an_mcp_backend() {
        let client = Arc::new(crate::mcp::tests::fake_server(true));
        let (t, status) = walk_blocking(
            Source::Mcp {
                client,
                backend: "sftp-1".into(),
                root: "/".into(),
                lazy: false,
            },
            Limits::default(),
            OsStr::new("/"),
        );
        assert!(status.done);
        let names: Vec<String> = t
            .nodes
            .iter()
            .map(|n| n.name.to_string_lossy().into_owned())
            .collect();
        for want in ["a", "x.txt", "deep", "y.bin", "z.png"] {
            assert!(names.contains(&want.to_string()), "{want} in {names:?}");
        }
        assert_eq!(find(&t, "y.bin").unwrap().size, 2048);
        assert_eq!(t.root().total, 11 + 2048 + 99);
    }

    #[test]
    fn a_failing_remote_walk_is_said_on_the_island() {
        let client = Arc::new(crate::mcp::tests::tool_server(|name, _| {
            let text = match name {
                "walk_tree" => "walk failed: boom",
                _ => "no such folder",
            };
            json!({"content": [{"type": "text", "text": text}], "isError": true})
        }));
        let (t, status) = walk_blocking(
            Source::Mcp {
                client,
                backend: "b".into(),
                root: "/gone".into(),
                lazy: false,
            },
            Limits::default(),
            OsStr::new("/gone"),
        );
        assert!(status.done);
        let e = status.error.expect("an error, not an empty island");
        assert_eq!(e, "walk failed: boom");
        assert_eq!(t.root().listing, Listing::Failed);
    }

    #[test]
    fn remote_names_that_reach_outside_are_left_out_and_counted() {
        let client = Arc::new(crate::mcp::tests::tool_server(|name, _| {
            let entries = json!([
                {"path": "/ok.txt", "name": "ok.txt", "size": 5},
                {"path": "/../../etc/passwd", "name": "../../etc/passwd", "size": 1},
                {"path": "/a/b", "name": "a/b", "size": 1},
            ]);
            let v = match name {
                "walk_tree" => json!({"entries": entries, "next_offset": null}),
                _ => entries,
            };
            json!({"content": [{"type": "text", "text": v.to_string()}], "isError": false})
        }));
        let (t, status) = walk_blocking(
            Source::Mcp {
                client,
                backend: "b".into(),
                root: "/".into(),
                lazy: false,
            },
            Limits::default(),
            OsStr::new("/"),
        );
        let names: Vec<String> = t
            .nodes
            .iter()
            .skip(1)
            .map(|n| n.name.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["ok.txt"]);
        assert!(status.skipped_names >= 2, "{}", status.skipped_names);
    }

    #[test]
    fn synthetic_trees_are_the_same_each_time() {
        let a = synthetic_entries(5000, 3);
        let b = synthetic_entries(5000, 3);
        assert_eq!(a.len(), b.len());
        assert_eq!(a.iter().filter(|e| e.kind != Kind::Dir).count(), 5000);
        let (t, s) = walk_blocking(
            Source::Synthetic {
                files: 5000,
                seed: 3,
            },
            Limits::default(),
            OsStr::new("syn"),
        );
        assert!(s.done);
        assert_eq!(t.root().count, 5000);
        assert_eq!(t.len(), a.len() + 1);
    }
}
