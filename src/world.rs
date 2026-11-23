//! The islands as the window keeps them: where each comes from, its tree
//! (the cached one at first, then the new walk's), its walk, its city; the
//! layout thread; the placement of all of them.

use crate::cache;
use crate::city::{IslandCity, IslandInstances, Ring, boxes_of, make_instances};
use crate::hw::HwIsland;
use crate::layout::{CityLayout, Params, layout, snapshot};
use crate::math::Aabb;
use crate::mcp::{Backend, Client};
use crate::mounts::{Mount, MountKind};
use crate::pick::Bvh;
use crate::tree::{NodeId, Tree};
use crate::walk::{Limits, Source, Status, Walker};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

/// Where an island comes from.
#[derive(Clone, Debug)]
pub enum IslandSource {
    /// A local folder: a mountpoint, or a folder given on the command line.
    Local { root: PathBuf, mount: Option<Mount> },
    /// A backend of axross's MCP server.
    Mcp {
        client: Arc<Client>,
        backend: Backend,
        root: String,
    },
    /// A made-up tree.
    Synthetic { files: usize, seed: u64 },
    /// The machine itself.
    Hardware,
}

impl IslandSource {
    /// The cache's key: what was walked.
    pub fn key(&self) -> String {
        match self {
            IslandSource::Local { root, .. } => format!("local:{}", root.to_string_lossy()),
            IslandSource::Mcp {
                client,
                backend,
                root,
            } => {
                format!(
                    "mcp:{}:{}:{}:{}",
                    client.server,
                    backend.id,
                    backend.profile.as_deref().unwrap_or(""),
                    root
                )
            }
            IslandSource::Synthetic { files, seed } => format!("synthetic:{files}:{seed}"),
            IslandSource::Hardware => "hardware".into(),
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, IslandSource::Mcp { .. })
    }
}

/// A walk's batch that does not fit the tree (never expected: the walk
/// ends there).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutOfStep;

pub struct Island {
    /// Unique for this run (messages name it).
    pub id: u64,
    pub source: IslandSource,
    pub title: String,
    pub ring: Ring,
    /// What is shown. Shared with the threads that read it (the layout,
    /// the cache's save): while one of them holds it, the walk's batches
    /// wait in `deferred` - the window never copies a tree.
    pub tree: Arc<Tree>,
    /// Batches that came while the tree was lent out.
    pub deferred: std::collections::VecDeque<crate::tree::Batch>,
    /// A new walk while a cached tree is shown (swapped in when it ends).
    pub fresh: Option<Tree>,
    pub generation: u32,
    pub status: Status,
    /// The cache's time, while the cached tree is shown.
    pub from_cache: Option<i64>,
    pub city: Option<IslandCity>,
    pub walker: Option<Walker>,
    /// The tree changed since the last layout was asked for.
    pub dirty: bool,
    pub layout_pending: bool,
    pub last_layout: Instant,
    /// Folders asked to be read (each once).
    pub requested: HashSet<NodeId>,
    /// The machine's island: its parts' details.
    pub hw: Option<HwIsland>,
    /// The disks (more than one: RAID, a pool) this island lives on.
    pub disks: Vec<usize>,
    /// About how many entries the walk will find (room is made for them
    /// at its start: no copy of a huge tree while it grows).
    pub expected: usize,
    /// Removable, a network file system or a remote backend: cached only
    /// when the setting says so.
    pub sensitive: bool,
    /// The cache was written for this walk.
    pub saved: bool,
    /// Which tree is shown (a new one - the cache's, a finished walk's -
    /// counts on): a layout is taken only for the tree it was made of.
    pub epoch: u32,
    /// The city stands for a tree no longer shown (its node ids are not
    /// this tree's): drawn, but not picked or labelled until the new
    /// layout comes.
    pub stale: bool,
}

impl Island {
    pub fn new(id: u64, source: IslandSource, title: String, ring: Ring) -> Island {
        let root_name = match &source {
            IslandSource::Local { root, .. } => root.as_os_str().to_os_string(),
            IslandSource::Mcp { backend, root, .. } => {
                let name = if backend.name.is_empty() {
                    backend.id.clone()
                } else {
                    backend.name.clone()
                };
                format!("{name}:{root}").into()
            }
            IslandSource::Synthetic { .. } => "synthetic".into(),
            IslandSource::Hardware => "This machine".into(),
        };
        Island {
            id,
            source,
            title,
            ring,
            tree: Arc::new(Tree::new(&root_name, 0)),
            deferred: std::collections::VecDeque::new(),
            fresh: None,
            generation: 0,
            status: Status::default(),
            from_cache: None,
            city: None,
            walker: None,
            dirty: true,
            layout_pending: false,
            last_layout: Instant::now() - std::time::Duration::from_secs(10),
            requested: HashSet::new(),
            hw: None,
            disks: Vec::new(),
            expected: 0,
            sensitive: false,
            saved: false,
            epoch: 0,
            stale: false,
        }
    }

