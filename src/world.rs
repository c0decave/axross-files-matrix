//! The islands as the window keeps them: where each comes from, its tree
//! (the cached one at first, then the new walk's), its walk, its city; the
//! layout thread; the placement of all of them.

use crate::cache;
use crate::city::{IslandCity, Ring, boxes_of};
use crate::hw::HwIsland;
use crate::layout::{CityLayout, LayoutNode, Params, layout};
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

pub struct Island {
    /// Unique for this run (messages name it).
    pub id: u64,
    pub source: IslandSource,
    pub title: String,
    pub ring: Ring,
    /// What is shown.
    pub tree: Tree,
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
    /// The disk (index in the machine) this island lives on.
    pub disk: Option<usize>,
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
            tree: Tree::new(&root_name, 0),
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
            disk: None,
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
        self.tree = tree;
        self.epoch += 1;
        self.dirty = true;
        self.layout_pending = false;
        self.requested.clear();
        if self.city.is_some() {
            self.stale = true;
        }
    }

    /// The tree the walk's batches go to.
    pub fn walk_tree(&mut self) -> &mut Tree {
        match self.fresh.as_mut() {
            Some(t) => t,
            None => &mut self.tree,
        }
    }

    /// The walk reads lazily asked folders only into the tree it builds:
    /// while a cached tree is shown, nothing is asked.
    pub fn can_ask(&self) -> bool {
        self.fresh.is_none() && self.walker.is_some()
    }
}

/// A layout asked of the layout thread.
pub struct Job {
    pub id: u64,
    pub generation: u32,
    pub nodes: Vec<LayoutNode>,
}

/// What the layout thread made.
pub struct Done {
    pub id: u64,
    pub generation: u32,
    pub layout: CityLayout,
    pub boxes: Vec<Aabb>,
    pub bvh: Bvh,
    pub seconds: f32,
}

/// The layout thread: the newest job per island, one after another.
pub struct LayoutWorker {
    tx: Sender<Job>,
    pub rx: Receiver<Done>,
}

impl LayoutWorker {
    pub fn start(wake: crate::walk::Wake) -> LayoutWorker {
        let (tx, jobs) = channel::<Job>();
        let (done_tx, rx) = channel();
        let _ = std::thread::Builder::new()
            .name("afm-layout".into())
            .spawn(move || {
                while let Ok(first) = jobs.recv() {
                    // the newest per island
                    let mut latest: HashMap<u64, Job> = HashMap::new();
                    latest.insert(first.id, first);
                    while let Ok(j) = jobs.try_recv() {
                        latest.insert(j.id, j);
                    }
                    let mut batch: Vec<Job> = latest.into_values().collect();
                    batch.sort_by_key(|j| j.nodes.len());
                    for job in batch {
                        let t = Instant::now();
                        let l = layout(&job.nodes, &Params::default());
                        let boxes = boxes_of(&l);
                        let bvh = Bvh::build(&boxes);
                        let done = Done {
                            id: job.id,
                            generation: job.generation,
                            layout: l,
                            boxes,
                            bvh,
                            seconds: t.elapsed().as_secs_f32(),
                        };
                        if done_tx.send(done).is_err() {
                            return;
                        }
                        wake();
                    }
                }
            });
        LayoutWorker { tx, rx }
    }

    pub fn ask(&self, job: Job) {
        let _ = self.tx.send(job);
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
