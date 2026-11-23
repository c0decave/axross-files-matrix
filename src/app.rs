//! The window: the city in the middle (the 3D view, its labels), the bar
//! above (search, the camera's mode, teleport, sources, settings, the
//! read-only badge), the inspect panel (details and preview of what is
//! chosen), the status line; and everything that streams in - walks,
//! layouts, cache files, axross's server, previews - taken a few
//! milliseconds a frame, so the window never waits.

use crate::cache;
use crate::camera::{Camera, Input, Mode, look_angles};
use crate::city::{self, FrameSet, Hit, Instance, IslandCity, LabelKind, Lod, Ring};
use crate::hw::{self, Machine, Part};
use crate::layout::{Placed, snapshot};
use crate::math::{Aabb, Frustum, Vec3, vec3};
use crate::mcp::{self, Backend, Client};
use crate::mounts;
use crate::preview::{self, Content, Preview, Where};
use crate::render::{self, FrameData, Globals, Stats};
use crate::search::Search;
use crate::settings::{self, Settings, Spot};
use crate::theme::{self, Theme};
use crate::tree::{Kind, Listing, NodeId, display_name};
use crate::update::{Install, UpdateMode};
use crate::update_ui::Updater;
use crate::walk::{self, Event, Msg, Walker};
use crate::world::{self, Island, IslandSource, Job, LayoutWorker};
use egui::{Color32, Key, Pos2, Rect, RichText, Sense, Ui, vec2};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How the program was started.
#[derive(Clone, Debug)]
pub struct StartOptions {
    /// Folders given on the command line: islands of these alone.
    pub paths: Vec<PathBuf>,
    /// A made-up island of this many files.
    pub synthetic: Option<usize>,
    /// The system's mountpoints as islands.
    pub mounts: bool,
    /// Look for axross's MCP server.
    pub mcp: bool,
    /// The machine's island.
    pub hardware: bool,
    /// Frame times to stderr once a second.
    pub frame_stats: bool,
    /// No adaptive level of detail (measurements).
    pub fixed_lod: bool,
    /// Where the settings and the cache are (tests: their own).
    pub config_dir: Option<PathBuf>,
    pub cache_dir: Option<PathBuf>,
    pub use_cache: bool,
    /// The machine as tests make it up (None: this one).
    pub machine: Option<Machine>,
    /// Check for updates as the settings say.
    pub updates: bool,
    /// The shaders' clock starts at this time (pictures that can be
    /// compared: the tests).
    pub clock: Option<f32>,
    /// Each frame is this long (seconds), whatever the wall clock says:
    /// a timeline the tests can measure frame by frame.
    pub step: Option<f32>,
    /// The flash guard (off only for the tests that show what it does).
    pub flash_guard: bool,
}

impl Default for StartOptions {
    fn default() -> StartOptions {
        StartOptions {
            paths: Vec::new(),
            synthetic: None,
            mounts: true,
            mcp: true,
            hardware: true,
            frame_stats: false,
            fixed_lod: false,
            config_dir: None,
            cache_dir: None,
            use_cache: true,
            machine: None,
            updates: true,
            clock: None,
            step: None,
            flash_guard: true,
        }
    }
}

/// axross's MCP server, as far as it is known.
#[derive(Clone, Debug)]
pub enum McpState {
    Off,
    Looking,
    NotFound,
    Failed(String),
    Connected {
        label: String,
        client: Arc<Client>,
        profiles: Vec<String>,
    },
}

/// What the search for axross's server found: the client, how it was
/// started, its backends and saved profiles.
type McpFound = (Arc<Client>, String, Vec<Backend>, Vec<String>);

enum Bg {
    Mcp(Result<McpFound, Option<String>>),
    Backends(Arc<Client>, Result<Vec<Backend>, String>),
    Preview(u64, NodeId, Box<Preview>),
    Opened(Result<String, String>),
}

struct PreviewView {
    island: u64,
    node: NodeId,
    loading: bool,
    preview: Option<Preview>,
    texture: Option<egui::TextureHandle>,
}

/// Frame times (seconds), the last two hundred.
#[derive(Default)]
pub struct FrameTimes {
    pub frames: std::collections::VecDeque<f32>,
    pub work: std::collections::VecDeque<f32>,
    last: Option<Instant>,
    reported: Option<Instant>,
}

impl FrameTimes {
    fn push(&mut self, work: f32) {
        let now = Instant::now();
        if let Some(l) = self.last {
            self.frames.push_back((now - l).as_secs_f32());
            self.work.push_back(work);
            while self.frames.len() > 200 {
                self.frames.pop_front();
                self.work.pop_front();
            }
        }
        self.last = Some(now);
    }
    pub fn mean_ms(v: &std::collections::VecDeque<f32>) -> f32 {
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<f32>() / v.len() as f32 * 1000.0
        }
    }
    pub fn p95_ms(v: &std::collections::VecDeque<f32>) -> f32 {
        if v.is_empty() {
            return 0.0;
        }
        let mut s: Vec<f32> = v.iter().copied().collect();
        s.sort_by(f32::total_cmp);
        s[(s.len() * 95 / 100).min(s.len() - 1)] * 1000.0
    }
}

pub struct App {
    pub settings: Settings,
    settings_path: PathBuf,
    pub opts: StartOptions,
    theme: &'static Theme,
    pub islands: Vec<Island>,
    next_id: u64,
    walk_tx: Sender<Msg>,
    walk_rx: Receiver<Msg>,
    cache_tx: Sender<(u64, Option<(i64, crate::tree::Tree)>)>,
    cache_rx: Receiver<(u64, Option<(i64, crate::tree::Tree)>)>,
    bg_tx: Sender<Bg>,
    bg_rx: Receiver<Bg>,
    layout: LayoutWorker,
    wake: walk::Wake,
    pub camera: Camera,
    frame_set: FrameSet,
    instances: Arc<Vec<Instance>>,
    opaque: u32,
    version: u64,
    world_version: u64,
    seen: (u64, [f32; 6]),
    pub lod: Lod,
    beams: Vec<Instance>,
    pub hover: Option<Hit>,
    pub selected: Option<(usize, NodeId)>,
    sel_box: Option<Aabb>,
    pub search: Option<Search>,
    pub search_text: String,
    search_index: usize,
    focus_search: bool,
    preview: Option<PreviewView>,
    pub mcp: McpState,
    machine: Option<Machine>,
    sampler: Option<hw::Sampler>,
    pub teleport_open: bool,
    pub settings_open: bool,
    pub sources_open: bool,
    pub help_open: bool,
    pub ask_mode_open: bool,
    toast: Option<(String, Instant)>,
    pub times: FrameTimes,
    render_stats: Option<Arc<Mutex<Stats>>>,
    has_gpu: bool,
    updater: Updater,
    captured: bool,
    started: Instant,
    last_frame: Instant,
    last_teleport: f32,
    frames: u64,
    queued_teleport: Option<(Vec3, f32, f32)>,
    placed_sides: Vec<f32>,
    view_rect: Rect,
    pub last_frame_instances: usize,
    /// The tests' flashing signal (0..1 over the picture; 0 in use).
    pub test_signal: f32,
    /// The window's size (kept for the next start).
    window_size: Option<[f32; 2]>,
}