    /// The walk's source (None: the machine, which is not walked).
    pub fn walk_source(&self, show_pseudo_skip: &[PathBuf]) -> Option<Source> {
        match &self.source {
            IslandSource::Local { root, mount } => Some(Source::Local {
                root: root.clone(),
                skip: show_pseudo_skip.to_vec(),
                auto_depth: mount
                    .as_ref()
                    .filter(|m| m.kind == MountKind::Network)
                    .map(|_| 3),
            }),
            IslandSource::Mcp {
                client,
                backend,
                root,
            } => Some(Source::Mcp {
                client: client.clone(),
                backend: backend.id.clone(),
                root: root.clone(),
                lazy: backend
                    .protocol
                    .as_deref()
                    .is_some_and(|p| crate::systems::Family::of(p).lazy()),
            }),
            IslandSource::Synthetic { files, seed } => Some(Source::Synthetic {
                files: *files,
                seed: *seed,
            }),
            IslandSource::Hardware => None,
        }
    }

    /// Shows another tree: the city becomes stale until its layout comes.
    pub fn replace_tree(&mut self, tree: Tree) {
        // (a big tree's memory is given back on another thread)
        drop_later(std::mem::replace(&mut self.tree, Arc::new(tree)));
        self.deferred.clear();
        self.epoch += 1;
        self.dirty = true;
        self.layout_pending = false;
        self.requested.clear();
        if self.city.is_some() {
            self.stale = true;
        }
    }

    /// Applies a walk's batch: to the walk's own tree while a cached one
    /// is shown, else to the shown tree - or, while that is lent out, keeps
    /// it for later. Ok(true): the shown tree changed. Err: out of step.
    pub fn apply_batch(&mut self, b: crate::tree::Batch) -> Result<bool, OutOfStep> {
        if let Some(f) = self.fresh.as_mut() {
            return if f.apply(b) {
                Ok(false)
            } else {
                Err(OutOfStep)
            };
        }
        if self.deferred.is_empty()
            && let Some(tree) = Arc::get_mut(&mut self.tree)
        {
            if !tree.apply(b) {
                return Err(OutOfStep);
            }
            self.dirty = true;
            return Ok(true);
        }
        // (the tree is lent out, or batches wait already: in order, later)
        self.deferred.push_back(b);
        Ok(false)
    }

    /// Applies what waited, if the tree is back - for at most `budget`
    /// (a backlog of batches never holds a frame).
    pub fn drain_deferred(&mut self, budget: std::time::Duration) -> Result<bool, OutOfStep> {
        let start = std::time::Instant::now();
        let mut changed = false;
        while !self.deferred.is_empty() && start.elapsed() < budget {
            let Some(tree) = Arc::get_mut(&mut self.tree) else {
                break;
            };
            let b = self.deferred.pop_front().expect("not empty");
            if !tree.apply(b) {
                self.deferred.clear();
                return Err(OutOfStep);
            }
            changed = true;
        }
        if changed {
            self.dirty = true;
        }
        Ok(changed)
    }

    /// The walk reads lazily asked folders only into the tree it builds:
    /// while a cached tree is shown, nothing is asked.
    pub fn can_ask(&self) -> bool {
        self.fresh.is_none() && self.walker.is_some()
    }
}

/// A layout asked of the layout thread: the tree itself (lent; the
/// thread makes its compact copy), where the island stands and how it
/// looks - the instances come back with the layout.
pub struct Job {
    pub id: u64,
    pub generation: u32,
    pub tree: Arc<Tree>,
    pub ring: Ring,
    pub offset: [f32; 2],
    pub theme: &'static crate::theme::Theme,
    pub by_alloc: bool,
}

/// Instances for a layout at a new place (another island grew).
pub struct PlaceJob {
    pub id: u64,
    pub layout: Arc<CityLayout>,
    pub ring: Ring,
    pub offset: [f32; 2],
    pub theme: &'static crate::theme::Theme,
}

enum Work {
    Layout(Job),
    Place(PlaceJob),
}

/// What the layout thread made.
pub enum Made {
    Layout(Done),
    Placed {
        id: u64,
        layout: Arc<CityLayout>,
        inst: Arc<IslandInstances>,
    },
}

pub struct Done {
    pub id: u64,
    pub generation: u32,
    pub layout: Arc<CityLayout>,
    pub boxes: Arc<Vec<Aabb>>,
    pub bvh: Arc<Bvh>,
    pub inst: Arc<IslandInstances>,
    pub seconds: f32,
}

/// The layout thread: the newest job per island, one after another; the
/// compact copy, the treemap, the picking boxes and the instances all made
/// here, never on the window's thread.
pub struct LayoutWorker {
    tx: Sender<Work>,
    pub rx: Receiver<Made>,
}

impl LayoutWorker {
    pub fn start(wake: crate::walk::Wake) -> LayoutWorker {
        let (tx, jobs) = channel::<Work>();
        let (done_tx, rx) = channel();
        let _ = std::thread::Builder::new()
            .name("afm-layout".into())
            .spawn(move || {
                while let Ok(first) = jobs.recv() {
                    // the newest per island (and kind of work)
                    let mut layouts: HashMap<u64, Job> = HashMap::new();
                    let mut places: HashMap<u64, PlaceJob> = HashMap::new();
                    let mut take = |w: Work| match w {
                        Work::Layout(j) => {
                            places.remove(&j.id);
                            layouts.insert(j.id, j);
                        }
                        Work::Place(j) => {
                            places.insert(j.id, j);
                        }
                    };
                    take(first);
                    while let Ok(w) = jobs.try_recv() {
                        take(w);
                    }
                    for (_, p) in places {
                        let inst = Arc::new(make_instances(&p.layout, p.ring, p.offset, p.theme));
                        if done_tx
                            .send(Made::Placed {
                                id: p.id,
                                layout: p.layout,
                                inst,
                            })
                            .is_err()
                        {
                            return;
                        }
                        wake();
                    }
                    let mut batch: Vec<Job> = layouts.into_values().collect();
                    batch.sort_by_key(|j| j.tree.len());
                    for job in batch {
                        let t = Instant::now();
                        let nodes = snapshot(&job.tree);
                        // (the tree goes back to the window at once)
                        drop(job.tree);
                        let params = Params {
                            by_alloc: job.by_alloc,
                            ..Params::default()
                        };
                        let l = Arc::new(layout(&nodes, &params));
                        drop(nodes);
                        let boxes = boxes_of(&l);
                        let bvh = Bvh::build(&boxes);
                        let inst = Arc::new(make_instances(&l, job.ring, job.offset, job.theme));
                        let done = Done {
                            id: job.id,
                            generation: job.generation,
                            layout: l,
                            boxes: Arc::new(boxes),
                            bvh: Arc::new(bvh),
                            inst,
                            seconds: t.elapsed().as_secs_f32(),
                        };
                        if done_tx.send(Made::Layout(done)).is_err() {
                            return;
                        }
                        wake();
                    }
                }
            });
        LayoutWorker { tx, rx }
    }

    pub fn ask(&self, job: Job) {
        let _ = self.tx.send(Work::Layout(job));
    }

    pub fn place(&self, job: PlaceJob) {
        let _ = self.tx.send(Work::Place(job));
    }
}

/// How long to wait between layouts of a growing island (a big one is laid
/// out less often).
pub fn layout_interval(nodes: usize) -> std::time::Duration {
    std::time::Duration::from_secs_f32((nodes as f32 / 300_000.0).clamp(0.25, 4.0))
}

/// Reads an island's cache on a thread of its own.
pub fn load_cache(
    dir: PathBuf,
    key: String,
    id: u64,
    tx: Sender<(u64, Option<(i64, Tree)>)>,
    wake: crate::walk::Wake,
) {
    let _ = std::thread::Builder::new()
        .name("afm-cache".into())
        .spawn(move || {
            let got = cache::load(&dir, &key).map(|(h, t)| (h.saved, t));
            let _ = tx.send((id, got));
            wake();
        });
}

/// A walk's limits from the settings.
pub fn limits(budget: usize, workers: usize) -> Limits {
    Limits {
        budget,
        workers,
        ..Limits::default()
    }
}

/// Gives `t` back on a thread of its own (a big tree's, a city's memory:
/// tens of milliseconds to unmap - never on the window's thread).
pub fn drop_later<T: Send + 'static>(t: T) {
    type Job = Box<dyn Send>;
    static DROPPER: std::sync::OnceLock<std::sync::Mutex<std::sync::mpsc::Sender<Job>>> =
        std::sync::OnceLock::new();
    let tx = DROPPER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        let _ = std::thread::Builder::new()
            .name("afm-drop".into())
            .spawn(move || {
                for job in rx {
                    drop(job)
                }
            });
        std::sync::Mutex::new(tx)
    });
    if let Ok(tx) = tx.lock() {
        let _ = tx.send(Box::new(t));
    }
}