fn now_secs() -> i64 {
    crate::layout::now_secs()
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, opts: StartOptions) -> App {
        let ctx = cc.egui_ctx.clone();
        let render_stats = cc.wgpu_render_state.as_ref().map(render::Renderer::install);
        App::with_context(&ctx, opts, render_stats)
    }

    pub fn with_context(
        ctx: &egui::Context,
        opts: StartOptions,
        render_stats: Option<Arc<Mutex<Stats>>>,
    ) -> App {
        let config = opts.config_dir.clone().unwrap_or_else(settings::config_dir);
        let settings_path = config.join(settings::FILE);
        let (settings, loaded) = settings::load(&settings_path);
        let theme = theme::find(&settings.theme);
        ctx.set_visuals(theme::visuals(theme));
        ctx.global_style_mut(|s| {
            s.spacing.item_spacing = vec2(8.0, 6.0);
            s.spacing.button_padding = vec2(8.0, 4.0);
        });
        let wake_ctx = ctx.clone();
        let wake: walk::Wake = Arc::new(move || wake_ctx.request_repaint());
        let (walk_tx, walk_rx) = channel();
        let (cache_tx, cache_rx) = channel();
        let (bg_tx, bg_rx) = channel();
        let layout = LayoutWorker::start(wake.clone());
        let mut camera = Camera {
            speed: settings.fly_speed,
            sensitivity: settings.mouse_sensitivity,
            invert_y: settings.invert_y,
            ..Camera::default()
        };
        camera.set_mode(Mode::Orbit);
        let has_gpu = render_stats.is_some();
        let mut updater = Updater::new(Install::current());
        if !opts.updates {
            // (tests and measurements: nothing goes to the network)
            updater.open = false;
        }
        let mut app = App {
            settings,
            settings_path,
            theme,
            islands: Vec::new(),
            next_id: 1,
            walk_tx,
            walk_rx,
            cache_tx,
            cache_rx,
            bg_tx,
            bg_rx,
            layout,
            wake,
            camera,
            frame_set: FrameSet::default(),
            instances: Arc::new(Vec::new()),
            opaque: 0,
            version: 0,
            world_version: 1,
            seen: (0, [0.0; 6]),
            lod: Lod::default(),
            beams: Vec::new(),
            hover: None,
            selected: None,
            sel_box: None,
            search: None,
            search_text: String::new(),
            search_index: 0,
            focus_search: false,
            preview: None,
            mcp: McpState::Off,
            machine: None,
            sampler: None,
            teleport_open: false,
            settings_open: false,
            sources_open: false,
            help_open: false,
            ask_mode_open: false,
            toast: None,
            times: FrameTimes::default(),
            render_stats,
            has_gpu,
            updater,
            captured: false,
            started: Instant::now(),
            last_frame: Instant::now(),
            last_teleport: -10.0,
            frames: 0,
            queued_teleport: None,
            placed_sides: Vec::new(),
            view_rect: Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)),
            last_frame_instances: 0,
            test_signal: 0.0,
            window_size: None,
            opts,
        };
        if let settings::Loaded::SetAside(p) = loaded {
            app.toast(format!(
                "The settings file could not be read; set aside as {}",
                p.display()
            ));
        }
        app.ask_mode_open = app.settings.ask_mode_before_flight;
        app.populate();
        app
    }

    /// Seconds since the start (the simulated timeline when the options
    /// give a step).
    pub fn time(&self) -> f32 {
        match self.opts.step {
            Some(step) => self.frames as f32 * step,
            None => self.started.elapsed().as_secs_f32(),
        }
    }

    fn toast(&mut self, text: String) {
        self.toast = Some((text, Instant::now()));
    }

    fn cache_dir(&self) -> Option<PathBuf> {
        if !self.opts.use_cache {
            return None;
        }
        Some(self.opts.cache_dir.clone().unwrap_or_else(cache::dir))
    }

    /// The islands of this start.
    fn populate(&mut self) {
        if self.opts.hardware && self.settings.hardware_island {
            let m = self.opts.machine.clone().unwrap_or_else(hw::detect);
            let hwi = hw::island(&m);
            let id = self.next_id;
            self.next_id += 1;
            let mut isl = Island::new(
                id,
                IslandSource::Hardware,
                "This machine".into(),
                Ring::Hardware,
            );
            isl.tree = hwi.tree.clone();
            let l = Arc::new(hwi.layout.clone());
            let boxes = Arc::new(city::boxes_of(&l));
            let bvh = Arc::new(crate::pick::Bvh::build(&boxes));
            isl.city = Some(IslandCity::new(Ring::Hardware, l, bvh, boxes));
            isl.hw = Some(hwi);
            isl.dirty = false;
            isl.status.done = true;
            self.islands.push(isl);
            self.machine = Some(m);
            if self.opts.machine.is_none() {
                self.sampler = Some(hw::Sampler::start(self.wake.clone()));
            }
        }
        let mut locals: Vec<(PathBuf, Option<mounts::Mount>)> = Vec::new();
        if !self.opts.paths.is_empty() {
            for p in &self.opts.paths {
                let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
                locals.push((p, None));
            }
        } else if self.opts.mounts && self.opts.synthetic.is_none() {
            let all = mounts::system_mounts();
            for m in mounts::visible(&all, self.settings.show_pseudo_mounts) {
                locals.push((m.mount_point.clone(), Some(m)));
            }
        }
        let all_mounts = if self.opts.paths.is_empty() && self.opts.mounts {
            mounts::system_mounts()
        } else {
            Vec::new()
        };
        for (root, mount) in locals {
            let title = match &mount {
                Some(m) => m.label(),
                None => root.to_string_lossy().into_owned(),
            };
            let skip = mounts::nested_below(&all_mounts, &root);
            let i = self.add_island(
                IslandSource::Local {
                    root,
                    mount: mount.clone(),
                },
                title,
                Ring::Local,
            );
            if let (Some(m), Some(machine)) = (&mount, &self.machine) {
                self.islands[i].disk = hw::disk_of(machine, m.dev);
            }
            self.start_walk(i, &skip);
        }
        if let Some(files) = self.opts.synthetic {
            let i = self.add_island(
                IslandSource::Synthetic { files, seed: 7 },
                format!("synthetic city ({files} files)"),
                Ring::Local,
            );
            // (it lives on the first disk, for the beam)
            if self.machine.as_ref().is_some_and(|m| !m.disks.is_empty()) {
                self.islands[i].disk = Some(0);
            }
            self.start_walk(i, &[]);
        }
        if self.opts.mcp && self.settings.mcp.enabled {
            self.connect_mcp();
        }
    }

    fn add_island(&mut self, source: IslandSource, title: String, ring: Ring) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.islands.push(Island::new(id, source, title, ring));
        self.islands.len() - 1
    }

    fn start_walk(&mut self, i: usize, skip: &[PathBuf]) {
        let cache_dir = self.cache_dir();
        let limits = world::limits(self.settings.walk_budget, self.settings.walk_workers);
        let isl = &mut self.islands[i];
        let Some(source) = isl.walk_source(skip) else {
            return;
        };
        isl.generation += 1;
        isl.saved = false;
        isl.requested.clear();
        isl.status = walk::Status::default();
        let root_name = isl.tree.root().name.clone();
        if isl.city.is_some() || isl.from_cache.is_some() {
            // a city stands: the new walk goes beside it
            isl.fresh = Some(crate::tree::Tree::new(&root_name, 0));
        } else {
            isl.replace_tree(crate::tree::Tree::new(&root_name, 0));
            isl.fresh = None;
            if let Some(dir) = cache_dir.filter(|_| isl.generation == 1) {
                // the cached city first, the walk beside it
                isl.fresh = Some(crate::tree::Tree::new(&root_name, 0));
                world::load_cache(
                    dir,
                    isl.source.key(),
                    isl.id,
                    self.cache_tx.clone(),
                    self.wake.clone(),
                );
            }
        }
        isl.walker = Some(Walker::start(
            isl.id,
            isl.generation,
            source,
            limits,
            self.walk_tx.clone(),
            self.wake.clone(),
        ));
    }

    fn connect_mcp(&mut self) {
        self.mcp = McpState::Looking;
        let tx = self.bg_tx.clone();
        let wake = self.wake.clone();
        let configured = self.settings.mcp.command.clone();
        let allow = self.settings.mcp.allow_uvx_download;
        let open = self.settings.mcp.open_profiles.clone();
        let _ = std::thread::Builder::new()
            .name("afm-mcp".into())
            .spawn(move || {
                let found = mcp::find_server(Some(&configured), allow);
                let Some((cmd, label)) = found else {
                    let _ = tx.send(Bg::Mcp(Err(None)));
                    wake();
                    return;
                };
                let timeout = if label.starts_with("uvx") {
                    Duration::from_secs(90)
                } else {
                    Duration::from_secs(30)
                };
                let client = match Client::spawn(&cmd, timeout) {
                    Ok(c) => Ok(c),
                    Err(_) if cmd.iter().any(|a| a == "--mcp-tools") => {
                        Client::spawn(&mcp::without_groups(&cmd), timeout)
                    }
                    Err(e) => Err(e),
                };
                let result = client.and_then(|c| {
                    let c = Arc::new(c);
                    for p in &open {
                        let _ = c.open_profile(p, Duration::from_secs(60));
                    }
                    let backends = c.backends(Duration::from_secs(30))?;
                    let profiles = c.profiles(Duration::from_secs(30)).unwrap_or_default();
                    Ok((c, label, backends, profiles))
                });
                let _ = tx.send(Bg::Mcp(result.map_err(Some)));
                wake();
            });
    }

    fn add_backends(&mut self, client: &Arc<Client>, backends: Vec<Backend>) {
        for b in backends {
            if b.class == "LocalFS" && !b.remote && !self.settings.mcp.show_local_backend {
                continue;
            }
            let exists = self.islands.iter().any(|isl| matches!(&isl.source, IslandSource::Mcp { backend, .. } if backend.id == b.id));
            if exists {
                continue;
            }
            let title = match (&b.profile, &b.protocol) {
                (Some(p), Some(proto)) => format!("{p}  ({proto}, axross)"),
                _ => format!(
                    "{}  ({}, axross)",
                    if b.name.is_empty() { &b.id } else { &b.name },
                    b.class
                ),
            };
            let i = self.add_island(
                IslandSource::Mcp {
                    client: client.clone(),
                    backend: b,
                    root: "/".into(),
                },
                title,
                Ring::Remote,
            );
            self.start_walk(i, &[]);
        }
    }

    // ------------------------------------------------------------------
    // what streams in
    // ------------------------------------------------------------------

    /// Takes what the threads sent, at most `budget` of time.
    pub fn pump(&mut self, budget: Duration) {
        let start = Instant::now();
        // cached cities
        while let Ok((id, got)) = self.cache_rx.try_recv() {
            let Some(isl) = self.islands.iter_mut().find(|i| i.id == id) else {
                continue;
            };
            match got {
                // (the walk still under way and the cache holds more: the
                // cached city stands until the walk is done)
                Some((saved, tree))
                    if isl.fresh.is_some()
                        && !isl.status.done
                        && isl.fresh.as_ref().map_or(0, |f| f.len()) < tree.len() =>
                {
                    isl.replace_tree(tree);
                    isl.from_cache = Some(saved);
                }
                _ => {
                    // no cache (or the walk was quicker): the walk's tree
                    // is shown as it grows
                    if isl.from_cache.is_none()
                        && let Some(f) = isl.fresh.take()
                    {
                        isl.replace_tree(f);
                    }
                }
            }
        }
        // walks
        let mut changed = Vec::new();
        let mut swapped: Vec<usize> = Vec::new();
        while start.elapsed() < budget {
            let Ok(msg) = self.walk_rx.try_recv() else {
                break;
            };
            let Some(i) = self.islands.iter().position(|isl| isl.id == msg.island) else {
                continue;
            };
            let isl = &mut self.islands[i];
            if msg.generation != isl.generation {
                continue;
            }
            match msg.event {
                Event::Batch(b) => {
                    let shown = isl.fresh.is_none();
                    if !isl.walk_tree().apply(b) {
                        // out of step (never expected): the walk starts over
                        isl.walker = None;
                        continue;
                    }
                    if shown {
                        isl.dirty = true;
                        changed.push(i);
                    }
                }
                Event::Status(s) => {
                    let done = s.done;
                    isl.status = s;
                    if done && isl.fresh.is_some() {
                        if isl.status.error.is_some() && isl.from_cache.is_some() {
                            // (the folder cannot be read now - unmounted,
                            // say: the cached city stays, marked so)
                            isl.fresh = None;
                            isl.saved = true;
                        } else {
                            // the new walk is complete: it replaces the
                            // cached city (a cache still on its way is
                            // dropped when it comes)
                            if let Some(f) = isl.fresh.take() {
                                isl.replace_tree(f);
                            }
                            isl.from_cache = None;
                            swapped.push(i);
                        }
                    }
                    if done && isl.fresh.is_none() && !isl.saved {
                        isl.saved = true;
                        self.save_cache(i);
                    }
                }
            }
        }
        for &i in &swapped {
            // what pointed into the old tree points nowhere now
            if self.selected.is_some_and(|(s, _)| s == i) {
                self.selected = None;
                self.sel_box = None;
                self.preview = None;
            }
            if let Some(s) = self.search.as_mut() {
                s.reset_island(i);
            }
            self.hover = None;
        }
        if !changed.is_empty()
            && let Some(s) = self.search.as_mut()
        {
            s.resume();
        }
        // layouts
        while let Ok(done) = self.layout.rx.try_recv() {
            let Some(isl) = self.islands.iter_mut().find(|i| i.id == done.id) else {
                continue;
            };
            isl.layout_pending = false;
            if done.generation != isl.epoch || done.layout.nodes > isl.tree.len() {
                isl.dirty = true;
                continue;
            }
            isl.stale = false;
            let ring = isl.ring;
            isl.city = Some(IslandCity::new(
                ring,
                Arc::new(done.layout),
                Arc::new(done.bvh),
                Arc::new(done.boxes),
            ));
            self.world_version += 1;
            self.placed_sides.clear();
        }
        // ask for layouts
        for isl in &mut self.islands {
            if isl.dirty
                && !isl.layout_pending
                && isl.hw.is_none()
                && isl.last_layout.elapsed() >= world::layout_interval(isl.tree.len())
            {
                isl.dirty = false;
                isl.layout_pending = true;
                isl.last_layout = Instant::now();
                self.layout.ask(Job {
                    id: isl.id,
                    generation: isl.epoch,
                    nodes: snapshot(&isl.tree),
                });
            }
        }
        // the rest
        while let Ok(bg) = self.bg_rx.try_recv() {
            match bg {
                Bg::Mcp(Ok((client, label, backends, profiles))) => {
                    self.mcp = McpState::Connected {
                        label,
                        client: client.clone(),
                        profiles,
                    };
                    self.add_backends(&client, backends);
                }
                Bg::Mcp(Err(None)) => self.mcp = McpState::NotFound,
                Bg::Mcp(Err(Some(e))) => self.mcp = McpState::Failed(e),
                Bg::Backends(client, Ok(b)) => self.add_backends(&client, b),
                Bg::Backends(_, Err(e)) => self.toast(format!("axross: {e}")),
                Bg::Preview(island, node, p) => {
                    if let Some(v) = self.preview.as_mut()
                        && v.island == island
                        && v.node == node
                    {
                        v.loading = false;
                        v.preview = Some(*p);
                        v.texture = None;
                    }
                }
                Bg::Opened(Ok(how)) => self.toast(format!("Opened with {how}")),
                Bg::Opened(Err(e)) => self.toast(e),
            }
        }
        // the machine's live light
        if let Some(sampler) = self.sampler.as_mut()
            && let Some((loads, mem)) = sampler.poll().cloned()
        {
            self.apply_load(&loads, mem);
        }
        self.place_islands();
    }

    /// The processor's load as its towers' light, the memory's in use.
    pub fn apply_load(&mut self, loads: &[f32], mem: f32) {
        let Some(isl) = self.islands.iter_mut().find(|i| i.hw.is_some()) else {
            return;
        };
        let (Some(hwi), Some(c)) = (isl.hw.as_ref(), isl.city.as_mut()) else {
            return;
        };
        for (node, part) in &hwi.parts {
            let Some(Placed::Building(b)) = c.layout.placed.get(*node as usize).copied() else {
                continue;
            };
            let target = match part {
                Part::Core(id) => loads.get(*id as usize).copied().unwrap_or(0.0),
                Part::Module | Part::MemoryInUse => mem,
                _ => continue,
            };
            // eased: the light follows the load slowly (never a flash)
            if let Some(inst) = c.buildings.get_mut(b as usize) {
                let old = ((inst.info >> 8) & 0xff) as f32 / 255.0;
                inst.set_glow(old + (target - old) * 0.5);
            }
        }
        self.world_version += 1;
    }

    fn save_cache(&self, i: usize) {
        let Some(dir) = self.cache_dir() else { return };
        let isl = &self.islands[i];
        if matches!(isl.source, IslandSource::Hardware) || isl.tree.len() < 2 {
            return;
        }
        let bytes = cache::encode(&isl.tree, &isl.source.key(), now_secs());
        let key = isl.source.key();
        let _ = std::thread::Builder::new()
            .name("afm-cache-save".into())
            .spawn(move || {
                let _ = cache::save(&dir, &key, &bytes);
            });
    }

    /// Saves every island's cache and the settings (at the end).
    pub fn save_all(&mut self) {
        if let Some(dir) = self.cache_dir() {
            for isl in &self.islands {
                if matches!(isl.source, IslandSource::Hardware)
                    || isl.tree.len() < 2
                    || isl.fresh.is_some()
                {
                    continue;
                }
                let bytes = cache::encode(&isl.tree, &isl.source.key(), now_secs());
                let _ = cache::save(&dir, &isl.source.key(), &bytes);
            }
        }
        self.settings.fly_speed = self.camera.speed;
        if let Some(size) = self.window_size {
            self.settings.window = Some(size);
        }
        let _ = settings::save(&self.settings_path, &self.settings);
    }

    /// Places the islands when a side changed; rebuilds their instances.
    fn place_islands(&mut self) {
        let sides: Vec<f32> = self
            .islands
            .iter()
            .map(|i| i.city.as_ref().map_or(0.0, |c| c.side()))
            .collect();
        if sides == self.placed_sides {
            return;
        }
        self.placed_sides = sides;
        let with_city: Vec<usize> = (0..self.islands.len())
            .filter(|&i| self.islands[i].city.is_some())
            .collect();
        let input: Vec<(Ring, f32)> = with_city
            .iter()
            .map(|&i| (self.islands[i].ring, self.placed_sides[i]))
            .collect();
        let offsets = city::place(&input);
        let theme = self.theme;
        for (k, &i) in with_city.iter().enumerate() {
            if let Some(c) = self.islands[i].city.as_mut() {
                c.offset = offsets[k];
                c.rebuild(theme);
            }
        }
        // the beams from the disks to their islands
        self.beams.clear();
        let hw_index = self.islands.iter().position(|i| i.hw.is_some());
        if let Some(h) = hw_index {
            let mut disk_at: Vec<(usize, Vec3)> = Vec::new();
            if let (Some(hwi), Some(hc)) =
                (self.islands[h].hw.as_ref(), self.islands[h].city.as_ref())
            {
                for (node, part) in &hwi.parts {
                    if let (Part::Disk(d), Some(Placed::Building(b))) =
                        (part, hc.layout.placed.get(*node as usize))
                    {
                        let c = hc.boxes[*b as usize].center();
                        disk_at.push((*d, vec3(c.x + hc.offset[0], 0.0, c.z + hc.offset[1])));
                    }
                }
            }
            for isl in &self.islands {
                let (Some(d), Some(c)) = (isl.disk, isl.city.as_ref()) else {
                    continue;
                };
                let Some((_, from)) = disk_at.iter().find(|(k, _)| *k == d) else {
                    continue;
                };
                let to = c.center();
                // (to the island's edge facing the machine)
                let dir = (to - *from).normalized();
                let end = to - dir * (c.side() * 0.5);
                self.beams
                    .push(city::beam(*from, end, city::pack_color(self.theme.accent)));
            }
        }
        self.world_version += 1;
    }

    // ------------------------------------------------------------------
    // the view
    // ------------------------------------------------------------------

    fn cities(&self) -> Vec<&IslandCity> {
        self.islands
            .iter()
            .filter_map(|i| i.city.as_ref())
            .collect()
    }

    /// Index of island k-th with a city (as `cities()` orders them) in
    /// `self.islands`.
    fn island_of_city(&self, k: usize) -> Option<usize> {
        (0..self.islands.len())
            .filter(|&i| self.islands[i].city.is_some())
            .nth(k)
    }

    fn world_box(&self) -> Option<Aabb> {
        self.cities()
            .iter()
            .map(|c| c.aabb())
            .reduce(|a, b| a.union(b))
    }

    /// Looks at the whole world from above and aside.
    pub fn overview(&mut self, fly: bool) {
        let Some(b) = self.world_box() else { return };
        let c = b.center();
        let size = (b.max.x - b.min.x).max(b.max.z - b.min.z).max(100.0);
        let pos = vec3(c.x, size * 0.55, c.z + size * 0.75);
        let target = vec3(c.x, 0.0, c.z);
        let (yaw, pitch) = look_angles(pos, target);
        if fly {
            let cities = self.cities();
            let ceiling = |a: Vec3, b: Vec3| city::ceiling(&cities, a, b);
            let mut cam = self.camera.clone();
            cam.fly_path(
                pos,
                (yaw, pitch),
                &ceiling,
                self.settings.reduce_motion,
                Some(target),
            );
            self.camera = cam;
        } else {
            self.camera.pos = pos;
            self.camera.yaw = yaw;
            self.camera.pitch = pitch;
            self.camera.target = target;
            self.camera.distance = (pos - target).length();
        }
    }

    /// Places the camera (tests, screenshots).
    pub fn look_from(&mut self, pos: Vec3, target: Vec3) {
        let (yaw, pitch) = look_angles(pos, target);
        self.camera.flight = None;
        self.camera.pos = pos;
        self.camera.yaw = yaw;
        self.camera.pitch = pitch;
        self.camera.target = target;
        self.camera.distance = (pos - target).length();
    }

    /// The world box of an island's node.
    pub fn node_box(&self, island: usize, node: NodeId) -> Option<Aabb> {
        let isl = self.islands.get(island)?;
        let c = isl.city.as_ref()?;
        let placed = city::placed_ancestor(c, &isl.tree, node)?;
        city::node_box(c, placed)
    }

    /// Flies to an island's node and chooses it.
    pub fn fly_to(&mut self, island: usize, node: NodeId) {
        let Some(b) = self.node_box(island, node) else {
            return;
        };
        self.select(island, node);
        let size = (b.max - b.min).length();
        let cities = self.cities();
        let ceiling = |a: Vec3, b: Vec3| city::ceiling(&cities, a, b);
        let mut cam = self.camera.clone();
        cam.fly_to(b.center(), size, &ceiling, self.settings.reduce_motion);
        self.camera = cam;
    }

    /// Jumps to an island's node at once (with the warp).
    pub fn teleport_to(&mut self, island: usize, node: NodeId) {
        let Some(b) = self.node_box(island, node) else {
            return;
        };
        self.select(island, node);
        let c = b.center();
        let size = (b.max - b.min).length().max(12.0);
        let mut pos = c + vec3(size * 0.35, size * 0.55, size * 0.7);
        let cities = self.cities();
        pos.y = pos.y.max(city::ceiling(&cities, pos, pos) + 3.0);
        let (yaw, pitch) = look_angles(pos, c);
        self.teleport(pos, yaw, pitch);
        if self.camera.mode == Mode::Orbit {
            self.camera.target = c;
            self.camera.distance = (pos - c).length();
        }
    }

    /// A teleport, at most two a second (more wait their turn: the warp
    /// never strobes).
    pub fn teleport(&mut self, pos: Vec3, yaw: f32, pitch: f32) {
        if self.time() - self.last_teleport < 0.5 {
            self.queued_teleport = Some((pos, yaw, pitch));
            return;
        }
        self.last_teleport = self.time();
        let warp = !self.settings.reduce_motion;
        let mode = self.camera.mode;
        self.camera.teleport(pos, yaw, pitch, warp);
        if mode == Mode::Orbit {
            self.camera.target = pos + crate::camera::forward_of(yaw, pitch) * self.camera.distance;
        }
    }

    pub fn select(&mut self, island: usize, node: NodeId) {
        self.selected = Some((island, node));
        self.sel_box = self.node_box(island, node);
        self.start_preview(island, node);
    }

    fn start_preview(&mut self, island: usize, node: NodeId) {
        let Some(isl) = self.islands.get(island) else {
            return;
        };
        let Some(n) = isl.tree.get(node) else { return };
        let id = isl.id;
        self.preview = Some(PreviewView {
            island: id,
            node,
            loading: true,
            preview: None,
            texture: None,
        });
        let at = match &isl.source {
            IslandSource::Local { root, .. } => Some(Where::Local(isl.tree.local_path(root, node))),
            IslandSource::Mcp {
                client,
                backend,
                root,
            } => Some(Where::Remote {
                client: client.clone(),
                backend: backend.id.clone(),
                path: isl.tree.remote_path(root, node).into_bytes(),
            }),
            _ => None,
        };
        let Some(at) = at else {
            if let Some(v) = self.preview.as_mut() {
                v.loading = false;
            }
            return;
        };
        let name = n.name.to_string_lossy().into_owned();
        let (size, is_dir) = (n.size, n.kind == Kind::Dir);
        let tx = self.bg_tx.clone();
        let wake = self.wake.clone();
        let _ = std::thread::Builder::new()
            .name("afm-preview".into())
            .spawn(move || {
                let p = preview::load(&at, &name, size, is_dir);
                let _ = tx.send(Bg::Preview(id, node, Box::new(p)));
                wake();
            });
    }

    /// Runs a search (from the field, or tests).
    pub fn run_search(&mut self, query: &str) {
        self.search_text = query.to_string();
        self.search = if query.trim().is_empty() {
            None
        } else {
            Some(Search::new(query))
        };
        self.search_index = 0;
        self.world_version += 1;
    }

    fn step_search(&mut self, budget: Duration) {
        let Some(s) = self.search.as_mut() else {
            return;
        };
        if s.done {
            return;
        }
        let trees: Vec<Option<&crate::tree::Tree>> =
            self.islands.iter().map(|i| Some(&i.tree)).collect();
        let before = s.hits.len();
        s.step(&trees, budget);
        if s.hits.len() != before {
            self.world_version += 1;
        }
    }

    /// Goes to the next search hit.
    pub fn next_hit(&mut self, teleport: bool) {
        let Some(s) = self.search.as_ref() else {
            return;
        };
        if s.hits.is_empty() {
            return;
        }
        let (i, n) = s.hits[self.search_index % s.hits.len()];
        self.search_index = (self.search_index + 1) % s.hits.len();
        if teleport {
            self.teleport_to(i, n);
        } else {
            self.fly_to(i, n);
        }
    }

    /// The pillars of light over the search hits (the first 400 drawn).
    fn pillars(&self) -> Vec<Instance> {
        let Some(s) = self.search.as_ref() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for &(i, n) in s.hits.iter().take(400) {
            let Some(b) = self.node_box(i, n) else {
                continue;
            };
            let c = b.center();
            let w = ((b.max.x - b.min.x).min(b.max.z - b.min.z) * 0.4).clamp(0.8, 8.0);
            out.push(Instance::new(
                city::Shape::Pillar,
                vec3(c.x - w * 0.5, b.max.y, c.z - w * 0.5),
                vec3(w, 120.0 + (b.max.y - b.min.y), w),
                0,
                1.0,
                0,
            ));
        }
        out
    }

    /// The level of detail for this frame, when the camera or the world
    /// changed.
    fn update_frame_set(&mut self, aspect: f32) -> (Frustum, Globals) {
        let view = self.camera.view();
        let proj = self.camera.proj(aspect);
        let vp = proj.mul(&view);
        let frustum = Frustum::from_view_proj(&vp);
        let key = [
            self.camera.pos.x,
            self.camera.pos.y,
            self.camera.pos.z,
            self.camera.yaw,
            self.camera.pitch,
            aspect + self.camera.fov_y,
        ];
        if self.seen != (self.world_version, key) {
            if self.seen.0 != self.world_version {
                self.sel_box = self.selected.and_then(|(i, n)| self.node_box(i, n));
            }
            self.seen = (self.world_version, key);
            let eye = self.camera.pos;
            let lod = self.lod;
            // (last frame's buffer, once the renderer let go of it: no new
            // allocation of megabytes each frame)
            if let Ok(v) = Arc::try_unwrap(std::mem::take(&mut self.instances)) {
                self.frame_set.opaque = v;
            }
            {
                let mut cities: Vec<&mut IslandCity> = self
                    .islands
                    .iter_mut()
                    .filter_map(|i| i.city.as_mut())
                    .collect();
                city::select(&mut cities, eye, &frustum, &lod, &mut self.frame_set);
            }
            // unread folders near: asked for
            let want = std::mem::take(&mut self.frame_set.want);
            for (k, node) in &want {
                if let Some(i) = self.island_of_city(*k) {
                    let isl = &mut self.islands[i];
                    if isl.can_ask() && isl.requested.insert(*node) {
                        if let Some(w) = isl.walker.as_ref() {
                            w.list(*node);
                        }
                        if let Some(n) = isl.tree.nodes.get_mut(*node as usize) {
                            n.listing = Listing::Pending;
                        }
                    }
                }
            }
            self.frame_set.want = want;
            let mut all = std::mem::take(&mut self.frame_set.opaque);
            let opaque = all.len() as u32;
            all.extend_from_slice(&self.beams);
            all.extend(self.pillars());
            self.opaque = opaque;
            self.last_frame_instances = all.len();
            self.instances = Arc::new(all);
            self.version += 1;
        }
        let inv = invert(&vp);
        let t = self.theme;
        let warp = if self.camera.warp > 0.0 {
            let k = 1.0 - self.camera.warp / crate::camera::WARP_SECONDS;
            (std::f32::consts::PI * k).sin()
        } else {
            0.0
        };
        let bx = |b: Option<Aabb>| match b {
            Some(b) => (
                [b.min.x, b.min.y, b.min.z, 1.0],
                [b.max.x, b.max.y, b.max.z, 1.0],
            ),
            None => ([0.0; 4], [0.0; 4]),
        };
        let (hmin, hmax) = bx(self.hover.map(|h| h.aabb));
        let (smin, smax) = bx(self.sel_box);
        let s = &self.settings;
        let motion = if s.reduce_motion { 0.0 } else { 1.0 };
        let g = Globals {
            view_proj: vp.to_cols(),
            inv_view_proj: inv,
            eye: [
                self.camera.pos.x,
                self.camera.pos.y,
                self.camera.pos.z,
                (self.opts.clock.unwrap_or(0.0) + self.time()) % 3600.0,
            ],
            fog: [t.fog[0], t.fog[1], t.fog[2], 0.00055],
            sky_top: [
                t.sky_top[0],
                t.sky_top[1],
                t.sky_top[2],
                if s.rain { t.rain_amount } else { 0.0 },
            ],
            sky_horizon: [
                t.sky_horizon[0],
                t.sky_horizon[1],
                t.sky_horizon[2],
                if s.scanlines { t.scanlines } else { 0.0 },
            ],
            rain: [
                t.rain[0],
                t.rain[1],
                t.rain[2],
                if s.bloom { t.bloom } else { 0.0 },
            ],
            edge: [t.edge[0], t.edge[1], t.edge[2], 1.0],
            grid: [t.grid[0], t.grid[1], t.grid[2], motion],
            highlight: [t.highlight[0], t.highlight[1], t.highlight[2], warp],
            hover_min: hmin,
            hover_max: hmax,
            sel_min: smin,
            sel_max: smax,
            viewport: [0.0; 4],
            params: [0.0, 1.35, 0.0, 0.0],
            debug: [self.test_signal, 0.0, 0.0, 0.0],
        };
        (frustum, g)
    }

    /// The camera's input from egui this frame.
    fn camera_input(&mut self, ui: &Ui, response: &egui::Response) -> Input {
        let mut input = Input::default();
        let typing = ui.ctx().egui_wants_keyboard_input();
        let (keys, mods, scroll, delta, motion) = ui.input(|i| {
            let k = |key| i.key_down(key);
            (
                [
                    k(Key::W) || k(Key::ArrowUp),
                    k(Key::S) || k(Key::ArrowDown),
                    k(Key::A) || k(Key::ArrowLeft),
                    k(Key::D) || k(Key::ArrowRight),
                    k(Key::E) || k(Key::PageUp),
                    k(Key::Q) || k(Key::PageDown),
                ],
                i.modifiers,
                i.smooth_scroll_delta.y,
                i.pointer.delta(),
                i.pointer.motion(),
            )
        });
        if !typing && !mods.ctrl && !mods.command {
            input.move_dir = [
                (keys[3] as i32 - keys[2] as i32) as f32,
                (keys[4] as i32 - keys[5] as i32) as f32,
                (keys[0] as i32 - keys[1] as i32) as f32,
            ];
            input.boost = mods.shift;
        }
        if response.hovered() || self.captured {
            input.scroll = scroll / 50.0;
        }
        let look = motion.unwrap_or(delta);
        match self.camera.mode {
            Mode::Fly => {
                if self.captured || response.dragged_by(egui::PointerButton::Secondary) {
                    input.look = [look.x, look.y];
                }
            }
            Mode::Orbit => {
                let turn = (response.dragged_by(egui::PointerButton::Primary) && !mods.shift)
                    || response.dragged_by(egui::PointerButton::Secondary);
                if turn {
                    input.orbit = [delta.x, delta.y];
                }
                if response.dragged_by(egui::PointerButton::Middle)
                    || (response.dragged_by(egui::PointerButton::Primary) && mods.shift)
                {
                    input.pan = [delta.x, delta.y];
                }
            }
        }
        input
    }

    fn keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                ctx.memory_mut(|m| m.request_focus(egui::Id::NULL));
            }
            return;
        }
        let (pressed, mods) = ctx.input(|i| {
            let keys = [
                Key::F,
                Key::T,
                Key::Home,
                Key::H,
                Key::F1,
                Key::Slash,
                Key::N,
                Key::O,
                Key::Escape,
                Key::M,
                Key::Enter,
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
                Key::Num8,
                Key::Num9,
            ];
            let p: Vec<Key> = keys.into_iter().filter(|k| i.key_pressed(*k)).collect();
            (p, i.modifiers)
        });
        for k in pressed {
            match k {
                Key::F if mods.command => self.focus_search = true,
                Key::F => {
                    let m = if self.camera.mode == Mode::Fly {
                        Mode::Orbit
                    } else {
                        Mode::Fly
                    };
                    self.camera.set_mode(m);
                }
                Key::Slash => self.focus_search = true,
                Key::T => self.teleport_open = !self.teleport_open,
                Key::Home => self.overview(true),
                Key::H | Key::F1 => self.help_open = !self.help_open,
                Key::N | Key::Enter => self.next_hit(mods.shift),
                Key::O => self.open_selected(!mods.shift, ctx),
                Key::M => self.set_capture(ctx, !self.captured),
                Key::Escape => {
                    if self.captured {
                        self.set_capture(ctx, false);
                    } else {
                        self.selected = None;
                        self.sel_box = None;
                        self.preview = None;
                    }
                }
                k => {
                    let n = match k {
                        Key::Num1 => 1,
                        Key::Num2 => 2,
                        Key::Num3 => 3,
                        Key::Num4 => 4,
                        Key::Num5 => 5,
                        Key::Num6 => 6,
                        Key::Num7 => 7,
                        Key::Num8 => 8,
                        Key::Num9 => 9,
                        _ => continue,
                    };
                    if mods.command || mods.ctrl {
                        self.save_spot(n);
                    } else {
                        self.jump_spot(n);
                    }
                }
            }
        }
    }

    fn set_capture(&mut self, ctx: &egui::Context, on: bool) {
        self.captured = on;
        if on {
            self.camera.set_mode(Mode::Fly);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(if on {
            egui::CursorGrab::Locked
        } else {
            egui::CursorGrab::None
        }));
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(!on));
    }

    pub fn save_spot(&mut self, n: usize) {
        let label = match self.selected {
            Some((i, node)) => self
                .islands
                .get(i)
                .and_then(|isl| isl.tree.get(node))
                .map(|x| display_name(&x.name, 60))
                .unwrap_or_default(),
            None => format!("spot {n}"),
        };
        if let Some(slot) = self.settings.bookmarks.get_mut(n - 1) {
            *slot = Some(Spot {
                pos: self.camera.pos.to_array(),
                yaw: self.camera.yaw,
                pitch: self.camera.pitch,
                label: label.clone(),
            });
        }
        let _ = settings::save(&self.settings_path, &self.settings);
        self.toast(format!("Saved spot {n}: {label}"));
    }

    pub fn jump_spot(&mut self, n: usize) {
        let Some(Some(s)) = self.settings.bookmarks.get(n - 1).cloned() else {
            self.toast(format!("No spot {n} yet (Ctrl+{n} saves one)"));
            return;
        };
        self.teleport(vec3(s.pos[0], s.pos[1], s.pos[2]), s.yaw, s.pitch);
    }

    fn open_selected(&mut self, axross_first: bool, _ctx: &egui::Context) {
        let Some((i, node)) = self.selected else {
            return;
        };
        let Some(isl) = self.islands.get(i) else {
            return;
        };
        let launch = match &isl.source {
            IslandSource::Local { root, .. } => {
                let path = isl.tree.local_path(root, node);
                let ax = if axross_first {
                    crate::open::in_axross(&path)
                } else {
                    None
                };
                ax.or_else(|| crate::open::system(&path))
                    .map(|l| (l, Some(path)))
            }
            IslandSource::Mcp { backend, root, .. } => {
                let path = isl.tree.remote_path(root, node);
                match (&backend.profile, &backend.protocol) {
                    (Some(p), Some(proto)) => {
                        crate::open::remote_in_axross(proto, p, &path).map(|l| (l, None))
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        let Some((launch, fallback)) = launch else {
            self.toast("Nothing to open it with here".into());
            return;
        };
        let tx = self.bg_tx.clone();
        let wake = self.wake.clone();
        let how = launch.label.clone();
        let is_axross = how == "axross";
        crate::open::spawn(&launch, move |r| {
            let r = match r {
                Ok(()) => Ok(how),
                // (axross that takes no path yet: the system's program instead)
                Err(e) if is_axross => match fallback.as_deref().and_then(crate::open::system) {
                    Some(sys) => {
                        let (tx2, rx2) = channel();
                        crate::open::spawn(&sys, move |r| {
                            let _ = tx2.send(r);
                        });
                        match rx2.recv_timeout(Duration::from_secs(5)) {
                            Ok(Ok(())) => Ok(format!("the default program ({e})")),
                            Ok(Err(e2)) => Err(e2),
                            Err(_) => Ok("the default program".into()),
                        }
                    }
                    None => Err(e),
                },
                Err(e) => Err(e),
            };
            let _ = tx.send(Bg::Opened(r));
            wake();
        });
    }

    // ------------------------------------------------------------------
    // drawing
    // ------------------------------------------------------------------

    fn view(&mut self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        self.view_rect = rect;
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let dt = self
            .opts
            .step
            .unwrap_or_else(|| self.last_frame.elapsed().as_secs_f32());
        self.last_frame = Instant::now();
        self.frames += 1;
        let input = self.camera_input(ui, &response);
        {
            let cities = self.cities();
            let solid = |p: Vec3| city::solid(&cities, p);
            let mut cam = self.camera.clone();
            cam.update(dt, &input, &solid);
            self.camera = cam;
        }
        if let Some((p, y, pi)) = self.queued_teleport
            && self.time() - self.last_teleport >= 0.5
        {
            self.queued_teleport = None;
            self.teleport(p, y, pi);
        }
        let aspect = rect.width() / rect.height().max(1.0);
        // hover and click
        let pointer = response.hover_pos().filter(|_| !self.captured);
        self.hover = pointer.and_then(|p| {
            let ndc = [
                (p.x - rect.left()) / rect.width() * 2.0 - 1.0,
                1.0 - (p.y - rect.top()) / rect.height() * 2.0,
            ];
            let ray = self.camera.ray(ndc, aspect);
            let cities = self.cities();
            city::pick(&cities, &ray).map(|h| Hit {
                island: self.island_of_city(h.island).unwrap_or(0),
                ..h
            })
        });
        if self
            .hover
            .is_some_and(|h| self.islands.get(h.island).is_none_or(|i| i.stale))
        {
            self.hover = None;
        }
        if response.clicked_by(egui::PointerButton::Primary)
            && let Some(h) = self.hover
        {
            if self.camera.mode == Mode::Orbit {
                self.fly_to(h.island, h.node);
            } else {
                self.select(h.island, h.node);
            }
        }
        if response.double_clicked()
            && let Some(h) = self.hover
        {
            self.fly_to(h.island, h.node);
        }
        let (_, globals) = self.update_frame_set(aspect);
        let frame = Arc::new(FrameData {
            globals,
            instances: self.instances.clone(),
            opaque: self.opaque,
            version: self.version,
            guard: self.opts.flash_guard,
            dt,
        });
        if self.has_gpu {
            ui.painter().add(render::callback(rect, frame));
        } else {
            ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
        }
        self.labels(ui, rect, aspect);
        if self.camera.mode == Mode::Fly {
            // a small crosshair
            let c = rect.center();
            let col = self.theme.muted.gamma_multiply(0.7);
            ui.painter()
                .line_segment([c - vec2(6.0, 0.0), c + vec2(6.0, 0.0)], (1.0, col));
            ui.painter()
                .line_segment([c - vec2(0.0, 6.0), c + vec2(0.0, 6.0)], (1.0, col));
        }
        if let Some(h) = self.hover
            && !response.dragged()
            && let Some(p) = pointer
        {
            let text = self.node_title(h.island, h.node);
            let font = egui::FontId::monospace(12.0);
            let galley = ui.painter().layout_no_wrap(text, font, self.theme.strong);
            let at = p + vec2(14.0, 10.0);
            let bg = Rect::from_min_size(at - vec2(4.0, 2.0), galley.size() + vec2(8.0, 4.0));
            ui.painter()
                .rect_filled(bg, 3.0, self.theme.surface.gamma_multiply(0.92));
            ui.painter().galley(at, galley, self.theme.strong);
        }
        if self.camera.flying()
            || self.camera.warp > 0.0
            || input.move_dir != [0.0; 3]
            || response.dragged()
            || self.search.as_ref().is_some_and(|s| !s.done)
        {
            ui.ctx().request_repaint();
        } else {
            // (the rain and the light keep moving, slowly)
            ui.ctx()
                .request_repaint_after(Duration::from_millis(if self.settings.reduce_motion {
                    500
                } else {
                    33
                }));
        }
    }

    /// "name  (size)" for the hover tip.
    fn node_title(&self, island: usize, node: NodeId) -> String {
        let Some(isl) = self.islands.get(island) else {
            return String::new();
        };
        let Some(n) = isl.tree.get(node) else {
            return String::new();
        };
        let name = if node == 0 {
            isl.title.clone()
        } else {
            display_name(&n.name, 60)
        };
        if isl.hw.is_some() {
            return name;
        }
        match n.kind {
            Kind::Dir => format!("{name}/  {} in {} files", hw::human_bytes(n.total), n.count),
            _ => format!("{name}  {}", hw::human_bytes(n.size)),
        }
    }

    fn labels(&mut self, ui: &Ui, rect: Rect, aspect: f32) {
        if !self.settings.labels {
            return;
        }
        let vp = self.camera.proj(aspect).mul(&self.camera.view());
        let mut cands = self.frame_set.labels.clone();
        cands.sort_by(|a, b| {
            let ka = (a.kind == LabelKind::Island) as u8;
            let kb = (b.kind == LabelKind::Island) as u8;
            kb.cmp(&ka).then(b.weight.total_cmp(&a.weight))
        });
        let mut taken: Vec<Rect> = Vec::new();
        let painter = ui.painter_at(rect);
        let t = self.theme;
        let cities = self.cities();
        let eye = self.camera.pos;
        // the chosen one first, whatever is in front of it
        if let (Some((i, n)), Some(b)) = (self.selected, self.sel_box)
            && let Some(isl) = self.islands.get(i)
            && let Some(node) = isl.tree.get(n)
        {
            let top = vec3(
                (b.min.x + b.max.x) * 0.5,
                b.max.y + 0.5,
                (b.min.z + b.max.z) * 0.5,
            );
            let p = vp.transform([top.x, top.y, top.z, 1.0]);
            if p[3] > 0.1 {
                let (x, y) = (p[0] / p[3], p[1] / p[3]);
                let at = Pos2::new(
                    rect.left() + (x * 0.5 + 0.5) * rect.width(),
                    rect.top() + (0.5 - y * 0.5) * rect.height(),
                );
                let name = if n == 0 {
                    isl.title.clone()
                } else {
                    display_name(&node.name, 48)
                };
                let galley = painter.layout_no_wrap(name, egui::FontId::monospace(13.0), t.strong);
                let r = Rect::from_center_size(
                    at - vec2(0.0, galley.size().y * 0.5 + 4.0),
                    galley.size() + vec2(8.0, 4.0),
                );
                painter.rect_filled(r, 2.0, Color32::from_rgba_unmultiplied(0, 20, 8, 210));
                painter.rect_stroke(r, 2.0, (1.0, t.accent), egui::StrokeKind::Outside);
                painter.galley(r.min + vec2(4.0, 2.0), galley, t.strong);
                taken.push(r);
            }
        }
        let mut buildings = 0;
        for c in cands.iter().take(300) {
            if taken.len() >= 90 {
                break;
            }
            if c.kind == LabelKind::Building {
                if buildings >= 30 {
                    continue;
                }
                buildings += 1;
            }
            // (behind a building: not shown - the islands' titles always are)
            if c.kind != LabelKind::Island && city::occluded(&cities, eye, c.at) {
                continue;
            }
            let p = vp.transform([c.at.x, c.at.y, c.at.z, 1.0]);
            if p[3] <= 0.1 {
                continue;
            }
            let (x, y) = (p[0] / p[3], p[1] / p[3]);
            if !(-1.1..=1.1).contains(&x) || !(-1.1..=1.1).contains(&y) {
                continue;
            }
            let at = Pos2::new(
                rect.left() + (x * 0.5 + 0.5) * rect.width(),
                rect.top() + (0.5 - y * 0.5) * rect.height(),
            );
            let Some(island) = self.island_of_city(c.island) else {
                continue;
            };
            let isl = &self.islands[island];
            if isl.stale && c.kind != LabelKind::Island {
                continue;
            }
            // (over an island, its title would float in the sky: left out)
            if c.kind == LabelKind::Island
                && isl.city.as_ref().is_some_and(|ci| {
                    let b = ci.aabb();
                    eye.x > b.min.x
                        && eye.x < b.max.x
                        && eye.z > b.min.z
                        && eye.z < b.max.z
                        && eye.y < b.max.y * 3.0 + 200.0
                })
            {
                continue;
            }
            let (text, size, color) = match c.kind {
                LabelKind::Island => {
                    let mut s = isl.title.clone();
                    if isl.from_cache.is_some() && isl.status.error.is_some() {
                        s.push_str("  · cached (cannot be read now)");
                    } else if isl.from_cache.is_some() {
                        s.push_str("  · cached, walking");
                    } else if !isl.status.done && isl.hw.is_none() {
                        s.push_str(&format!("  · {} …", isl.status.entries));
                    }
                    (s, 15.0, t.strong)
                }
                LabelKind::District => {
                    let Some(n) = isl.tree.get(c.node) else {
                        continue;
                    };
                    let slash = if isl.hw.is_some() { "" } else { "/" };
                    (
                        format!("{}{slash}", display_name(&n.name, 32)),
                        12.5,
                        t.label,
                    )
                }
                LabelKind::Building => {
                    let Some(n) = isl.tree.get(c.node) else {
                        continue;
                    };
                    (display_name(&n.name, 28), 11.0, t.label_dim)
                }
            };
            let font = if c.kind == LabelKind::Island {
                egui::FontId::proportional(size)
            } else {
                egui::FontId::monospace(size)
            };
            let galley = painter.layout_no_wrap(text, font, color);
            let r = Rect::from_center_size(
                at - vec2(0.0, galley.size().y * 0.5 + 2.0),
                galley.size() + vec2(6.0, 2.0),
            );
            if taken.iter().any(|o| o.intersects(r)) {
                continue;
            }
            taken.push(r);
            if c.kind == LabelKind::Island {
                painter.rect_filled(
                    r.expand(3.0),
                    2.0,
                    Color32::from_rgba_unmultiplied(0, 12, 4, 190),
                );
                painter.rect_stroke(
                    r.expand(3.0),
                    2.0,
                    (1.0, t.border),
                    egui::StrokeKind::Outside,
                );
            } else {
                // (a dark shadow: readable over lit towers)
                painter.galley(r.min + vec2(4.0, 2.0), galley.clone(), Color32::BLACK);
            }
            painter.galley(r.min + vec2(3.0, 1.0), galley, color);
        }
    }

    fn top_bar(&mut self, ui: &mut Ui) {
        let t = self.theme;
        ui.horizontal(|ui| {
            ui.label(RichText::new("axross files matrix").monospace().strong().color(t.accent));
            ui.separator();
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.search_text)
                    .hint_text("search names or *.glob  ( / )")
                    .desired_width(240.0),
            );
            if std::mem::take(&mut self.focus_search) {
                search.request_focus();
            }
            if search.changed() {
                let q = self.search_text.clone();
                self.run_search(&q);
            }
            if search.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                self.next_hit(false);
            }
            if let Some(s) = &self.search {
                let state = if s.done { "" } else { " …" };
                ui.label(RichText::new(format!("{} hits{state}", s.hits.len())).color(t.muted));
                if ui.button("next").on_hover_text("Fly to the next hit (N; Shift+N teleports)").clicked() {
                    self.next_hit(false);
                }
            }
            ui.separator();
            let fly = self.camera.mode == Mode::Fly;
            if ui.selectable_label(fly, "Fly").on_hover_text("Free flight: WASD, Q/E, mouse look with the right button or M (F switches)").clicked() {
                self.camera.set_mode(Mode::Fly);
            }
            if ui.selectable_label(!fly, "Orbit").on_hover_text("Orbit and map: drag to turn, Shift or the middle button to pan, wheel to zoom, click to fly there").clicked() {
                self.camera.set_mode(Mode::Orbit);
            }
            if ui.button("Overview").on_hover_text("The whole world (Home)").clicked() {
                self.overview(true);
            }
            if ui.button("Teleport").on_hover_text("Islands, districts, spots and hits (T)").clicked() {
                self.teleport_open = !self.teleport_open;
            }
            ui.separator();
            if ui.button("Sources").clicked() {
                self.sources_open = !self.sources_open;
            }
            if ui.button("Settings").clicked() {
                self.settings_open = !self.settings_open;
            }
            if ui.button("?").on_hover_text("Keys and mouse (H)").clicked() {
                self.help_open = !self.help_open;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // the write mode: visibly off (file operations come later)
                let badge = RichText::new(" READ-ONLY ").monospace().strong().color(t.ground).background_color(t.accent);
                ui.label(badge).on_hover_text("Nothing is changed on any disk. File operations (copy, move, rename, delete) come in a later release.");
                let mut write = false;
                ui.add_enabled(false, egui::Checkbox::new(&mut write, "write mode")).on_disabled_hover_text("Not available yet: this release only reads.");
            });
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let t = self.theme;
        ui.horizontal(|ui| {
            let walking: Vec<&Island> = self.islands.iter().filter(|i| !i.status.done && i.hw.is_none()).collect();
            let nodes: usize = self.islands.iter().map(|i| i.tree.len()).sum();
            ui.label(RichText::new(format!("{} islands · {} entries", self.islands.len(), nodes)).color(t.muted));
            if !walking.is_empty() {
                let entries: u64 = walking.iter().map(|i| i.status.entries).sum();
                ui.label(RichText::new(format!("· walking {} ({} found)", walking.len(), entries)).color(t.text));
            }
            match &self.mcp {
                McpState::Connected { label, .. } => {
                    ui.label(RichText::new(format!("· axross: {label}")).color(t.muted));
                }
                McpState::Looking => {
                    ui.label(RichText::new("· looking for axross…").color(t.muted));
                }
                _ => {}
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ms = FrameTimes::mean_ms(&self.times.frames);
                let fps = if ms > 0.0 { 1000.0 / ms } else { 0.0 };
                ui.label(RichText::new(format!("{fps:.0} fps · {} boxes", self.last_frame_instances)).monospace().color(t.muted));
                let stats = self.render_stats.as_ref().and_then(|s| s.lock().ok().map(|s| s.clone()));
                if let Some(s) = &stats {
                    ui.label(RichText::new(&s.adapter).color(t.muted));
                    if s.software {
                        ui.label(RichText::new("software renderer: slow").color(t.warn)).on_hover_text(
                            "No GPU was found (or none that can draw this window): a software renderer draws the city, a few frames a second. \
                             With a GPU, see the README's GPU section (WGPU_BACKEND, WGPU_ADAPTER_NAME, Settings > GPU).",
                        );
                    }
                }
                if let Some(s) = stats
                    && s.veil
                {
                    ui.label(RichText::new("flash guard veiling").color(t.warn))
                        .on_hover_text("The picture was about to flash faster than three times a second here; it is softened.");
                }
                ui.label(RichText::new(match self.camera.mode {
                    Mode::Fly => format!("fly {:.0} m/s", self.camera.speed),
                    Mode::Orbit => "orbit".into(),
                }).color(t.muted));
                if let Some((text, at)) = &self.toast
                    && at.elapsed() < Duration::from_secs(6)
                {
                    ui.label(RichText::new(text).color(t.strong));
                }
            });
        });
    }

    fn inspect_panel(&mut self, ui: &mut Ui) {
        let Some((i, node)) = self.selected else {
            return;
        };
        let t = self.theme;
        let Some(isl) = self.islands.get(i) else {
            return;
        };
        let Some(n) = isl.tree.get(node) else { return };
        let title = if node == 0 {
            isl.title.clone()
        } else {
            display_name(&n.name, 200)
        };
        ui.label(
            RichText::new(title)
                .monospace()
                .strong()
                .color(t.strong)
                .size(14.0),
        );
        ui.add_space(4.0);
        let mut rows: Vec<(String, String)> = Vec::new();
        if let Some(hwi) = &isl.hw {
            if let Some(info) = hwi.info.get(&node) {
                rows.extend(info.iter().cloned());
            }
        } else {
            rows.push(("island".into(), isl.title.clone()));
            match n.kind {
                Kind::Dir => {
                    rows.push(("kind".into(), "folder".into()));
                    rows.push((
                        "holds".into(),
                        format!("{} in {} files", hw::human_bytes(n.total), n.count),
                    ));
                    rows.push(("entries".into(), n.children.len().to_string()));
                    rows.push(("read".into(), n.listing.label().into()));
                    rows.push(("newest change".into(), preview::time_string(n.newest)));
                }
                k => {
                    rows.push(("type".into(), n.ftype.label().into()));
                    if k == Kind::Link {
                        rows.push(("kind".into(), "symbolic link (not followed)".into()));
                    }
                    rows.push((
                        "size".into(),
                        format!("{} ({} bytes)", hw::human_bytes(n.size), n.size),
                    ));
                }
            }
        }
        let details = self
            .preview
            .as_ref()
            .filter(|p| p.island == isl.id && p.node == node)
            .and_then(|p| p.preview.as_ref())
            .map(|p| p.details.rows.clone())
            .unwrap_or_default();
        for (k, v) in details {
            if !rows.iter().any(|(a, _)| *a == k) {
                rows.push((k, v));
            }
        }
        egui::Grid::new("details")
            .num_columns(2)
            .spacing(vec2(10.0, 3.0))
            .show(ui, |ui| {
                for (k, v) in &rows {
                    ui.label(RichText::new(k).color(t.muted));
                    ui.add(egui::Label::new(RichText::new(v).monospace().color(t.text)).wrap());
                    ui.end_row();
                }
            });
        ui.add_space(6.0);
        let is_hw = isl.hw.is_some();
        let is_local = matches!(isl.source, IslandSource::Local { .. });
        let path_text = match &isl.source {
            IslandSource::Local { root, .. } => isl
                .tree
                .local_path(root, node)
                .to_string_lossy()
                .into_owned(),
            IslandSource::Mcp { root, .. } => isl.tree.remote_path(root, node),
            _ => String::new(),
        };
        let has_axross = !is_hw && crate::open::axross().is_some();
        let mut fly = false;
        let mut open_ax = false;
        let mut open_sys = false;
        ui.horizontal_wrapped(|ui| {
            fly = ui.button("Fly here").clicked();
            if !is_hw {
                open_ax = has_axross && ui.button("Open in axross").on_hover_text("O").clicked();
                open_sys = is_local
                    && ui
                        .button("Open")
                        .on_hover_text("With the system's default program (Shift+O)")
                        .clicked();
                if ui.button("Copy path").clicked() {
                    ui.ctx().copy_text(path_text.clone());
                }
            }
        });
        if fly {
            self.fly_to(i, node);
        }
        if open_ax || open_sys {
            self.open_selected(open_ax, ui.ctx());
        }
        ui.separator();
        // the preview
        let Some(pv) = self.preview.as_mut() else {
            return;
        };
        if pv.loading {
            ui.label(RichText::new("reading…").color(t.muted));
            return;
        }
        let Some(p) = pv.preview.as_ref() else { return };
        match &p.content {
            Content::Text(text) => {
                if p.truncated {
                    ui.label(RichText::new("the first 64 KB").color(t.muted));
                }
                egui::ScrollArea::both().id_salt("preview").show(ui, |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(text).monospace().size(11.5).color(t.text))
                            .extend(),
                    );
                });
            }
            Content::Hex(text) => {
                ui.label(RichText::new("binary: the first bytes").color(t.muted));
                egui::ScrollArea::both().id_salt("preview").show(ui, |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(text).monospace().size(11.0).color(t.text))
                            .extend(),
                    );
                });
            }
            Content::Image(img, size) => {
                let tex = pv.texture.get_or_insert_with(|| {
                    ui.ctx().load_texture(
                        "afm-preview",
                        (**img).clone(),
                        egui::TextureOptions::LINEAR,
                    )
                });
                ui.label(RichText::new(format!("{} × {} pixels", size[0], size[1])).color(t.muted));
                let avail = ui.available_width();
                let s = tex.size_vec2();
                let k = (avail / s.x).min(1.0);
                ui.image((tex.id(), s * k));
            }
            Content::Empty => {}
            Content::Error(e) => {
                ui.label(RichText::new(format!("could not be read: {e}")).color(t.bad));
            }
        }
    }

    fn teleport_window(&mut self, ctx: &egui::Context) {
        if !self.teleport_open {
            return;
        }
        let mut open = true;
        let mut go: Option<(usize, NodeId)> = None;
        let mut spot: Option<usize> = None;
        let mut save: Option<usize> = None;
        let t = self.theme;
        egui::Window::new("Teleport")
            .open(&mut open)
            .default_width(320.0)
            .default_pos(self.view_rect.left_top() + vec2(16.0, 16.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(480.0)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Islands").strong());
                        for (i, isl) in self.islands.iter().enumerate() {
                            if isl.city.is_none() {
                                continue;
                            }
                            ui.horizontal(|ui| {
                                if ui.button("go").clicked() {
                                    go = Some((i, 0));
                                }
                                ui.label(&isl.title);
                            });
                            // its largest districts
                            let root = isl.tree.root();
                            let mut kids: Vec<NodeId> = root
                                .children
                                .iter()
                                .copied()
                                .filter(|c| isl.tree.nodes[*c as usize].kind == Kind::Dir)
                                .collect();
                            kids.sort_by_key(|c| {
                                std::cmp::Reverse(isl.tree.nodes[*c as usize].total)
                            });
                            for c in kids.into_iter().take(5) {
                                ui.horizontal(|ui| {
                                    ui.add_space(18.0);
                                    if ui.small_button("go").clicked() {
                                        go = Some((i, c));
                                    }
                                    let n = &isl.tree.nodes[c as usize];
                                    let text = if isl.hw.is_some() {
                                        display_name(&n.name, 40)
                                    } else {
                                        format!(
                                            "{}/  {}",
                                            display_name(&n.name, 40),
                                            hw::human_bytes(n.total)
                                        )
                                    };
                                    ui.label(RichText::new(text).color(t.muted));
                                });
                            }
                        }
                        ui.separator();
                        ui.label(RichText::new("Spots (1-9 jump, Ctrl+1-9 save)").strong());
                        for k in 0..9 {
                            ui.horizontal(|ui| {
                                let s = self.settings.bookmarks.get(k).cloned().flatten();
                                if ui
                                    .add_enabled(
                                        s.is_some(),
                                        egui::Button::new(format!("{}", k + 1)).small(),
                                    )
                                    .clicked()
                                {
                                    spot = Some(k + 1);
                                }
                                if ui.small_button("save here").clicked() {
                                    save = Some(k + 1);
                                }
                                ui.label(
                                    RichText::new(s.map_or("-".into(), |s| s.label)).color(t.muted),
                                );
                            });
                        }
                        if let Some(s) = &self.search
                            && !s.hits.is_empty()
                        {
                            ui.separator();
                            ui.label(
                                RichText::new(format!("Search hits ({})", s.hits.len())).strong(),
                            );
                            for &(i, n) in s.hits.iter().take(40) {
                                let name = self
                                    .islands
                                    .get(i)
                                    .and_then(|isl| isl.tree.get(n))
                                    .map(|x| display_name(&x.name, 50))
                                    .unwrap_or_default();
                                ui.horizontal(|ui| {
                                    if ui.small_button("go").clicked() {
                                        go = Some((i, n));
                                    }
                                    ui.label(RichText::new(name).monospace());
                                });
                            }
                        }
                    });
            });
        self.teleport_open = open;
        if let Some((i, n)) = go {
            self.teleport_to(i, n);
        }
        if let Some(k) = spot {
            self.jump_spot(k);
        }
        if let Some(k) = save {
            self.save_spot(k);
        }
    }

    fn sources_window(&mut self, ctx: &egui::Context) {
        if !self.sources_open {
            return;
        }
        let mut open = true;
        let t = self.theme;
        let mut open_profile: Option<(Arc<Client>, String)> = None;
        let mut rescan: Option<usize> = None;
        let mut reconnect = false;
        egui::Window::new("Sources").open(&mut open).default_width(420.0).show(ctx, |ui| {
            for (i, isl) in self.islands.iter().enumerate() {
                ui.horizontal(|ui| {
                    let state = if isl.hw.is_some() {
                        "the machine".to_string()
                    } else if let Some(e) = &isl.status.error {
                        match isl.from_cache {
                            Some(t) => format!("could not be read ({e}); the city from {}", preview::time_string(t)),
                            None => format!("could not be read: {e}"),
                        }
                    } else if isl.from_cache.is_some() {
                        format!("cached; walking ({} found)", isl.status.entries)
                    } else if isl.status.done {
                        format!("{} entries in {:.1} s", isl.tree.len(), isl.status.seconds)
                    } else {
                        format!("walking: {} found", isl.status.entries)
                    };
                    ui.label(RichText::new(&isl.title).monospace());
                    ui.label(RichText::new(state).color(t.muted));
                    if isl.hw.is_none() && ui.small_button("walk again").clicked() {
                        rescan = Some(i);
                    }
                });
            }
            ui.separator();
            ui.label(RichText::new("axross").strong());
            match &self.mcp {
                McpState::Off => {
                    ui.label("Off (Settings).");
                }
                McpState::Looking => {
                    ui.label("Looking for axross-headless-mcp…");
                }
                McpState::NotFound => {
                    ui.label("No axross found: local disks and mounts only.");
                    ui.label(RichText::new("Install axross-headless-mcp (pip or uvx) or axross for remote backends:").color(t.muted));
                    ui.hyperlink_to("github.com/c0decave/axross", "https://github.com/c0decave/axross");
                }
                McpState::Failed(e) => {
                    ui.label(RichText::new(format!("axross could not be started: {e}")).color(t.warn));
                    if ui.button("try again").clicked() {
                        reconnect = true;
                    }
                }
                McpState::Connected { label, client, profiles } => {
                    ui.label(format!("{label} ({} {}), {} tools", client.server, client.version, client.tools.len()));
                    if !profiles.is_empty() {
                        ui.label(RichText::new("Saved remote profiles (opening one connects to it):").color(t.muted));
                        for p in profiles {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(p).monospace());
                                if ui.small_button("open").clicked() {
                                    open_profile = Some((client.clone(), p.clone()));
                                }
                            });
                        }
                    }
                }
            }
        });
        self.sources_open = open;
        if reconnect {
            self.connect_mcp();
        }
        if let Some(i) = rescan {
            let all = mounts::system_mounts();
            let skip = match &self.islands[i].source {
                IslandSource::Local { root, .. } if self.opts.paths.is_empty() => {
                    mounts::nested_below(&all, root)
                }
                _ => Vec::new(),
            };
            self.start_walk(i, &skip);
        }
        if let Some((client, name)) = open_profile {
            if !self.settings.mcp.open_profiles.contains(&name) {
                self.settings.mcp.open_profiles.push(name.clone());
                let _ = settings::save(&self.settings_path, &self.settings);
            }
            let tx = self.bg_tx.clone();
            let wake = self.wake.clone();
            let _ = std::thread::Builder::new()
                .name("afm-mcp-open".into())
                .spawn(move || {
                    let r = client
                        .open_profile(&name, Duration::from_secs(90))
                        .and_then(|_| client.backends(Duration::from_secs(30)));
                    let _ = tx.send(Bg::Backends(client, r));
                    wake();
                });
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let mut open = true;
        let t = self.theme;
        let before = self.settings.clone();
        let mut mode =
            UpdateMode::from_key(&self.settings.update_mode).unwrap_or(UpdateMode::Notify);
        egui::Window::new("Settings")
            .open(&mut open)
            .default_width(380.0)
            .show(ctx, |ui| {
                let s = &mut self.settings;
                ui.label(RichText::new("Access").strong());
                ui.add_enabled(
                    false,
                    egui::Checkbox::new(
                        &mut true.clone(),
                        "Read-only (nothing on any disk is changed)",
                    ),
                );
                ui.label(
                    RichText::new(
                        "File operations come in a later release; until then the city only reads.",
                    )
                    .color(t.muted),
                );
                ui.checkbox(
                    &mut s.ask_mode_before_flight,
                    "Ask \"read-only or write?\" before the flight",
                );
                ui.separator();
                ui.label(RichText::new("Islands").strong());
                ui.checkbox(
                    &mut s.show_pseudo_mounts,
                    "Show pseudo file systems (proc, sysfs, tmpfs, …) - at the next start",
                );
                ui.checkbox(
                    &mut s.hardware_island,
                    "The machine's own island - at the next start",
                );
                ui.checkbox(&mut s.mcp.enabled, "Look for axross's MCP server");
                ui.checkbox(
                    &mut s.mcp.allow_uvx_download,
                    "Let uvx fetch axross-headless-mcp when it does not have it",
                );
                ui.horizontal(|ui| {
                    ui.label("Server command:");
                    let mut cmd = s.mcp.command.join(" ");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut cmd)
                                .hint_text("found by itself")
                                .desired_width(200.0),
                        )
                        .changed()
                    {
                        // (split on spaces: each part one argument, no shell)
                        s.mcp.command = cmd.split_whitespace().map(String::from).collect();
                    }
                });
                ui.separator();
                ui.label(RichText::new("Look").strong());
                ui.horizontal(|ui| {
                    ui.label("Theme:");
                    egui::ComboBox::from_id_salt("theme")
                        .selected_text(theme::find(&s.theme).name)
                        .show_ui(ui, |ui| {
                            for th in &theme::THEMES {
                                ui.selectable_value(&mut s.theme, th.key.to_string(), th.name);
                            }
                        });
                });
                ui.checkbox(&mut s.labels, "Labels");
                ui.checkbox(&mut s.rain, "Digital rain");
                ui.checkbox(&mut s.bloom, "Bloom");
                ui.checkbox(&mut s.scanlines, "Scanlines");
                ui.checkbox(
                    &mut s.reduce_motion,
                    "Reduce motion (no warp, still rain, slower flights)",
                );
                ui.horizontal(|ui| {
                    ui.label("GPU:");
                    let current = crate::gpu::Preference::from_key(&s.gpu);
                    egui::ComboBox::from_id_salt("gpu")
                        .selected_text(current.label())
                        .show_ui(ui, |ui| {
                            for p in [
                                crate::gpu::Preference::Auto,
                                crate::gpu::Preference::HighPerformance,
                                crate::gpu::Preference::LowPower,
                            ] {
                                ui.selectable_value(&mut s.gpu, p.key().to_string(), p.label());
                            }
                        });
                });
                ui.label(RichText::new("(the GPU is chosen at the start)").color(t.muted));
                ui.separator();
                ui.label(RichText::new("Flight").strong());
                ui.add(egui::Slider::new(&mut s.mouse_sensitivity, 0.1..=5.0).text("mouse"));
                ui.checkbox(&mut s.invert_y, "Invert the mouse's up and down");
                ui.separator();
                ui.label(RichText::new("Updates").strong());
                for m in [UpdateMode::Off, UpdateMode::Notify, UpdateMode::Install] {
                    ui.radio_value(&mut mode, m, m.label());
                }
                if ui.button("Check for updates…").clicked() {
                    self.updater.open = true;
                }
            });
        self.settings.update_mode = mode.key().to_string();
        self.settings_open = open;
        if self.settings != before {
            self.camera.sensitivity = self.settings.mouse_sensitivity;
            self.camera.invert_y = self.settings.invert_y;
            self.theme = theme::find(&self.settings.theme);
            self.world_version += 1;
            let _ = settings::save(&self.settings_path, &self.settings);
        }
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        if !self.help_open {
            return;
        }
        let mut open = true;
        let adapter = self
            .render_stats
            .as_ref()
            .and_then(|s| s.lock().ok().map(|s| (s.adapter.clone(), s.software)));
        egui::Window::new("Keys and mouse")
            .open(&mut open)
            .default_width(420.0)
            .show(ctx, |ui| {
                egui::Grid::new("help").num_columns(2).show(ui, |ui| {
                    if let Some((a, soft)) = &adapter {
                        ui.label(RichText::new("drawn by").color(self.theme.muted));
                        ui.label(if *soft {
                            format!("{a} (a software renderer)")
                        } else {
                            a.clone()
                        });
                        ui.end_row();
                    }
                    for (k, v) in HELP {
                        ui.label(RichText::new(*k).monospace().color(self.theme.accent));
                        ui.label(*v);
                        ui.end_row();
                    }
                });
            });
        self.help_open = open;
    }

    fn ask_mode_window(&mut self, ctx: &egui::Context) {
        if !self.ask_mode_open {
            return;
        }
        egui::Modal::new(egui::Id::unique("afm-ask-mode")).show(ctx, |ui| {
            ui.label(RichText::new("How do you want to fly?").strong().size(16.0));
            ui.add_space(6.0);
            if ui.button("Read-only").clicked() {
                self.ask_mode_open = false;
            }
            ui.add_enabled(false, egui::Button::new("Read and write"))
                .on_disabled_hover_text(
                    "Not available yet: file operations come in a later release.",
                );
            ui.label(
                RichText::new("(This question can be turned off in the settings.)")
                    .color(self.theme.muted),
            );
        });
    }

    /// Everything a frame does but the drawing of the panels (tests call
    /// it to let the walks and layouts come in).
    pub fn settle(&mut self, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            self.pump(Duration::from_millis(50));
            let walking = self
                .islands
                .iter()
                .any(|i| !i.status.done && i.hw.is_none());
            let laying = self.islands.iter().any(|i| i.dirty || i.layout_pending);
            let cached = self.islands.iter().any(|i| i.fresh.is_some());
            if !walking && !laying && !cached && self.islands.iter().all(|i| i.city.is_some()) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    /// The search runs to its end (tests).
    pub fn finish_search(&mut self) {
        while self.search.as_ref().is_some_and(|s| !s.done) {
            self.step_search(Duration::from_millis(50));
        }
    }

    /// Waits for the chosen file's preview (tests).
    pub fn wait_preview(&mut self, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            self.pump(Duration::from_millis(5));
            if self.preview.as_ref().is_some_and(|p| !p.loading) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }
}