/// What the start needs of the system - the mounts, the machine, whether
/// a path is a folder - behind a trait: on a thread of its own (a hung
/// mount never holds the window), and the tests give it one that hangs.
pub trait Probe: Send + Sync {
    fn mounts(&self) -> Vec<Mount>;
    fn machine(&self) -> crate::hw::Machine;
    /// A path made absolute (literally: no symbolic link resolved).
    fn absolute(&self, p: &std::path::Path) -> PathBuf;
    fn is_dir(&self, p: &std::path::Path) -> bool;
    /// How many entries (inodes) the file system at `p` holds - room is
    /// made for that many at the walk's start (None: not known).
    fn entries_on(&self, _p: &std::path::Path) -> Option<u64> {
        None
    }
}

/// The system itself.
pub struct RealProbe;

impl Probe for RealProbe {
    fn mounts(&self) -> Vec<Mount> {
        crate::mounts::system_mounts()
    }
    fn machine(&self) -> crate::hw::Machine {
        crate::hw::detect()
    }
    fn absolute(&self, p: &std::path::Path) -> PathBuf {
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|d| d.join(p))
                .unwrap_or_else(|_| p.to_path_buf())
        }
    }
    fn is_dir(&self, p: &std::path::Path) -> bool {
        p.is_dir()
    }
    fn entries_on(&self, p: &std::path::Path) -> Option<u64> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
            // SAFETY: statvfs fills the struct given, from a NUL-ended path
            let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
            if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
                return None;
            }
            #[allow(clippy::unnecessary_cast)]
            let used = (st.f_files as u64).saturating_sub(st.f_ffree as u64);
            (used > 0).then_some(used)
        }
        #[cfg(not(unix))]
        {
            let _ = p;
            None
        }
    }
}

/// A local island to make: its root, its mount, the mountpoints below it
/// (islands of their own), the disks it lives on.
pub struct LocalIsland {
    pub root: PathBuf,
    pub mount: Option<Mount>,
    pub skip: Vec<PathBuf>,
    pub disks: Vec<usize>,
    /// About how many entries its file system holds (room made for them).
    pub expected: Option<u64>,
}

/// What the start found (on its thread).
pub struct Startup {
    pub locals: Vec<LocalIsland>,
    pub machine: Option<crate::hw::Machine>,
    pub hw: Option<HwIsland>,
}

/// What to look at at the start.
#[derive(Clone, Debug)]
pub struct StartupWants {
    pub paths: Vec<PathBuf>,
    pub mounts: bool,
    pub show_pseudo: bool,
    pub show_binds: bool,
    pub hardware: bool,
    pub machine: Option<crate::hw::Machine>,
}

/// The start's work: the mounts (pseudo ones, bind mounts of a file system
/// shown already and mountpoints that are no folder left out), the
/// machine and its island, which disks each island lives on.
pub fn startup(probe: &dyn Probe, w: &StartupWants) -> Startup {
    let machine = if w.hardware {
        Some(w.machine.clone().unwrap_or_else(|| probe.machine()))
    } else {
        None
    };
    let hw = machine.as_ref().map(crate::hw::island);
    let all = if w.mounts || !w.paths.is_empty() {
        probe.mounts()
    } else {
        Vec::new()
    };
    let mut locals = Vec::new();
    if !w.paths.is_empty() {
        for p in &w.paths {
            locals.push(LocalIsland {
                root: probe.absolute(p),
                mount: None,
                skip: Vec::new(),
                disks: Vec::new(),
                expected: None,
            });
        }
    } else if w.mounts {
        let shown = crate::mounts::visible(&all, w.show_pseudo);
        let shown = if w.show_binds {
            shown
        } else {
            crate::mounts::hide_binds(&shown)
        };
        for m in shown {
            // (a file bind-mounted into place - /etc/hosts in a container
            // - is no island)
            if !probe.is_dir(&m.mount_point) {
                continue;
            }
            let disks = match &machine {
                Some(mc) => crate::hw::disks_of(mc, &m),
                None => Vec::new(),
            };
            // (a network file system is not asked: it may hang - this is
            // its own thread, but its walk is lazy anyway)
            let expected = if m.kind == MountKind::Network {
                None
            } else {
                probe.entries_on(&m.mount_point)
            };
            locals.push(LocalIsland {
                expected,
                skip: crate::mounts::nested_below(&all, &m.mount_point),
                root: m.mount_point.clone(),
                mount: Some(m),
                disks,
            });
        }
    }
    Startup {
        locals,
        machine,
        hw,
    }
}