pub const HELP: &[(&str, &str)] = &[
    ("W A S D", "fly forward, left, back, right (arrows too)"),
    ("Q / E", "down / up"),
    ("Shift", "five times as fast"),
    ("wheel", "flight speed; in orbit: zoom"),
    ("right drag", "look around (fly) / turn (orbit)"),
    ("M", "capture the mouse to look (Esc lets go)"),
    ("F", "free flight / orbit and map"),
    ("drag", "orbit: turn; Shift+drag or middle: pan"),
    (
        "click",
        "choose (orbit: and fly there); double-click: fly there",
    ),
    ("/ or Ctrl+F", "search (text, or a glob like *.iso)"),
    ("N / Enter", "fly to the next hit (Shift: teleport)"),
    ("T", "teleport: islands, districts, spots, hits"),
    ("1 - 9", "jump to a saved spot"),
    ("Ctrl+1 - 9", "save this spot"),
    ("Home", "overview"),
    ("O / Shift+O", "open in axross / with the default program"),
    ("Esc", "let the mouse go, then clear the choice"),
    ("H / F1", "this help"),
];

fn invert(m: &crate::math::Mat4) -> [[f32; 4]; 4] {
    // Gauss-Jordan on the 4 x 4 (column-major in, column-major out)
    let mut a = [[0f64; 8]; 4];
    for (r, row) in a.iter_mut().enumerate() {
        for c in 0..4 {
            row[c] = m.0[c][r] as f64;
        }
        row[4 + r] = 1.0;
    }
    for col in 0..4 {
        let pivot = (col..4)
            .max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))
            .unwrap_or(col);
        a.swap(col, pivot);
        let p = a[col][col];
        if p.abs() < 1e-30 {
            return crate::math::Mat4::IDENTITY.0;
        }
        for v in a[col].iter_mut() {
            *v /= p;
        }
        for r in 0..4 {
            if r != col {
                let f = a[r][col];
                for c in 0..8 {
                    a[r][c] -= f * a[col][c];
                }
            }
        }
    }
    let mut out = [[0f32; 4]; 4];
    for (r, row) in a.iter().enumerate() {
        for c in 0..4 {
            out[c][r] = row[4 + c] as f32;
        }
    }
    out
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let t0 = Instant::now();
        if let Some(r) = ui.ctx().input(|i| i.viewport().inner_rect) {
            self.window_size = Some([r.width(), r.height()]);
        }
        let ctx = ui.ctx().clone();
        self.pump(Duration::from_millis(6));
        self.step_search(Duration::from_millis(3));
        self.keys(&ctx);
        let mode = UpdateMode::from_key(&self.settings.update_mode).unwrap_or(UpdateMode::Notify);
        if self.opts.updates {
            self.updater.background(&ctx, mode);
            self.updater.tick(&ctx, mode);
        }
        let t = self.theme;
        egui::Panel::top("top")
            .frame(
                egui::Frame::new()
                    .fill(t.ground)
                    .inner_margin(egui::Margin::symmetric(8, 5)),
            )
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(t.ground)
                    .inner_margin(egui::Margin::symmetric(8, 3)),
            )
            .show(ui, |ui| self.status_bar(ui));
        if self.selected.is_some() {
            egui::Panel::right("inspect")
                .default_size(340.0)
                .frame(
                    egui::Frame::new()
                        .fill(t.surface)
                        .inner_margin(egui::Margin::same(10)),
                )
                .show(ui, |ui| {
                    ui.set_min_width(260.0);
                    self.inspect_panel(ui);
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::BLACK))
            .show(ui, |ui| self.view(ui));
        self.teleport_window(&ctx);
        self.sources_window(&ctx);
        self.settings_window(&ctx);
        self.help_window(&ctx);
        self.ask_mode_window(&ctx);
        let mut mode = mode;
        if self.updater.window(&ctx, &mut mode) {
            self.settings.update_mode = mode.key().to_string();
            let _ = settings::save(&self.settings_path, &self.settings);
        }
        // the adaptive level of detail: fewer boxes when frames are slow
        let work = t0.elapsed().as_secs_f32();
        self.times.push(work);
        if !self.opts.fixed_lod && self.times.frames.len() >= 30 {
            let ms = FrameTimes::mean_ms(&self.times.frames);
            if ms > 20.0 && self.last_frame_instances > 20_000 {
                self.lod.collapse = (self.lod.collapse * 1.04).min(0.2);
            } else if ms < 13.0 {
                self.lod.collapse = (self.lod.collapse * 0.99).max(0.02);
            }
        }
        if self.opts.frame_stats {
            let due = self
                .times
                .reported
                .is_none_or(|r| r.elapsed() >= Duration::from_secs(1));
            if due {
                self.times.reported = Some(Instant::now());
                eprintln!(
                    "frame {:.2} ms (p95 {:.2}), work {:.2} ms, {} boxes, lod {:.3}",
                    FrameTimes::mean_ms(&self.times.frames),
                    FrameTimes::p95_ms(&self.times.frames),
                    FrameTimes::mean_ms(&self.times.work),
                    self.last_frame_instances,
                    self.lod.collapse
                );
            }
            ctx.request_repaint();
        }
    }

    fn on_exit(&mut self) {
        self.save_all();
    }
}

#[cfg(unix)]
static ASKED_TO_END: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn asked_to_end(_: libc::c_int) {
    // (asked a second time - the window not closing: ended at once)
    if ASKED_TO_END.swap(true, std::sync::atomic::Ordering::SeqCst) {
        // SAFETY: _exit is async-signal-safe
        unsafe { libc::_exit(130) };
    }
}

/// SIGTERM and SIGINT (a logout, `kill`, Ctrl+C in the terminal it runs in)
/// close the window as its close button does: the walk caches and the
/// settings written, not the process cut off. (As axross-amp does; the
/// program's own start, not the tests'.)
#[cfg(unix)]
pub fn close_on_signals(ctx: &egui::Context) {
    let handler = asked_to_end as extern "C" fn(libc::c_int) as libc::sighandler_t;
    // SAFETY: the handler only stores to an atomic (or _exits)
    unsafe {
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGINT, handler);
    }
    let ctx = ctx.clone();
    let _ = std::thread::Builder::new()
        .name("afm-signals".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(100));
                if ASKED_TO_END.load(std::sync::atomic::Ordering::SeqCst) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    ctx.request_repaint();
                    // (asked again each second, should a frame not come)
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Mat4;

    #[test]
    fn inverts_a_view_projection() {
        let v = Mat4::look_to(
            vec3(1.0, 2.0, 3.0),
            vec3(0.3, -0.5, -1.0).normalized(),
            Vec3::Y,
        );
        let p = Mat4::perspective_reversed_infinite(1.0, 1.5, 0.1);
        let vp = p.mul(&v);
        let inv = Mat4(invert(&vp));
        let id = inv.mul(&vp);
        for (c, col) in id.0.iter().enumerate() {
            for (r, v) in col.iter().enumerate() {
                let want = if c == r { 1.0 } else { 0.0 };
                assert!((v - want).abs() < 1e-3, "{c} {r}: {v}");
            }
        }
    }
}
