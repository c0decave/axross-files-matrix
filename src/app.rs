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
use crate::layout::Placed;
use crate::math::{Aabb, Frustum, Vec3, vec3};
use crate::mcp::{self, Backend, Client};
use crate::mounts::{self, MountKind};
use crate::open::{self, Axross, Launch};
use crate::preview::{self, Content, Preview, Where};
use crate::render::{self, FrameData, Globals, Stats};
use crate::search::Search;
use crate::settings::{self, Settings, Spot};
use crate::systems::SysState;
use crate::theme::{self, Theme};
use crate::tree::{Kind, NodeId, display_name};
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
    /// What the start asks of the system (tests: one that hangs).
    pub probe: Option<Arc<dyn world::Probe>>,
    /// Launches are written here instead of started (tests).
    pub launch_log: Option<Arc<Mutex<Vec<Launch>>>>,
    /// An Open's check takes this long (tests: a disk that does not answer).
    pub open_check_delay: Option<Duration>,
    /// This axross program instead of the one on the PATH (tests).
    pub axross: Option<PathBuf>,
    /// This program opens files and folders instead of the system's
    /// (tests).
    pub opener: Option<PathBuf>,
    /// A connected MCP client instead of looking for a server (tests).
    pub mcp_client: Option<Arc<Client>>,
    /// How long a system's connect may take (tests: short).
    pub connect_timeout: Option<Duration>,
}

impl std::fmt::Debug for dyn world::Probe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Probe")
    }
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
            probe: None,
            launch_log: None,
            open_check_delay: None,
            axross: None,
            opener: None,
            mcp_client: None,
            connect_timeout: None,
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
        /// The command line it was started with (said in Sources).
        command: String,
        client: Arc<Client>,
        /// The saved connections and protocols, or why the server cannot
        /// list them.
        systems: Result<mcp::Systems, String>,
    },
}

/// What the search for axross's server found: the client, how it was
/// started, its backends and saved profiles.
type McpFound = (
    Arc<Client>,
    String,
    String,
    Vec<Backend>,
    Result<mcp::Systems, String>,
);

enum Bg {
    Mcp(Result<McpFound, Option<String>>),
    Preview(u64, NodeId, Box<Preview>),
    Opened(Result<String, String>),
    Startup(Box<world::Startup>),
    Axross(Axross, Option<settings::AxrossProbe>),
    OpenChecked(Box<OpenCheck>),
    /// A folder to add, made whole and looked at (or why not).
    SourceChecked(Result<PathBuf, String>),
    /// The walk cache's size, measured again (Settings shows it).
    CacheUsage((u64, usize)),
    /// The file looked at again right before it is opened: still the one
    /// that was checked (or asked about)?
    OpenRechecked(PathBuf, bool),
    CachePruned(cache::Pruned, (u64, usize)),
    /// A system's connect came back (its attempt: a cancelled one's late
    /// answer is dropped).
    SystemOpened {
        name: String,
        attempt: u64,
        result: Result<mcp::Opened, String>,
    },
    /// The systems listed again.
    Systems(Result<mcp::Systems, String>),
    /// Fonts for the scripts egui's own do not draw (found on a thread).
    Fonts(Vec<(String, Vec<u8>)>),
}

/// A row of the Teleport menu's systems.
#[derive(Clone, Debug)]
pub struct SysRow {
    pub name: String,
    sign: &'static str,
    family: &'static str,
    color: Color32,
    where_: String,
    pub state: SysState,
    label: String,
    has_island: bool,
}

/// The Teleport menu's cached rows.
#[derive(Default)]
struct TeleportCache {
    key: (u64, String),
    stamp: Option<Instant>,
    rows: std::rc::Rc<Vec<SysRow>>,
    hits: HitFilter,
}

/// The search hits the menu's filter takes, found a little each frame.
#[derive(Default)]
struct HitFilter {
    query: String,
    search: String,
    scanned: usize,
    matched: Vec<u32>,
}

/// An island's districts for the Teleport menu, largest first (at most
/// 108), made again when its tree changed - at most once a second while it
/// grows.
fn island_districts<'a>(
    isl: &Island,
    cache: &'a mut std::collections::HashMap<u64, (usize, Instant, Vec<NodeId>)>,
) -> &'a [NodeId] {
    let len = isl.tree.len();
    let stale = cache
        .get(&isl.id)
        .is_none_or(|(l, at, _)| *l != len && at.elapsed() > Duration::from_secs(1));
    if stale {
        let root = isl.tree.root();
        let mut kids: Vec<NodeId> = root
            .children
            .iter()
            .copied()
            .filter(|c| isl.tree.nodes[*c as usize].kind == Kind::Dir)
            .collect();
        kids.sort_by_key(|c| std::cmp::Reverse(isl.tree.nodes[*c as usize].total));
        kids.truncate(108);
        cache.insert(isl.id, (len, Instant::now(), kids));
    }
    &cache[&isl.id].2
}

/// A flight waiting for its target: an island to stand, a district of it
/// to be read (a spot inside a remote system).
#[derive(Clone, Debug)]
pub struct PendingFly {
    pub island: u64,
    /// The path from the island's root (raw names).
    pub path: Vec<Vec<u8>>,
    pub deadline: Instant,
    pub what: String,
}

/// Where the hover's tip (`size`) stands by the pointer `p`: below right,
/// else another corner - never over an island's title (its state and
/// reason stay readable), always inside the view.
pub fn hover_tip_at(p: Pos2, size: egui::Vec2, view: Rect, titles: &[Rect]) -> Pos2 {
    let tries = [
        vec2(14.0, 10.0),
        vec2(14.0, -10.0 - size.y),
        vec2(-14.0 - size.x, 10.0),
        vec2(-14.0 - size.x, -10.0 - size.y),
        vec2(14.0, 28.0),
        vec2(14.0, -28.0 - size.y),
    ];
    let inside = |at: Pos2| {
        let r = Rect::from_min_size(at - vec2(4.0, 2.0), size);
        let dx = (view.left() - r.left()).max(0.0) + (view.right() - r.right()).min(0.0);
        let dy = (view.top() - r.top()).max(0.0) + (view.bottom() - r.bottom()).min(0.0);
        at + vec2(dx, dy)
    };
    for d in tries {
        let at = inside(p + d);
        let r = Rect::from_min_size(at - vec2(4.0, 2.0), size);
        if !titles.iter().any(|t| t.intersects(r)) {
            return at;
        }
    }
    inside(p + tries[0])
}

/// An island's title as drawn: its state's reason on a line of its own
/// (wrapped inside the window, never cut off at its edge).
pub fn title_text(caption: &str) -> String {
    caption
        .replacen("  · failed: ", "\nfailed: ", 1)
        .replacen("  · cached (", "\ncached (", 1)
}

/// How fast the view moves, eased (0 still .. 1 fast): its turn (radians
/// a second) and its travel against how near things are (`clearance`:
/// the height over the roofs and the ground, or a few metres among walls -
/// what sweeps across the picture), from 0.25 to 1.5 a second; eased (a
/// quarter of a second up, half a second down); a cut is no motion.
pub fn view_motion(
    before: f32,
    last: Option<(Vec3, f32, f32)>,
    cam: &crate::camera::Camera,
    clearance: f32,
    dt: f32,
    cut: bool,
) -> f32 {
    let Some((pos, yaw, pitch)) = last else {
        return 0.0;
    };
    if cut || dt <= 0.0 {
        return before;
    }
    let turn = crate::camera::wrap_angle(cam.yaw - yaw).abs() + (cam.pitch - pitch).abs();
    let travel = (cam.pos - pos).length() / clearance.min(cam.pos.y).max(3.0);
    if travel > 2.0 {
        // (a jump - a teleport - is no motion either)
        return before;
    }
    let rate = (turn + travel) / dt;
    let now = crate::math::smoothstep((rate - 0.25) / 1.25);
    // (eased both ways - over a quarter of a second up, half a second
    // down: what the motion tones down changes slowly itself, never a
    // flicker of its own when a flight starts, stops or stutters)
    let tau = if now >= before { 0.25 } else { 0.5 };
    before + (now - before) * (1.0 - (-dt / tau).exp())
}

/// How often an ended server is started again by itself.
pub const SERVER_RESTARTS: u32 = 5;

/// How long before the `n`th start again (1 first): 2, 4, 8 ... 60 s.
pub fn server_backoff(n: u32) -> Duration {
    Duration::from_secs((1u64 << n.clamp(1, 6)).min(60))
}

/// The first `n` of a list, and how many more ("a, b, c and 7 more").
pub fn shortlist(items: &[String], n: usize) -> String {
    let mut s = items.iter().take(n).cloned().collect::<Vec<_>>().join(", ");
    if items.len() > n {
        s.push_str(&format!(" and {} more", items.len() - n));
    }
    s
}

/// `~` and `~/x` as the home folder and below it (else as given).
pub fn expand_home(text: &str, home: Option<&std::path::Path>) -> PathBuf {
    match (text, home) {
        ("~", Some(h)) => h.to_path_buf(),
        (t, Some(h)) if t.starts_with("~/") => h.join(&t[2..]),
        (t, _) => PathBuf::from(t),
    }
}

/// The axross this program works with best: it opens paths and saved
/// connections from the command line, and its MCP server lists every
/// protocol and frees a given-up connect's slot at once.
pub const AXROSS_VERSION: &str = "0.15.186";

/// "1 island", "2 islands", "0 islands".
pub fn count(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// How long an Open's check may take before the window says the disk
/// does not answer.
pub const OPEN_PATIENCE: Duration = Duration::from_millis(1500);

/// How the chosen thing is to be opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenHow {
    Axross,
    DefaultProgram,
    FileManager,
}

/// A path to open with the default program, and why it needs a question
/// first (None: it does not).
#[derive(Clone, Debug)]
pub struct OpenCheck {
    pub path: PathBuf,
    pub risk: Option<open::Risk>,
    /// What it was when checked.
    pub id: Option<open::Identity>,
}

/// The question before opening (shown as a dialog).
#[derive(Clone, Debug, PartialEq)]
pub enum OpenAsk {
    /// The default program would run or trust something.
    Risky {
        path: PathBuf,
        why: String,
        /// A type this program does not know: "do not ask again" may be
        /// ticked for it.
        unknown: Option<String>,
        /// What it was when asked about (opened only if it still is).
        id: Option<open::Identity>,
    },
    /// axross cannot open it: why, and the local path the default
    /// program could open instead (if there is one).
    NoAxross { why: String, path: Option<PathBuf> },
    /// A system needs a password or key passphrase axross has not stored
    /// (the matrix never asks for one).
    Unlock { name: String },
    /// axross's server holds as many systems open as it may: `name` could
    /// not be opened; `open` are the open ones, least recently visited
    /// first.
    TooMany { name: String, open: Vec<String> },
}

struct PreviewJob {
    island: u64,
    node: NodeId,
    at: Where,
    name: String,
    size: u64,
    is_dir: bool,
}

/// The preview worker: one preview at a time, and of the requests waiting
/// only the newest (choosing ten files fast reads one, not ten).
/// Previews read at once (each on a thread of its own); one that has not
/// answered for [`PREVIEW_ABANDON`] counts no more - a file on a dead mount
/// holds its thread, never the others' - up to [`PREVIEW_ALIVE`] threads
/// in all.
const PREVIEW_THREADS: usize = 3;
const PREVIEW_ABANDON: Duration = Duration::from_secs(5);
const PREVIEW_ALIVE: usize = 24;
/// A request that waited this long for a free thread gets a fresh one
/// (three previews stuck on a dead mount never hold up a healthy one for
/// [`PREVIEW_ABANDON`]).
const PREVIEW_PATIENCE: Duration = Duration::from_millis(300);

/// Whether the dispatcher may start a preview now: threads under way
/// `ages` ago, the request waiting `waited`.
pub fn preview_may_start(ages: &[Duration], waited: Duration) -> bool {
    if ages.len() >= PREVIEW_ALIVE {
        return false;
    }
    let busy = ages.iter().filter(|a| **a < PREVIEW_ABANDON).count();
    busy < PREVIEW_THREADS || waited >= PREVIEW_PATIENCE
}

/// The preview dispatcher: of the requests waiting only the newest is
/// read, on a thread of its own.
fn preview_worker(rx: Receiver<PreviewJob>, tx: Sender<Bg>, wake: walk::Wake) {
    // (the threads under way: when each started)
    let running: Arc<Mutex<Vec<(u64, Instant)>>> = Arc::new(Mutex::new(Vec::new()));
    let mut waiting: Option<PreviewJob> = None;
    let mut since = Instant::now();
    let mut next_id = 0u64;
    loop {
        let next = if waiting.is_some() {
            rx.recv_timeout(Duration::from_millis(50)).ok()
        } else {
            match rx.recv() {
                Ok(j) => Some(j),
                Err(_) => return,
            }
        };
        if waiting.is_none() {
            since = Instant::now();
        }
        if let Some(j) = next {
            waiting = Some(j);
        }
        while let Ok(newer) = rx.try_recv() {
            waiting = Some(newer);
        }
        if waiting.is_none() {
            continue;
        }
        {
            let r = running.lock().unwrap_or_else(|p| p.into_inner());
            let ages: Vec<Duration> = r.iter().map(|(_, at)| at.elapsed()).collect();
            if !preview_may_start(&ages, since.elapsed()) {
                continue;
            }
        }
        let Some(job) = waiting.take() else { continue };
        next_id += 1;
        let id = next_id;
        running
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((id, Instant::now()));
        let (tx, wake, running) = (tx.clone(), wake.clone(), running.clone());
        let started = std::thread::Builder::new()
            .name("afm-preview".into())
            .spawn(move || {
                let p = preview::load(&job.at, &job.name, job.size, job.is_dir);
                running
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .retain(|(k, _)| *k != id);
                let _ = tx.send(Bg::Preview(job.island, job.node, Box::new(p)));
                wake();
            });
        if started.is_err() {
            return;
        }
    }
}

struct PreviewView {
    island: u64,
    node: NodeId,
    loading: bool,
    preview: Option<Preview>,
    texture: Option<egui::TextureHandle>,
}

/// What a pick under the pointer was made for: the pointer, the camera,
/// the view's shape, and the world's version.
type PickKey = ([f32; 8], u64);

/// Where a frame's own work went (milliseconds; the last frame).
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameParts {
    pub pick_ms: f32,
    pub lod_ms: f32,
    pub labels_ms: f32,
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
    preview_jobs: Sender<PreviewJob>,
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
    /// The Teleport menu's filter takes the keyboard next frame (T).
    focus_filter: bool,
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
    /// The last frame's parts (milliseconds): picking under the pointer,
    /// the level of detail and the instances, the labels.
    pub parts: FrameParts,
    /// The last pick under the pointer and what it was made for.
    last_pick: Option<(PickKey, Option<Hit>)>,
    render_stats: Option<Arc<Mutex<Stats>>>,
    has_gpu: bool,
    updater: Updater,
    captured: bool,
    started: Instant,
    last_frame: Instant,
    last_teleport: f32,
    frames: u64,
    queued_teleport: Option<(Vec3, f32, f32)>,
    /// A teleport to an island's node waiting its turn.
    queued_target: Option<(usize, NodeId)>,
    placed_sides: Vec<f32>,
    view_rect: Rect,
    pub last_frame_instances: usize,
    /// The tests' flashing signal (0..1 over the picture; 0 in use).
    pub test_signal: f32,
    /// The window's size (kept for the next start).
    window_size: Option<[f32; 2]>,
    /// The start's thread has not answered yet.
    pub starting: bool,
    /// What is known of axross (the program).
    pub axross: Axross,
    /// The beams need placing again (an island moved).
    beams_dirty: bool,
    /// The camera jumped this frame (the veil's ground is cleared).
    cut: bool,
    /// Until the camera is moved by hand, it frames the biggest city (the
    /// first view): which island and at which size it framed.
    pub auto_frame: bool,
    framed: Option<(u64, f32, [f32; 2])>,
    /// The island kept framed (None: the biggest city).
    frame_on: Option<u64>,
    /// The 3D view's width / height (last frame).
    view_aspect: f32,
    /// How fast the view moves (0 still .. 1 fast; eased), and where the
    /// camera was the frame before.
    pub motion: f32,
    last_cam: Option<(Vec3, f32, f32)>,
    /// How long the movement keys have moved nothing, and whether that
    /// was said.
    stuck_for: f32,
    stuck_said: bool,
    /// When the wheel last changed the flight's speed (said for a moment).
    pub speed_said: Option<Instant>,
    /// Where the islands' titles stand on the screen (the hover's tip
    /// keeps off them).
    title_rects: Vec<Rect>,
    /// The open question, if one is shown.
    pub open_ask: Option<OpenAsk>,
    /// An Open's check under way: since when, and whether its slowness was
    /// said (a disk that does not answer).
    open_waiting: Option<(Instant, bool)>,
    /// Its "do not ask again for this type" box.
    pub trust_type: bool,
    /// The Sources window's "add a folder" field.
    pub add_source_text: String,
    /// The window has the keyboard (None: not known).
    focused: bool,
    /// Minimised or hidden.
    hidden: bool,
    /// The colour legend is shown.
    pub legend_open: bool,
    /// How much bigger the machine's island is drawn.
    pub machine_scale: f32,
    /// The Teleport menu's cached rows, and districts per island.
    tp_cache: TeleportCache,
    tp_districts: std::collections::HashMap<u64, (usize, Instant, Vec<NodeId>)>,
    /// Counts up whenever the systems are listed anew.
    systems_version: u64,
    /// The Teleport menu's filter (across systems, islands, spots, hits).
    pub teleport_filter: String,
    /// The systems being opened or opened, by profile name.
    pub live: std::collections::HashMap<String, crate::systems::Live>,
    /// The systems' list as last known from a server that ended (shown,
    /// marked as old, until it is back), how often it was started again,
    /// and when it is next.
    stale_systems: Option<mcp::Systems>,
    server_restarts: u32,
    server_restart_at: Option<Instant>,
    attempts: u64,
    /// A flight waiting for its target.
    pub pending_fly: Option<PendingFly>,
    /// The axross on the PATH opens its connection manager from the
    /// command line.
    axross_manager: bool,
    /// A system to open again once the systems are listed again.
    retry_after_refresh: Option<String>,
    egui_ctx: egui::Context,
    /// The fallback fonts are in (tests wait for them).
    pub fonts_ready: bool,
    /// The fonts found were given up after a frame failed with them.
    pub fonts_dropped: bool,
    /// The next frame fails as a damaged font would (tests).
    pub test_font_failure: bool,
    /// The cache folder's size and files (once pruned).
    cache_usage: Option<(u64, usize)>,
    /// Settings was open last frame (its opening measures the cache).
    settings_shown: bool,
    cache_clearing: bool,
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
        // (a character no font has is drawn as a box - in a test harness
        // too, which would stop at it)
        ctx.set_missing_glyph_policy(egui::MissingGlyphPolicy::Tofu);
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
        let (preview_jobs, preview_rx) = channel();
        {
            let (tx, wake) = (bg_tx.clone(), wake.clone());
            let _ = std::thread::Builder::new()
                .name("afm-preview".into())
                .spawn(move || preview_worker(preview_rx, tx, wake));
        }
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
            preview_jobs,
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
            focus_filter: false,
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
            parts: FrameParts::default(),
            last_pick: None,
            render_stats,
            has_gpu,
            updater,
            captured: false,
            started: Instant::now(),
            last_frame: Instant::now(),
            last_teleport: -10.0,
            frames: 0,
            queued_teleport: None,
            queued_target: None,
            placed_sides: Vec::new(),
            view_rect: Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)),
            last_frame_instances: 0,
            test_signal: 0.0,
            window_size: None,
            starting: false,
            axross: Axross::Looking,
            beams_dirty: false,
            cut: false,
            auto_frame: true,
            framed: None,
            frame_on: None,
            view_aspect: 1.6,
            motion: 0.0,
            last_cam: None,
            stuck_for: 0.0,
            stuck_said: false,
            speed_said: None,
            title_rects: Vec::new(),
            open_ask: None,
            open_waiting: None,
            trust_type: false,
            add_source_text: String::new(),
            focused: true,
            hidden: false,
            legend_open: true,
            machine_scale: 1.0,
            tp_cache: TeleportCache::default(),
            tp_districts: std::collections::HashMap::new(),
            systems_version: 0,
            teleport_filter: String::new(),
            live: std::collections::HashMap::new(),
            stale_systems: None,
            server_restarts: 0,
            server_restart_at: None,
            attempts: 0,
            pending_fly: None,
            axross_manager: false,
            retry_after_refresh: None,
            egui_ctx: ctx.clone(),
            fonts_ready: false,
            fonts_dropped: false,
            test_font_failure: false,
            cache_usage: None,
            settings_shown: false,
            cache_clearing: false,
            opts,
        };
        if let settings::Loaded::Salvaged { keys, backup } = &loaded {
            let kept = backup.as_ref().map_or(String::new(), |b| {
                format!("; the file as it was is kept as {}", b.display())
            });
            app.toast(format!(
                "Some settings were not understood and have their defaults: {}{kept}",
                shortlist(keys, 6)
            ));
        }
        if let settings::Loaded::SetAside(p) = loaded {
            app.toast(format!(
                "The settings file could not be read; set aside as {}",
                p.display()
            ));
        }
        app.ask_mode_open = app.settings.ask_mode_before_flight;
        {
            // (scanned on a thread: the window never waits for it)
            let (tx, wake) = (app.bg_tx.clone(), app.wake.clone());
            crate::fonts::find_in_background(move |f| {
                let _ = tx.send(Bg::Fonts(f));
                wake();
            });
        }
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
        // the mounts, the machine, the paths: on a thread of their own - a
        // hung mount never keeps the window from coming
        let wants = world::StartupWants {
            paths: self.opts.paths.clone(),
            mounts: self.opts.paths.is_empty() && self.opts.mounts && self.opts.synthetic.is_none(),
            show_pseudo: self.settings.show_pseudo_mounts,
            show_binds: self.settings.show_bind_mounts,
            hardware: self.opts.hardware && self.settings.hardware_island,
            machine: self.opts.machine.clone(),
        };
        let probe: Arc<dyn world::Probe> = self
            .opts
            .probe
            .clone()
            .unwrap_or_else(|| Arc::new(world::RealProbe));
        let tx = self.bg_tx.clone();
        let wake = self.wake.clone();
        self.starting = true;
        let _ = std::thread::Builder::new()
            .name("afm-startup".into())
            .spawn(move || {
                let found = world::startup(&*probe, &wants);
                let _ = tx.send(Bg::Startup(Box::new(found)));
                wake();
            });
        if let Some(files) = self.opts.synthetic {
            let i = self.add_island(
                IslandSource::Synthetic { files, seed: 7 },
                format!("synthetic city ({files} files)"),
                Ring::Local,
            );
            self.islands[i].expected = files + files / 6;
            self.start_walk(i, &[]);
        }
        if self.opts.mcp && self.settings.mcp.enabled {
            self.connect_mcp();
        }
        self.probe_axross();
        self.prune_cache(false);
    }

    /// What the start's thread found: the machine's island, the local ones.
    fn started(&mut self, found: world::Startup) {
        self.starting = false;
        if let (Some(m), Some(hwi)) = (found.machine, found.hw) {
            let id = self.next_id;
            self.next_id += 1;
            let mut isl = Island::new(
                id,
                IslandSource::Hardware,
                "This machine".into(),
                Ring::Hardware,
            );
            isl.tree = Arc::new(hwi.tree.clone());
            let l = Arc::new(hwi.layout.clone());
            let boxes = Arc::new(city::boxes_of(&l));
            let bvh = Arc::new(crate::pick::Bvh::build(&boxes));
            let mut c = IslandCity::new(Ring::Hardware, l, bvh, boxes);
            c.rebuild(self.theme);
            isl.city = Some(c);
            isl.hw = Some(hwi);
            isl.dirty = false;
            isl.status.done = true;
            // (first: the machine stands in the middle)
            self.islands.insert(0, isl);
            // the synthetic city lives on the first disk (for the beam)
            if !m.disks.is_empty() {
                for i in &mut self.islands {
                    if matches!(i.source, IslandSource::Synthetic { .. }) {
                        i.disks = vec![0];
                    }
                }
            }
            if self.opts.machine.is_none() {
                self.sampler = Some(hw::Sampler::start(self.wake.clone()));
            }
            self.machine = Some(m);
            self.placed_sides.clear();
            self.search_reset_all();
        }
        for local in found.locals {
            let title = match &local.mount {
                Some(m) => m.label(),
                None => local.root.to_string_lossy().into_owned(),
            };
            let sensitive = local
                .mount
                .as_ref()
                .is_some_and(|m| matches!(m.kind, MountKind::Network | MountKind::Removable));
            let i = self.add_island(
                IslandSource::Local {
                    root: local.root,
                    mount: local.mount,
                },
                title,
                Ring::Local,
            );
            self.islands[i].disks = local.disks;
            // (room for what its file system holds, up to the walk's budget)
            self.islands[i].expected = local.expected.map_or(0, |n| {
                (n as usize).min(self.settings.walk_budget.max(1) * 11 / 10)
            });
            self.islands[i].sensitive = sensitive;
            self.start_walk(i, &local.skip);
        }
        // the folders added in Sources before (not with folders given on
        // the command line: those alone, then)
        if self.opts.paths.is_empty() {
            for p in self.settings.sources.clone() {
                self.request_source(&p);
            }
        }
    }

    /// Islands were put before others: every search hit's island index
    /// moved - the search starts over.
    fn search_reset_all(&mut self) {
        if let Some(s) = &self.search {
            let q = s.query.clone();
            self.search = Some(Search::new(&q));
        }
        self.selected = None;
        self.sel_box = None;
        self.preview = None;
        self.hover = None;
    }

    fn add_island(&mut self, source: IslandSource, title: String, ring: Ring) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        let remote = source.is_remote();
        let mut isl = Island::new(id, source, title, ring);
        // (a remote backend's walk is cached only when the settings say so)
        isl.sensitive = remote;
        self.islands.push(isl);
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
        let cache_dir = cache_dir.filter(|_| !isl.sensitive || self.settings.cache_sensitive);
        let root_name = isl.tree.root().name.clone();
        // (a city that is only a placeholder - a system being opened, a
        // walk that failed - is replaced as the walk streams in)
        if (isl.city.is_some() && isl.tree.len() > 1) || isl.from_cache.is_some() {
            // a city stands: the new walk goes beside it
            isl.fresh = Some(crate::tree::Tree::with_room(
                &root_name,
                isl.expected.min(4_000_000),
            ));
        } else {
            let mut t = crate::tree::Tree::new(&root_name, 0);
            // (untouched room: memory is only used as entries come)
            t.nodes.reserve(isl.expected.min(4_000_000));
            isl.replace_tree(t);
            isl.fresh = None;
            if let Some(dir) = cache_dir.filter(|_| isl.generation == 1) {
                // the cached city first, the walk beside it
                isl.fresh = Some(crate::tree::Tree::with_room(
                    &root_name,
                    isl.expected.min(4_000_000),
                ));
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

    /// axross's server, watched: one that ended (its process gone, its
    /// pipe closed) is said, its systems marked, its last list kept marked
    /// as old - and it is started again by itself, after 2, 4, 8 ... up to
    /// 60 seconds (five times; Sources' "try again" any time).
    fn watch_server(&mut self) {
        if let McpState::Connected {
            client, systems, ..
        } = &self.mcp
            && !client.is_alive()
        {
            self.stale_systems = systems.as_ref().ok().cloned();
            self.mcp = McpState::Failed("axross's server ended".into());
            self.systems_version += 1;
            for l in self.live.values_mut() {
                if matches!(
                    l.state,
                    SysState::Connecting | SysState::Open | SysState::Walking
                ) {
                    l.state = SysState::Failed("axross's server ended".into());
                    l.since = None;
                }
            }
            self.server_restarts += 1;
            self.server_restart_at = Some(Instant::now() + server_backoff(self.server_restarts));
            self.toast("axross's server ended: it is started again in a moment".into());
        }
        if let Some(at) = self.server_restart_at
            && Instant::now() >= at
            && matches!(self.mcp, McpState::Failed(_))
        {
            self.server_restart_at = None;
            if self.server_restarts <= SERVER_RESTARTS {
                self.connect_mcp();
            }
        }
    }

    fn connect_mcp(&mut self) {
        self.mcp = McpState::Looking;
        let tx = self.bg_tx.clone();
        let wake = self.wake.clone();
        let configured = self.settings.mcp.command.clone();
        let allow = self.settings.mcp.allow_uvx_download;
        let given = self.opts.mcp_client.clone();
        let _ = std::thread::Builder::new()
            .name("afm-mcp".into())
            .spawn(move || {
                if let Some(c) = given {
                    // (the tests' server, connected already)
                    let result = c.backends(Duration::from_secs(30)).map(|b| {
                        let systems = c.systems(Duration::from_secs(30));
                        (
                            c.clone(),
                            "given".to_string(),
                            "(given)".to_string(),
                            b,
                            systems,
                        )
                    });
                    let _ = tx.send(Bg::Mcp(result.map_err(Some)));
                    wake();
                    return;
                }
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
                // (read-only: never --mcp-write; the remote group for the
                // saved connections - a server that does not know it is
                // started without it, and then says it cannot list them)
                let (client, used) = match Client::spawn(&cmd, timeout) {
                    Ok(c) => (Ok(c), cmd.clone()),
                    Err(_) if cmd.iter().any(|a| a == "--mcp-tools") => {
                        let plain = mcp::without_groups(&cmd);
                        (Client::spawn(&plain, timeout), plain)
                    }
                    Err(e) => (Err(e), cmd.clone()),
                };
                let line = used
                    .iter()
                    .map(|a| crate::tree::display_text(a))
                    .collect::<Vec<_>>()
                    .join(" ");
                let result = client.and_then(|c| {
                    let c = Arc::new(c);
                    let backends = c.backends(Duration::from_secs(30))?;
                    let systems = c.systems(Duration::from_secs(30));
                    Ok((c, label, line, backends, systems))
                });
                let _ = tx.send(Bg::Mcp(result.map_err(Some)));
                wake();
            });
    }

    // ------------------------------------------------------------------
    // the systems of axross's connection manager
    // ------------------------------------------------------------------

    /// The saved connections axross's server listed (empty when it cannot).
    pub fn system_profiles(&self) -> Vec<mcp::Profile> {
        match &self.mcp {
            McpState::Connected { systems: Ok(s), .. } => s.profiles.clone(),
            _ => Vec::new(),
        }
    }

    fn profile(&self, name: &str) -> Option<mcp::Profile> {
        match &self.mcp {
            McpState::Connected { systems: Ok(s), .. } => {
                s.profiles.iter().find(|p| p.name == name).cloned()
            }
            _ => None,
        }
    }

    /// The protocol's name in axross's registry.
    fn protocol_name(&self, id: &str) -> Option<String> {
        match &self.mcp {
            McpState::Connected { systems: Ok(s), .. } => s
                .protocols
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.name.clone()),
            _ => None,
        }
    }

    /// The island of a system (by its profile's name).
    pub fn system_island(&self, name: &str) -> Option<usize> {
        if let Some(id) = self.live.get(name).and_then(|l| l.island)
            && let Some(i) = self.islands.iter().position(|x| x.id == id)
        {
            return Some(i);
        }
        self.islands.iter().position(|x| {
            matches!(&x.source, IslandSource::Mcp { backend, .. } if backend.profile.as_deref() == Some(name))
        })
    }

    /// Where a system stands, its island and profile known already (the
    /// menu's rows: no search per row).
    fn system_state_with(&self, name: &str, island: Option<usize>, p: &mcp::Profile) -> SysState {
        if let Some(l) = self.live.get(name)
            && matches!(
                l.state,
                SysState::Connecting
                    | SysState::Failed(_)
                    | SysState::NeedsSecret
                    | SysState::Unavailable(_)
            )
        {
            return l.state.clone();
        }
        let island = self
            .live
            .get(name)
            .and_then(|l| l.island)
            .and_then(|id| self.islands.iter().position(|x| x.id == id))
            .or(island);
        if let Some(i) = island {
            let isl = &self.islands[i];
            return match &isl.status.error {
                Some(e) => SysState::Failed(e.clone()),
                None if isl.walker.is_none() && !isl.status.done => SysState::NotOpen,
                None if !isl.status.done => SysState::Walking,
                None => SysState::Open,
            };
        }
        if p.needs_secret {
            SysState::NeedsSecret
        } else if !p.openable {
            SysState::Unavailable(p.reason.clone())
        } else {
            SysState::NotOpen
        }
    }

    /// Where a system stands.
    pub fn system_state(&self, name: &str) -> SysState {
        if let Some(l) = self.live.get(name)
            && matches!(
                l.state,
                SysState::Connecting
                    | SysState::Failed(_)
                    | SysState::NeedsSecret
                    | SysState::Unavailable(_)
            )
        {
            return l.state.clone();
        }
        if let Some(i) = self.system_island(name) {
            let isl = &self.islands[i];
            return match &isl.status.error {
                Some(e) => SysState::Failed(e.clone()),
                None if isl.walker.is_none() && !isl.status.done => SysState::NotOpen,
                None if !isl.status.done => SysState::Walking,
                None => SysState::Open,
            };
        }
        match self.profile(name) {
            Some(p) if p.needs_secret => SysState::NeedsSecret,
            Some(p) if !p.openable => SysState::Unavailable(p.reason),
            _ => SysState::NotOpen,
        }
    }

    /// Teleports to a system: there at once when its island stands;
    /// otherwise it is opened through axross's server - its island raised
    /// at once, "connecting…" - and the camera flies there. `path`: a
    /// district inside it to fly to once it has been read (a saved spot).
    pub fn teleport_system(&mut self, name: &str, path: Vec<Vec<u8>>) {
        if let Some(l) = self.live.get_mut(name) {
            l.visited = Instant::now();
        }
        let Some(profile) = self.profile(name) else {
            // (no longer in axross's list: an island that still stands is
            // still flown to)
            if let Some(i) = self.system_island(name) {
                self.fly_when_ready(i, path, name);
            } else {
                self.toast(format!(
                    "{} is no longer among axross's saved connections",
                    crate::wire::shown(name, 80)
                ));
            }
            return;
        };
        let state = self.system_state(name);
        if state == SysState::NeedsSecret {
            self.open_ask = Some(OpenAsk::Unlock {
                name: profile.name.clone(),
            });
            return;
        }
        if let Some(i) = self.system_island(name) {
            if matches!(state, SysState::Failed(_) | SysState::NotOpen) {
                self.open_system(name, Some(i));
            }
            self.fly_when_ready(i, path, &profile.name);
            return;
        }
        match state {
            SysState::NeedsSecret => {
                self.open_ask = Some(OpenAsk::Unlock {
                    name: profile.name.clone(),
                });
                return;
            }
            SysState::Unavailable(why) => {
                self.toast(format!("{} cannot be opened here: {why}", profile.name));
                return;
            }
            _ => {}
        }
        if let Some(i) = self.open_system(name, None) {
            self.fly_when_ready(i, path, &profile.name);
        }
    }

    /// Flies to island `i` (and the district at `path` in it) as soon as
    /// it stands (and that district has been read).
    fn fly_when_ready(&mut self, i: usize, path: Vec<Vec<u8>>, what: &str) {
        self.pending_fly = Some(PendingFly {
            island: self.islands[i].id,
            path,
            deadline: Instant::now() + crate::systems::SPOT_TIMEOUT,
            what: what.to_string(),
        });
    }

    /// Raises the island of a system (unless it stands: `at`) and opens it
    /// through axross's server on a thread of its own. The island's index.
    fn open_system(&mut self, name: &str, at: Option<usize>) -> Option<usize> {
        let profile = self.profile(name)?;
        let McpState::Connected { client, .. } = &self.mcp else {
            return None;
        };
        let client = client.clone();
        if profile.needs_secret {
            self.live.entry(name.to_string()).or_default().state = SysState::NeedsSecret;
            return at;
        }
        let i = match at.or_else(|| self.system_island(name)) {
            Some(i) => i,
            None => {
                let family = crate::systems::Family::of(&profile.protocol);
                let proto = profile
                    .protocol_name
                    .clone()
                    .or_else(|| self.protocol_name(&profile.protocol))
                    .unwrap_or_else(|| profile.protocol.clone());
                let title = format!("{} {}  ({proto}, axross)", family.sign(), profile.name);
                let backend = Backend {
                    id: profile
                        .open_as
                        .clone()
                        .unwrap_or_else(|| format!("profile:{}", profile.name)),
                    class: String::new(),
                    name: profile.name.clone(),
                    remote: true,
                    is_default: false,
                    profile: Some(profile.name.clone()),
                    protocol: Some(profile.protocol.clone()),
                    production: profile.production,
                };
                self.add_island(
                    IslandSource::Mcp {
                        client: client.clone(),
                        backend,
                        root: "/".into(),
                    },
                    title,
                    Ring::Remote,
                )
            }
        };
        self.attempts += 1;
        let attempt = self.attempts;
        {
            let isl = &mut self.islands[i];
            // (a walk that runs is ended: the new connect starts it again)
            isl.walker = None;
            isl.status = walk::Status::default();
            isl.generation += 1;
        }
        let live = self.live.entry(name.to_string()).or_default();
        live.state = SysState::Connecting;
        live.island = Some(self.islands[i].id);
        live.since = Some(Instant::now());
        live.attempt = attempt;
        let tx = self.bg_tx.clone();
        let wake = self.wake.clone();
        let n = name.to_string();
        // (the window gives up at the connect's time limit - see
        // `connect_deadlines`; the call itself waits much longer, so that a
        // late success is heard and closed on the server)
        let _ = std::thread::Builder::new()
            .name("afm-system-open".into())
            .spawn(move || {
                let result = client.open_profile(&n, crate::systems::LATE_ANSWER);
                let _ = tx.send(Bg::SystemOpened {
                    name: n,
                    attempt,
                    result,
                });
                wake();
            });
        Some(i)
    }

    /// A system's connect came back: its walk starts, or it failed (said
    /// on its island and in the menu).
    fn system_opened(&mut self, name: &str, attempt: u64, result: Result<mcp::Opened, String>) {
        let Some(live) = self.live.get_mut(name) else {
            return;
        };
        if live.attempt != attempt || live.state != SysState::Connecting {
            // (cancelled, timed out or tried again since: this answer is
            // old - a connection it opened is closed on the server unless a
            // newer try uses it)
            if let (Ok(opened), false) = (&result, live.state == SysState::Connecting) {
                let b = opened.backend.clone();
                self.close_on_server(&b);
            }
            return;
        }
        live.since = None;
        let Some(i) = live
            .island
            .and_then(|id| self.islands.iter().position(|x| x.id == id))
        else {
            return;
        };
        match result {
            Ok(opened) => {
                live.state = SysState::Walking;
                if let IslandSource::Mcp { backend, .. } = &mut self.islands[i].source {
                    backend.id = opened.backend;
                    if let Some(p) = opened.protocol {
                        backend.protocol = Some(p);
                    }
                }
                self.start_walk(i, &[]);
            }
            Err(e) => {
                // (whatever the server may hold of it: closed, best effort)
                self.close_on_server(&format!("profile:{name}"));
                let Some(live) = self.live.get_mut(name) else {
                    return;
                };
                let lower = e.to_ascii_lowercase();
                let secret = (lower.contains("password") || lower.contains("passphrase"))
                    && (lower.contains("not stored") || lower.contains("keyring"));
                let too_many = lower.contains("at most") && lower.contains("open at once");
                live.state = if secret {
                    SysState::NeedsSecret
                } else {
                    SysState::Failed(e.clone())
                };
                let isl = &mut self.islands[i];
                isl.status.error = Some(e);
                isl.status.done = true;
                if self
                    .pending_fly
                    .as_ref()
                    .is_some_and(|f| f.island == isl.id && !f.path.is_empty())
                {
                    self.pending_fly = None;
                }
                if too_many {
                    // (the server's limit of open systems: say which are
                    // open, offer to close the one least recently visited)
                    let mut open: Vec<(String, Instant)> = self
                        .live
                        .iter()
                        .filter(|(n, l)| {
                            n.as_str() != name
                                && matches!(l.state, SysState::Walking | SysState::Open)
                                && l.island.is_some()
                        })
                        .map(|(n, l)| (n.clone(), l.visited))
                        .collect();
                    open.sort_by_key(|(_, at)| *at);
                    self.open_ask = Some(OpenAsk::TooMany {
                        name: name.to_string(),
                        open: open.into_iter().map(|(n, _)| n).collect(),
                    });
                }
            }
        }
    }

    /// Closes a backend on axross's server (a thread; best effort).
    fn close_on_server(&self, backend: &str) {
        if let McpState::Connected { client, .. } = &self.mcp {
            let (client, backend) = (client.clone(), backend.to_string());
            let _ = std::thread::Builder::new()
                .name("afm-system-close".into())
                .spawn(move || {
                    let _ = client.close_profile(&backend, Duration::from_secs(30));
                });
        }
    }

    /// Closes a system: on the server (it frees one of its open slots) and
    /// here - its walk ends, its island goes.
    pub fn close_system(&mut self, name: &str) {
        let Some(i) = self.system_island(name) else {
            self.live.remove(name);
            return;
        };
        let backend = match &self.islands[i].source {
            IslandSource::Mcp { backend, .. } => backend.id.clone(),
            _ => return,
        };
        self.close_on_server(&backend);
        self.live.remove(name);
        self.remove_island(i);
        self.toast(format!("{name} closed"));
    }

    /// Stops island `i`'s walk: what it found stays, said to be all there
    /// is for now ("walk again" reads it whole).
    pub fn stop_walk(&mut self, i: usize) {
        let Some(isl) = self.islands.get_mut(i) else {
            return;
        };
        if isl.walker.take().is_none() {
            return;
        }
        // (its last messages are let go; the city stands as it is)
        isl.generation += 1;
        if let Some(fresh) = isl.fresh.take()
            && fresh.len() > isl.tree.len()
        {
            isl.replace_tree(fresh);
        }
        isl.status.done = true;
        isl.status.note = Some("stopped by hand".into());
        isl.dirty = true;
        isl.epoch += 1;
        let title = isl.title.clone();
        self.world_version += 1;
        self.toast(format!("{title}: walk stopped"));
    }

    /// Adds a folder as an island and walks it (Sources). Err: why not.
    pub fn add_source(&mut self, root: PathBuf) -> Result<usize, String> {
        if !root.is_absolute() {
            return Err(format!(
                "{}: give the whole path (from / or ~)",
                root.display()
            ));
        }
        // one island per folder: not the same again, not one inside
        // another (its files counted twice), nor one holding another
        for isl in &self.islands {
            let IslandSource::Local { root: r, .. } = &isl.source else {
                continue;
            };
            if *r == root {
                return Err(format!("{} is an island already", root.display()));
            }
            if root.starts_with(r) {
                return Err(format!(
                    "{} lies inside the island {}: it is shown there already",
                    root.display(),
                    isl.title
                ));
            }
            if r.starts_with(&root) {
                return Err(format!(
                    "the island {} lies inside {}: take that island out first",
                    isl.title,
                    root.display()
                ));
            }
        }
        // (whether it can be read is the walk's to find out - off the
        // window's thread: a dead mount never holds the window)
        let title = root.to_string_lossy().into_owned();
        let key = title.clone();
        let i = self.add_island(
            IslandSource::Local { root, mount: None },
            title,
            Ring::Local,
        );
        self.start_walk(i, &[]);
        self.placed_sides.clear();
        self.beams_dirty = true;
        self.world_version += 1;
        // (kept: it is an island at the next start too)
        if !self.settings.sources.contains(&key) {
            self.settings.sources.push(key);
            let _ = settings::save(&self.settings_path, &self.settings);
        }
        Ok(i)
    }

    /// Adds a folder as Sources' field names it: `~` is the home folder;
    /// the path is made whole (links and `..` resolved) and looked at on a
    /// thread - a dead mount never holds the window; then
    /// [`App::add_source`].
    pub fn request_source(&mut self, text: &str) {
        let text = text.trim().to_string();
        let path = expand_home(&text, dirs::home_dir().as_deref());
        if !path.is_absolute() {
            self.toast(format!("{text}: give the whole path (from / or ~)"));
            return;
        }
        let tx = self.bg_tx.clone();
        let wake = self.wake.clone();
        let _ = std::thread::Builder::new()
            .name("afm-source-check".into())
            .spawn(move || {
                let r = match std::fs::canonicalize(&path) {
                    Ok(p) if p.is_dir() => Ok(p),
                    Ok(p) => Err(format!("{}: no folder (a file is no island)", p.display())),
                    Err(e) => Err(format!("{}: {e}", path.display())),
                };
                let _ = tx.send(Bg::SourceChecked(r));
                wake();
            });
    }

    /// Takes island `i` out of the world (what pointed into it is let go).
    pub fn remove_island(&mut self, i: usize) {
        let isl = self.islands.remove(i);
        // (a folder added in Sources: not again at the next start)
        if let IslandSource::Local { root, .. } = &isl.source {
            let key = root.to_string_lossy().into_owned();
            if let Some(k) = self.settings.sources.iter().position(|s| *s == key) {
                self.settings.sources.remove(k);
                let _ = settings::save(&self.settings_path, &self.settings);
            }
        }
        if self
            .pending_fly
            .as_ref()
            .is_some_and(|f| f.island == isl.id)
        {
            self.pending_fly = None;
        }
        world::drop_later(isl);
        // (indices moved: the search starts over; a choice on another
        // island stays, its index moved with it)
        let keep = self
            .selected
            .filter(|(k, _)| *k != i)
            .map(|(k, n)| (if k > i { k - 1 } else { k }, n));
        let preview = if keep.is_some() {
            self.preview.take()
        } else {
            None
        };
        self.search_reset_all();
        if let Some((k, n)) = keep {
            self.selected = Some((k, n));
            self.sel_box = self.node_box(k, n);
            self.preview = preview;
        }
        self.placed_sides.clear();
        self.beams_dirty = true;
        self.world_version += 1;
    }

    /// Connects that ran past their time limit: given up here, said so
    /// (their late answer is closed on the server when it comes).
    fn connect_deadlines(&mut self) {
        let limit = self
            .opts
            .connect_timeout
            .unwrap_or(crate::systems::CONNECT_TIMEOUT);
        let late: Vec<String> = self
            .live
            .iter()
            .filter(|(_, l)| {
                l.state == SysState::Connecting && l.since.is_some_and(|t| t.elapsed() > limit)
            })
            .map(|(n, _)| n.clone())
            .collect();
        for n in late {
            let why = format!(
                "no answer within {} s (the host may be down)",
                limit.as_secs()
            );
            self.give_up_system(&n, &why);
        }
    }

    /// Gives up a connect under way (its late answer is closed on the
    /// server); the island stays, "cancelled", with a retry.
    pub fn cancel_system(&mut self, name: &str) {
        self.give_up_system(name, "cancelled");
    }

    fn give_up_system(&mut self, name: &str, why: &str) {
        let Some(live) = self.live.get_mut(name) else {
            return;
        };
        if live.state != SysState::Connecting {
            return;
        }
        live.state = SysState::Failed(why.into());
        live.since = None;
        let island = live.island;
        // given up on the server at once too (its slot freed now - a retry
        // is never told "being connected already"); a late answer is
        // closed again when it comes
        self.close_on_server(&format!("profile:{name}"));
        if let Some(i) = island.and_then(|id| self.islands.iter().position(|x| x.id == id)) {
            let isl = &mut self.islands[i];
            isl.status.error = Some(why.into());
            isl.status.done = true;
            if self
                .pending_fly
                .as_ref()
                .is_some_and(|f| f.island == isl.id)
            {
                self.pending_fly = None;
            }
        }
    }

    /// Lists the systems again (after unlocking one in axross).
    pub fn refresh_systems(&mut self) {
        let McpState::Connected { client, .. } = &self.mcp else {
            return;
        };
        let client = client.clone();
        let tx = self.bg_tx.clone();
        let wake = self.wake.clone();
        let _ = std::thread::Builder::new()
            .name("afm-systems".into())
            .spawn(move || {
                let _ = tx.send(Bg::Systems(client.systems(Duration::from_secs(30))));
                wake();
            });
    }

    /// The flight that waits for its target: there when the island stands
    /// (and the district at its path has been read - folders on the way
    /// asked for); given up at its deadline or when the island failed.
    fn pending_flight(&mut self) {
        let Some(f) = self.pending_fly.clone() else {
            return;
        };
        let Some(i) = self.islands.iter().position(|x| x.id == f.island) else {
            self.pending_fly = None;
            return;
        };
        if let Some(e) = self.islands[i].status.error.clone() {
            self.pending_fly = None;
            self.toast(format!("{}: {e}", f.what));
            return;
        }
        if Instant::now() > f.deadline {
            self.pending_fly = None;
            self.toast(format!("{} could not be reached in time", f.what));
            return;
        }
        let shown = self.islands[i]
            .city
            .as_ref()
            .is_some_and(|c| c.shown && c.target.is_none_or(|t| t == c.offset));
        if !shown {
            return;
        }
        let (node, rest) = find_path(&self.islands[i].tree, &f.path);
        if rest == 0 {
            // (its district stands in the layout: there)
            if self.node_box(i, node).is_some() {
                self.pending_fly = None;
                self.teleport_to(i, node);
            }
            return;
        }
        // the folder on the way, read when it is not yet
        let isl = &mut self.islands[i];
        if let Some(n) = isl.tree.get(node)
            && n.kind == Kind::Dir
            && matches!(n.listing, crate::tree::Listing::Unlisted)
            && isl.can_ask()
            && isl.requested.insert(node)
            && let Some(w) = isl.walker.as_ref()
        {
            w.list(node);
        }
    }

    /// Ends axross's server: killed and reaped, a call still waiting
    /// included (the walks holding the client hear "ended").
    pub fn shutdown_mcp(&mut self) {
        if let McpState::Connected { client, .. } = &self.mcp {
            client.shutdown();
        }
        for isl in &self.islands {
            if let IslandSource::Mcp { client, .. } = &isl.source {
                client.shutdown();
            }
        }
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
        let mut lap: Vec<(&str, Duration)> = Vec::new();
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
        lap.push(("walks", start.elapsed()));
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
                Event::Batch(b) => match isl.apply_batch(b) {
                    Ok(true) => changed.push(i),
                    Ok(false) => {}
                    Err(world::OutOfStep) => {
                        // out of step (never expected): the walk ends here
                        isl.walker = None;
                    }
                },
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
                }
            }
        }
        lap.push(("deferred", start.elapsed()));
        // batches that waited for a tree lent out (the layout, the cache)
        for (i, isl) in self.islands.iter_mut().enumerate() {
            if !isl.deferred.is_empty() {
                match isl.drain_deferred(Duration::from_millis(4)) {
                    Ok(true) => changed.push(i),
                    Ok(false) => {}
                    Err(world::OutOfStep) => isl.walker = None,
                }
            }
        }
        // a walk done and every batch of it in the tree: cached
        for i in 0..self.islands.len() {
            let isl = &self.islands[i];
            if isl.status.done && isl.fresh.is_none() && !isl.saved && isl.deferred.is_empty() {
                self.islands[i].saved = true;
                self.save_cache(i);
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
        lap.push(("layouts", start.elapsed()));
        // layouts, and instances for islands that moved
        while let Ok(made) = self.layout.rx.try_recv() {
            match made {
                world::Made::Layout(done) => {
                    let Some(isl) = self.islands.iter_mut().find(|i| i.id == done.id) else {
                        continue;
                    };
                    isl.layout_pending = false;
                    if done.generation != isl.epoch || done.layout.nodes > isl.tree.len() {
                        isl.dirty = true;
                        continue;
                    }
                    isl.stale = false;
                    let mut c = IslandCity::new(isl.ring, done.layout, done.bvh, done.boxes);
                    // (a new island is drawn once it stands where it belongs;
                    // one that stood keeps standing where it was meanwhile)
                    // (placed again by place_islands: a placing asked for
                    // the old layout is dropped when it comes)
                    c.shown = isl.city.as_ref().is_some_and(|o| o.shown);
                    c.set_instances(done.inst);
                    // (the old city's memory: given back elsewhere)
                    if let Some(old) = isl.city.take() {
                        world::drop_later(old);
                    }
                    if let Some(tint) = family_tint(&isl.source) {
                        Arc::make_mut(&mut c.inst).ground.color = tint;
                    }
                    isl.city = Some(c);
                    self.world_version += 1;
                    self.placed_sides.clear();
                }
                world::Made::Placed { id, layout, inst } => {
                    let tint = self
                        .islands
                        .iter()
                        .find(|i| i.id == id)
                        .and_then(|i| family_tint(&i.source));
                    if let Some(c) = self
                        .islands
                        .iter_mut()
                        .find(|i| i.id == id)
                        .and_then(|i| i.city.as_mut())
                        .filter(|c| Arc::ptr_eq(&c.layout, &layout))
                    {
                        let old = std::mem::take(&mut c.inst);
                        c.set_instances(inst);
                        world::drop_later(old);
                        if let Some(tint) = tint {
                            Arc::make_mut(&mut c.inst).ground.color = tint;
                        }
                        self.world_version += 1;
                        self.beams_dirty = true;
                    }
                }
            }
        }
        lap.push(("ask", start.elapsed()));
        // ask for layouts (the tree lent, not copied)
        let theme = self.theme;
        let by_alloc = self.settings.size_by_allocated;
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
                    tree: isl.tree.clone(),
                    ring: isl.ring,
                    offset: isl.city.as_ref().map_or([0.0, 0.0], |c| c.offset),
                    theme,
                    by_alloc,
                });
            }
        }
        lap.push(("bg", start.elapsed()));
        // the rest
        while let Ok(bg) = self.bg_rx.try_recv() {
            match bg {
                Bg::Mcp(Ok((client, label, command, backends, systems))) => {
                    self.systems_version += 1;
                    self.stale_systems = None;
                    self.server_restarts = 0;
                    self.server_restart_at = None;
                    self.mcp = McpState::Connected {
                        label,
                        command,
                        client: client.clone(),
                        systems,
                    };
                    self.add_backends(&client, backends);
                    // the systems opened at the start (on their own: a hung
                    // host holds nothing else)
                    for name in self.settings.mcp.open_profiles.clone() {
                        self.open_system(&name, None);
                    }
                }
                Bg::SystemOpened {
                    name,
                    attempt,
                    result,
                } => self.system_opened(&name, attempt, result),
                Bg::Fonts(fonts) => {
                    // (once given up after a failure, not again this run)
                    if !self.fonts_dropped {
                        self.egui_ctx.set_fonts(crate::fonts::definitions(fonts));
                        self.fonts_ready = true;
                    }
                }
                Bg::Systems(systems) => {
                    self.systems_version += 1;
                    if let McpState::Connected { systems: s, .. } = &mut self.mcp {
                        *s = systems;
                    }
                    if let Some(name) = self.retry_after_refresh.take() {
                        if let Some(l) = self.live.get_mut(&name) {
                            l.state = SysState::NotOpen;
                        }
                        self.teleport_system(&name, Vec::new());
                    }
                }
                Bg::Mcp(Err(None)) => self.mcp = McpState::NotFound,
                Bg::Mcp(Err(Some(e))) => {
                    self.mcp = McpState::Failed(e);
                    if self.stale_systems.is_some() {
                        // (a start again after the server ended: once more, later)
                        self.server_restarts += 1;
                        self.server_restart_at =
                            Some(Instant::now() + server_backoff(self.server_restarts));
                    }
                }
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
                Bg::Startup(found) => self.started(*found),
                Bg::Axross(state, probe) => {
                    self.axross = state;
                    self.axross_manager = probe.as_ref().is_some_and(|p| p.connection_manager);
                    if probe.is_some() && probe != self.settings.axross_probe {
                        self.settings.axross_probe = probe;
                        let _ = settings::save(&self.settings_path, &self.settings);
                    }
                }
                Bg::CacheUsage(usage) => self.cache_usage = Some(usage),
                Bg::SourceChecked(r) => match r.and_then(|p| self.add_source(p)) {
                    Ok(_) => self.add_source_text.clear(),
                    Err(e) => self.toast(e),
                },
                Bg::CachePruned(pruned, usage) => {
                    self.cache_usage = Some(usage);
                    if pruned.files > 0 && self.cache_clearing {
                        self.toast(format!(
                            "The cache is cleared: {} files, {}",
                            pruned.files,
                            hw::human_bytes(pruned.bytes)
                        ));
                    }
                    self.cache_clearing = false;
                }
                Bg::OpenChecked(check) => self.open_checked(*check),
                Bg::OpenRechecked(path, same) => self.open_rechecked(path, same),
            }
        }
        lap.push(("sampler", start.elapsed()));
        // the machine's live light
        if let Some(sampler) = self.sampler.as_mut()
            && let Some((loads, mem)) = sampler.poll().cloned()
        {
            self.apply_load(&loads, mem);
        }
        lap.push(("place", start.elapsed()));
        self.place_islands();
        if let Some(b) = self.world_box() {
            self.camera.world = (b.max.x - b.min.x).max(b.max.z - b.min.z);
        }
        self.mark_island_states();
        self.frame_first_view();
        self.connect_deadlines();
        self.watch_server();
        if let Some((since, said)) = self.open_waiting
            && !said
            && since.elapsed() > OPEN_PATIENCE
        {
            // (a check that hangs - a dead mount - is said, never waited for)
            self.open_waiting = Some((since, true));
            self.toast(
                "No answer from the disk it lies on: not opened (it opens if the disk answers)"
                    .into(),
            );
        }
        self.pending_flight();
        if self.opts.frame_stats && start.elapsed() > Duration::from_millis(20) {
            lap.push(("end", start.elapsed()));
            let mut prev = Duration::ZERO;
            let parts: Vec<String> = lap
                .iter()
                .map(|(n, t)| {
                    let d = *t - prev;
                    prev = *t;
                    format!("{n} {:.1}", d.as_secs_f32() * 1000.0)
                })
                .collect();
            eprintln!("slow pump: before {}", parts.join(", "));
        }
    }

    /// The processor's load as its towers' light, the memory's in use.
    pub fn apply_load(&mut self, loads: &[f32], mem: f32) {
        let Some(isl) = self.islands.iter_mut().find(|i| i.hw.is_some()) else {
            return;
        };
        let (Some(hwi), Some(c)) = (isl.hw.as_ref(), isl.city.as_mut()) else {
            return;
        };
        let mut changed = false;
        for (node, part) in &hwi.parts {
            let Some(Placed::Building(b)) = c.layout.placed.get(*node as usize).copied() else {
                continue;
            };
            let target = match part {
                Part::Core(id) => loads.get(*id as usize).copied().unwrap_or(0.0),
                Part::Module | Part::MemoryInUse => mem,
                _ => continue,
            };
            // eased: the light follows the load slowly (never a flash); a
            // change too small to see changes nothing (no new frame set)
            let Some(inst) = c.inst.buildings.get(b as usize) else {
                continue;
            };
            let old = ((inst.info >> 8) & 0xff) as f32 / 255.0;
            let new = old + (target - old) * 0.5;
            if (new - old).abs() < 0.03 {
                continue;
            }
            Arc::make_mut(&mut c.inst).buildings[b as usize].set_glow(new);
            changed = true;
        }
        if changed {
            self.world_version += 1;
        }
    }

    /// Whether island `i`'s walk is kept in the cache: not the machine's,
    /// not a tiny one, and one on a removable medium, a network file
    /// system or a remote backend only when the settings say so.
    fn cacheable(&self, i: usize) -> bool {
        let isl = &self.islands[i];
        !matches!(isl.source, IslandSource::Hardware)
            && isl.tree.len() >= 2
            && (!isl.sensitive || self.settings.cache_sensitive)
    }

    /// Saves island `i`'s walk: the tree lent to a thread, encoded there.
    fn save_cache(&self, i: usize) {
        let Some(dir) = self.cache_dir() else { return };
        if !self.cacheable(i) {
            return;
        }
        let isl = &self.islands[i];
        let tree = isl.tree.clone();
        let key = isl.source.key();
        let _ = std::thread::Builder::new()
            .name("afm-cache-save".into())
            .spawn(move || {
                let bytes = cache::encode(&tree, &key, now_secs());
                drop(tree);
                let _ = cache::save(&dir, &key, &bytes);
            });
    }

    /// Prunes the cache folder (at the start, and "clear" in the
    /// settings: everything) - on a thread.
    /// The walk cache's size and files, as last measured.
    pub fn cache_usage(&self) -> Option<(u64, usize)> {
        self.cache_usage
    }

    /// Measures the walk cache again (on a thread): Settings shows it as it
    /// is, not as it was at the start.
    fn measure_cache(&mut self) {
        let Some(dir) = self.cache_dir() else { return };
        let tx = self.bg_tx.clone();
        let wake = self.wake.clone();
        let _ = std::thread::Builder::new()
            .name("afm-cache-usage".into())
            .spawn(move || {
                let _ = tx.send(Bg::CacheUsage(cache::usage(&dir)));
                wake();
            });
    }

    fn prune_cache(&mut self, everything: bool) {
        let Some(dir) = self.cache_dir() else { return };
        let (max_bytes, max_age) = if everything {
            (0, Duration::ZERO)
        } else {
            (
                self.settings.cache_max_mb << 20,
                Duration::from_secs(self.settings.cache_max_days * 86_400),
            )
        };
        self.cache_clearing = everything;
        let tx = self.bg_tx.clone();
        let wake = self.wake.clone();
        let _ = std::thread::Builder::new()
            .name("afm-cache-prune".into())
            .spawn(move || {
                let pruned = cache::prune(&dir, max_bytes, max_age);
                let usage = cache::usage(&dir);
                let _ = tx.send(Bg::CachePruned(pruned, usage));
                wake();
            });
    }

    /// Saves every island's cache and the settings, and ends axross's
    /// server (at the end).
    pub fn save_all(&mut self) {
        if let Some(dir) = self.cache_dir() {
            let jobs: Vec<(Arc<crate::tree::Tree>, String)> = (0..self.islands.len())
                .filter(|&i| self.cacheable(i) && self.islands[i].fresh.is_none())
                .map(|i| (self.islands[i].tree.clone(), self.islands[i].source.key()))
                .collect();
            // (in parallel: the window is closing)
            std::thread::scope(|sc| {
                for (tree, key) in &jobs {
                    let dir = &dir;
                    sc.spawn(move || {
                        let bytes = cache::encode(tree, key, now_secs());
                        let _ = cache::save(dir, key, &bytes);
                    });
                }
            });
        }
        self.shutdown_mcp();
        self.settings.fly_speed = self.camera.speed;
        if let Some(size) = self.window_size {
            self.settings.window = Some(size);
        }
        let _ = settings::save(&self.settings_path, &self.settings);
    }

    /// Places the islands when a side changed: the layout thread makes the
    /// instances of each island that moves (no island is rebuilt here, and
    /// one that stays is not rebuilt at all).
    fn place_islands(&mut self) {
        self.scale_machine();
        let sides: Vec<f32> = self
            .islands
            .iter()
            .map(|i| i.city.as_ref().map_or(0.0, |c| c.side()))
            .collect();
        if sides != self.placed_sides {
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
                let id = self.islands[i].id;
                let hw = self.islands[i].hw.is_some();
                let Some(c) = self.islands[i].city.as_mut() else {
                    continue;
                };
                let want = offsets[k];
                if c.offset == want {
                    c.target = Some(want);
                    c.shown = true;
                    continue;
                }
                if c.target == Some(want) {
                    continue; // (asked already)
                }
                c.target = Some(want);
                if hw {
                    // (the machine's island is small: placed here)
                    c.offset = want;
                    c.rebuild(theme);
                    c.shown = true;
                } else {
                    self.layout.place(world::PlaceJob {
                        id,
                        layout: c.layout.clone(),
                        ring: c.ring,
                        offset: want,
                        theme,
                    });
                }
            }
            self.beams_dirty = true;
        }
        if !std::mem::take(&mut self.beams_dirty) {
            return;
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
                let Some(c) = isl.city.as_ref().filter(|c| c.shown) else {
                    continue;
                };
                // (one beam from each disk it lives on: RAID members, a pool)
                for d in &isl.disks {
                    let Some((_, from)) = disk_at.iter().find(|(k, _)| k == d) else {
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
        }
        self.world_version += 1;
    }

    /// The machine's island grows with the biggest city (about a quarter of
    /// its side, in steps of half again): never a speck beside it.
    fn scale_machine(&mut self) {
        let biggest = self
            .islands
            .iter()
            .filter(|i| i.hw.is_none())
            .filter_map(|i| i.city.as_ref().map(|c| c.side()))
            .fold(0.0f32, f32::max);
        let k = machine_scale(biggest, hw::SIDE);
        if k == self.machine_scale {
            return;
        }
        let theme = self.theme;
        let Some(isl) = self.islands.iter_mut().find(|i| i.hw.is_some()) else {
            return;
        };
        let (Some(hwi), Some(old)) = (isl.hw.as_ref(), isl.city.as_ref()) else {
            return;
        };
        self.machine_scale = k;
        let l = Arc::new(hwi.layout.scaled(k));
        let boxes = Arc::new(city::boxes_of(&l));
        let bvh = Arc::new(crate::pick::Bvh::build(&boxes));
        let mut c = IslandCity::new(Ring::Hardware, l, bvh, boxes);
        c.offset = old.offset;
        c.rebuild(theme);
        isl.city = Some(c);
        // (the light it had comes back with the next sample)
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

    /// The first view: until the camera is moved by hand, the biggest city
    /// is framed (again when it has grown or shrunk by a sixth) - or the
    /// island a teleport or Z framed, while it grows (a system's island
    /// filling in after its connect).
    fn frame_first_view(&mut self) {
        if !self.auto_frame {
            return;
        }
        let shown = |i: &crate::world::Island| {
            i.city
                .as_ref()
                .filter(|c| c.shown && c.target.is_none_or(|t| t == c.offset))
                .map(|c| (c.side(), c.offset))
        };
        let best = match self.frame_on {
            Some(id) => self
                .islands
                .iter()
                .enumerate()
                .find(|(_, i)| i.id == id)
                .and_then(|(k, i)| shown(i).map(|s| (k, i.id, s))),
            None => self
                .islands
                .iter()
                .enumerate()
                .filter(|(_, i)| i.hw.is_none())
                .filter_map(|(k, i)| shown(i).map(|s| (k, i.id, s)))
                .max_by(|a, b| a.2.0.total_cmp(&b.2.0)),
        };
        let Some((k, id, (side, at))) = best else {
            return;
        };
        // (again when it grew or shrank by a sixth, or moved - a growing
        // system's island makes room in its ring)
        let again = match self.framed {
            Some((fid, fside, fat)) => {
                let moved = (at[0] - fat[0]).abs().max((at[1] - fat[1]).abs());
                fid != id
                    || side > fside * 1.17
                    || side < fside * 0.83
                    || moved > side.max(160.0) * 0.1
            }
            None => true,
        };
        if !again {
            return;
        }
        let first = self.framed.is_none() || self.frame_on.is_none();
        self.framed = Some((id, side, at));
        // (only small cities - empty or missing folders - beside the
        // machine's island: the world whole, never half the machine)
        let machine = self
            .islands
            .iter()
            .filter(|i| i.hw.is_some())
            .filter_map(|i| i.city.as_ref().map(|c| c.side()))
            .fold(0.0f32, f32::max);
        let (pos, target) = if self.frame_on.is_none() && side < machine {
            self.box_view(self.world_box().expect("a city"))
        } else {
            self.island_view(k)
        };
        let (yaw, pitch) = look_angles(pos, target);
        if first {
            self.camera.flight = None;
            self.camera.pos = pos;
            self.camera.yaw = yaw;
            self.camera.pitch = pitch;
            self.camera.target = target;
            self.camera.distance = (pos - target).length();
        } else {
            // (an island that grew: the camera glides back to see it whole)
            let cities = self.cities();
            let ceiling = |a: Vec3, b: Vec3| city::ceiling(&cities, a, b);
            let mut cam = self.camera.clone();
            cam.fly_path(pos, (yaw, pitch), &ceiling, true, Some(target));
            self.camera = cam;
        }
        self.world_version += 1;
    }

    /// The low view of island `i`: 15 degrees down, the island whole in
    /// the window, the horizon and the sky's rain in its upper third (an
    /// empty or failed island seen from far enough to read its state).
    /// The eye and the point it looks at.
    fn island_view(&self, i: usize) -> (Vec3, Vec3) {
        let Some(c) = self.islands.get(i).and_then(|x| x.city.as_ref()) else {
            return (self.camera.pos, self.camera.target);
        };
        self.box_view(c.aabb())
    }

    /// The low view of a box (an island's, the world's).
    fn box_view(&self, b: Aabb) -> (Vec3, Vec3) {
        let side = (b.max.x - b.min.x).max(b.max.z - b.min.z);
        let grow = ((160.0 - side) * 0.5).max(0.0);
        let min = vec3(b.min.x - grow, 0.0, b.min.z - grow);
        let max = vec3(b.max.x + grow, b.max.y.max(1.0), b.max.z + grow);
        let eye = crate::camera::frame_box(
            min,
            max,
            crate::camera::LOW_PITCH,
            self.camera.fov_y,
            self.view_aspect,
        );
        // (looked at: the island's middle - orbit turns round it)
        let target = vec3((min.x + max.x) * 0.5, 0.0, (min.z + max.z) * 0.5);
        (eye, target)
    }

    /// Frames an island whole (Z, the inspect panel's button): the low
    /// view, kept while the island grows until the camera is moved.
    pub fn frame_island(&mut self, island: usize) {
        let Some(id) = self
            .islands
            .get(island)
            .filter(|i| i.city.is_some())
            .map(|i| i.id)
        else {
            return;
        };
        let (pos, target) = self.island_view(island);
        let (yaw, pitch) = look_angles(pos, target);
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
        self.keep_framed(island, id);
    }

    /// Keeps island `island` framed while it grows (until moved by hand).
    fn keep_framed(&mut self, island: usize, id: u64) {
        self.auto_frame = true;
        self.frame_on = Some(id);
        self.framed = self
            .islands
            .get(island)
            .and_then(|i| i.city.as_ref())
            .map(|c| (id, c.side(), c.offset));
    }

    /// Home: back to the low view of the biggest city (the first view).
    pub fn home(&mut self) {
        let best = self
            .islands
            .iter()
            .enumerate()
            .filter(|(_, i)| i.hw.is_none())
            .filter_map(|(k, i)| i.city.as_ref().map(|c| (k, c.side())))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(k, _)| k);
        match best {
            Some(k) => {
                self.frame_island(k);
                // (the biggest, whichever that is: not this island for good)
                self.frame_on = None;
            }
            None => self.overview(true),
        }
    }

    /// The islands' grounds show their state: connecting (a breathing
    /// outline), failed (amber hatching).
    fn mark_island_states(&mut self) {
        for isl in &mut self.islands {
            let system = match &isl.source {
                IslandSource::Mcp { backend, .. } => backend.profile.clone(),
                _ => None,
            };
            let connecting = system
                .as_ref()
                .and_then(|n| self.live.get(n))
                .is_some_and(|l| l.state == SysState::Connecting);
            let failed = isl.status.error.is_some() && isl.from_cache.is_none();
            let state = if connecting {
                city::flags::UNREAD
            } else if failed {
                city::flags::DENIED
            } else {
                0
            };
            if let Some(c) = isl.city.as_mut()
                && c.state != state
            {
                c.state = state;
                self.world_version += 1;
            }
        }
    }

    /// The fog's density for the world as it is: about 0.00055 for a city
    /// of a kilometre, thinner for bigger ones.
    fn fog_density(&self) -> f32 {
        let extent = self
            .world_box()
            .map_or(1000.0, |b| (b.max.x - b.min.x).max(b.max.z - b.min.z));
        (1.4 / (extent * 1.6).max(2500.0)).min(0.00056)
    }

    /// How the shader draws far buildings this frame (picking, labels and
    /// the level of detail follow it).
    pub fn raise(&self) -> render::Raise {
        render::Raise {
            look: self.far_look(),
            lens: 2.0 * (self.camera.fov_y * 0.5).tan(),
            height: self.view_rect.height().max(1.0),
            eye: self.camera.pos,
        }
    }

    /// The far view's knobs for the world as it is ([`render::far_look`]).
    fn far_look(&self) -> [f32; 4] {
        let (width, top) = self.world_box().map_or((1000.0, 30.0), |b| {
            ((b.max.x - b.min.x).max(b.max.z - b.min.z), b.max.y)
        });
        render::far_look(width, top)
    }

    /// Looks at the whole world from above and aside.
    pub fn overview(&mut self, fly: bool) {
        self.auto_frame = false;
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

    /// What the pointer would pick at a point of the view (-1..1 each way,
    /// y up): as drawn, raised far roofs and widened far towers included.
    pub fn pick_ndc(&self, ndc: [f32; 2]) -> Option<(usize, NodeId)> {
        let ray = self.camera.ray(ndc, self.view_aspect);
        let cities = self.cities();
        city::pick(&cities, &ray, &self.raise())
            .map(|h| (self.island_of_city(h.island).unwrap_or(0), h.node))
    }

    /// Where a world point is in the view (-1..1 each way, y up), if in
    /// front of the camera.
    pub fn to_ndc(&self, p: Vec3) -> Option<[f32; 2]> {
        let vp = self.camera.proj(self.view_aspect).mul(&self.camera.view());
        let c = vp.transform([p.x, p.y, p.z, 1.0]);
        (c[3] > 0.0).then(|| [c[0] / c[3], c[1] / c[3]])
    }

    /// Whether a world point is inside the 3D view as the camera stands
    /// (tests: the first view frames the whole city).
    pub fn in_view(&self, p: Vec3) -> bool {
        let vp = self.camera.proj(self.view_aspect).mul(&self.camera.view());
        let c = vp.transform([p.x, p.y, p.z, 1.0]);
        c[3] > 0.0 && (c[0] / c[3]).abs() <= 1.0 && (c[1] / c[3]).abs() <= 1.0
    }

    /// Whether the flash guard veils any part of the picture now, and how
    /// many times it began to (tests, the status line).
    pub fn guard_veiling(&self) -> bool {
        self.render_stats
            .as_ref()
            .and_then(|s| s.lock().ok().map(|s| s.veil))
            .unwrap_or(false)
    }

    pub fn guard_engaged(&self) -> u64 {
        self.render_stats
            .as_ref()
            .and_then(|s| s.lock().ok().map(|s| s.engaged))
            .unwrap_or(0)
    }

    /// Places the camera (tests, screenshots).
    pub fn look_from(&mut self, pos: Vec3, target: Vec3) {
        self.auto_frame = false;
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
        self.auto_frame = false;
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
        self.auto_frame = false;
        if self.time() - self.last_teleport < TELEPORT_GAP {
            // (its turn comes: nothing changes before - not even the
            // choice, whose light would flicker with every request)
            self.queued_target = Some((island, node));
            return;
        }
        self.queued_target = None;
        if node == 0 && self.islands.get(island).is_some_and(|i| i.city.is_some()) {
            // (an island: seen whole - an empty or failed one from far
            // enough to read its state)
            self.select(island, 0);
            let (pos, target) = self.island_view(island);
            let (yaw, pitch) = look_angles(pos, target);
            self.teleport(pos, yaw, pitch);
            if self.camera.mode == Mode::Orbit {
                self.camera.target = target;
                self.camera.distance = (pos - target).length();
            }
            // (kept whole while it grows: a system filling in)
            if let Some(id) = self.islands.get(island).map(|i| i.id) {
                self.keep_framed(island, id);
            }
            return;
        }
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

    /// A teleport, at most one every [`TELEPORT_GAP`] (more wait their
    /// turn: the warp never strobes).
    pub fn teleport(&mut self, pos: Vec3, yaw: f32, pitch: f32) {
        if self.time() - self.last_teleport < TELEPORT_GAP {
            self.queued_teleport = Some((pos, yaw, pitch));
            return;
        }
        // (teleports in quick succession - a storm of them - jump without
        // the warp: its streaks would add to the cuts' changes of light)
        let alone = self.time() - self.last_teleport > 2.0;
        self.last_teleport = self.time();
        self.cut = true;
        let warp = !self.settings.reduce_motion && alone;
        self.camera.teleport(pos, yaw, pitch, warp);
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
        let not_open = matches!(isl.source, IslandSource::Mcp { .. }) && isl.walker.is_none();
        let at = match &isl.source {
            IslandSource::Local { root, .. } => Some(Where::Local(isl.tree.local_path(root, node))),
            // (a system not open yet: nothing to ask its server)
            IslandSource::Mcp { .. } if not_open => None,
            IslandSource::Mcp {
                client,
                backend,
                root,
            } => Some(Where::Remote {
                client: client.clone(),
                backend: backend.id.clone(),
                path: isl.tree.remote_path_bytes(root, node),
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
        // (one preview at a time: the worker takes only the newest request)
        let _ = self.preview_jobs.send(PreviewJob {
            island: id,
            node,
            at,
            name,
            size,
            is_dir,
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
            self.islands.iter().map(|i| Some(&*i.tree)).collect();
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
        for &(i, n) in s.hits.iter().take(PILLARS) {
            let Some(b) = self.node_box(i, n) else {
                continue;
            };
            let c = b.center();
            // tinted by the type of what was found
            let color = self
                .islands
                .get(i)
                .and_then(|isl| isl.city.as_ref())
                .and_then(|c| match c.layout.placed.get(n as usize)? {
                    crate::layout::Placed::Building(k) => {
                        c.layout.buildings.get(*k as usize).map(|b| b.ftype)
                    }
                    crate::layout::Placed::District(k) => {
                        c.layout.districts.get(*k as usize).map(|d| d.ftype)
                    }
                    crate::layout::Placed::Nowhere => None,
                })
                .map_or(0, |t| city::pack_color(self.theme.type_color(t)));
            // (a beam: wider than the thing, never thinner than a few
            // pixels - the shader widens it from afar)
            let w = ((b.max.x - b.min.x).min(b.max.z - b.min.z) * 0.6).clamp(1.5, 14.0);
            out.push(Instance::new(
                city::Shape::Pillar,
                vec3(c.x - w * 0.5, b.max.y, c.z - w * 0.5),
                vec3(w, 160.0 + (b.max.y - b.min.y) * 2.0, w),
                color,
                1.0,
                0,
            ));
            // and a ring on the ground round it
            let r = ((b.max.x - b.min.x).max(b.max.z - b.min.z) * 1.6).clamp(6.0, 60.0);
            out.push(Instance::new(
                city::Shape::Ring,
                vec3(c.x - r * 0.5, b.min.y + 0.03, c.z - r * 0.5),
                vec3(r, 0.02, r),
                color,
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
            let lod = city::Lod {
                raise: self.raise(),
                ..self.lod
            };
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
                let budget = if self.opts.fixed_lod {
                    usize::MAX
                } else {
                    city::FRAME_BUDGET
                };
                city::select_within(
                    &mut cities,
                    eye,
                    &frustum,
                    &lod,
                    budget,
                    &mut self.frame_set,
                );
            }
            // unread folders near: asked for
            let want = std::mem::take(&mut self.frame_set.want);
            for (k, node) in &want {
                if let Some(i) = self.island_of_city(*k) {
                    let isl = &mut self.islands[i];
                    if isl.can_ask()
                        && isl.requested.insert(*node)
                        && let Some(w) = isl.walker.as_ref()
                    {
                        w.list(*node);
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
        let warp = self.camera.warp_amount();
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
            // (the fog thins with the world's size: a city of millions is
            // seen whole from afar, never lost in it)
            fog: [t.fog[0], t.fog[1], t.fog[2], self.fog_density()],
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
            look: self.far_look(),
            lens: [2.0 * (self.camera.fov_y * 0.5).tan(), self.motion, 0.0, 0.0],
        };
        (frustum, g)
    }

    /// The camera's input from egui this frame.
    fn camera_input(&mut self, ui: &Ui, response: &egui::Response) -> Input {
        let mut input = Input::default();
        // (typing in a field: no flight; a button with the keyboard's
        // focus - Tab - takes nothing from it)
        let typing = ui.ctx().text_edit_focused() || self.modal_open();
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

    /// A dialog that takes the keys is open.
    fn modal_open(&self) -> bool {
        self.open_ask.is_some() || self.ask_mode_open
    }

    fn keys(&mut self, ctx: &egui::Context) {
        if self.modal_open() {
            return;
        }
        if ctx.text_edit_focused() {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                ctx.memory_mut(|m| m.surrender_focus(m.focused().unwrap_or(egui::Id::NULL)));
            }
            return;
        }
        // a button or another widget with the keyboard's focus (Tab): the
        // shortcuts still work; Enter and Space are its own, Esc lets go
        let widget_focus = ctx.memory(|m| m.focused());
        if let Some(id) = widget_focus
            && ctx.input(|i| i.key_pressed(Key::Escape))
        {
            ctx.memory_mut(|m| m.surrender_focus(id));
            return;
        }
        let (pressed, mods) = ctx.input(|i| {
            let keys = [
                Key::F,
                Key::T,
                Key::Home,
                Key::Z,
                Key::H,
                Key::F1,
                Key::Slash,
                Key::N,
                Key::O,
                Key::Escape,
                Key::M,
                Key::L,
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
                Key::T => {
                    self.teleport_open = !self.teleport_open;
                    self.focus_filter = self.teleport_open;
                }
                Key::Home => self.home(),
                Key::Z => {
                    // the chosen thing's island (or the one under the pointer)
                    if let Some(i) = self.selected.map(|s| s.0).or(self.hover.map(|h| h.island)) {
                        self.frame_island(i);
                    } else {
                        self.home();
                    }
                }
                Key::H | Key::F1 => self.help_open = !self.help_open,
                Key::Enter if widget_focus.is_some() => {}
                Key::N | Key::Enter => self.next_hit(mods.shift),
                Key::L => self.legend_open = !self.legend_open,
                Key::O => self.open_selected(if mods.shift {
                    OpenHow::DefaultProgram
                } else {
                    OpenHow::Axross
                }),
                Key::M => self.set_capture(ctx, !self.captured),
                Key::Escape => {
                    if self.captured {
                        self.set_capture(ctx, false);
                    } else if self.teleport_open {
                        // (the menu closes first)
                        self.teleport_open = false;
                    } else if self.help_open || self.settings_open || self.sources_open {
                        // (then the other windows, one each time)
                        if self.help_open {
                            self.help_open = false;
                        } else if self.settings_open {
                            self.settings_open = false;
                        } else {
                            self.sources_open = false;
                        }
                    } else if let Some(f) = self.pending_fly.take() {
                        // (the flight that waits - and only the connect it
                        // waits for - given up)
                        let name = self.live.iter().find_map(|(n, l)| {
                            (l.island == Some(f.island) && l.state == SysState::Connecting)
                                .then(|| n.clone())
                        });
                        if let Some(n) = name {
                            self.cancel_system(&n);
                        }
                        self.toast(format!("{}: cancelled", f.what));
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

    /// The mouse is captured (looking around).
    pub fn captured(&self) -> bool {
        self.captured
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
        // (chosen inside a system of axross's: the spot is that system and
        // the path there - jumping to it opens the system again if needed)
        let (system, path_b64, label) = match self.selected.and_then(|(i, node)| {
            let isl = self.islands.get(i)?;
            match &isl.source {
                IslandSource::Mcp { backend, root, .. } => {
                    let name = backend.profile.clone()?;
                    let rel = isl
                        .tree
                        .chain(node)
                        .iter()
                        .skip(1)
                        .map(|c| crate::tree::os_to_bytes(c))
                        .collect::<Vec<_>>()
                        .join(&b'/');
                    let _ = root;
                    Some((name, rel))
                }
                _ => None,
            }
        }) {
            Some((name, rel)) => {
                let shown = format!(
                    "{name}: /{}",
                    crate::tree::display_text(&crate::tree::os_from_bytes(&rel))
                );
                (Some(name), Some(mcp::b64_encode(&rel)), shown)
            }
            None => (None, None, label),
        };
        if let Some(slot) = self.settings.bookmarks.get_mut(n - 1) {
            *slot = Some(Spot {
                pos: self.camera.pos.to_array(),
                yaw: self.camera.yaw,
                pitch: self.camera.pitch,
                label: label.clone(),
                system,
                path_b64,
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
        if let Some(system) = &s.system {
            // inside a system: opened again if needed, then there once its
            // district has been read
            let rel = s
                .path_b64
                .as_deref()
                .and_then(mcp::b64_decode)
                .unwrap_or_default();
            let path: Vec<Vec<u8>> = rel
                .split(|&b| b == b'/')
                .filter(|c| !c.is_empty())
                .map(|c| c.to_vec())
                .collect();
            self.teleport_system(system, path);
            return;
        }
        self.teleport(vec3(s.pos[0], s.pos[1], s.pos[2]), s.yaw, s.pitch);
    }

    /// Looks for axross and asks it (once per program file, remembered in
    /// the settings) whether it opens paths - on a thread of its own.
    fn probe_axross(&mut self) {
        self.axross = Axross::Looking;
        let given = self.opts.axross.clone();
        let known = self.settings.axross_probe.clone();
        let tx = self.bg_tx.clone();
        let wake = self.wake.clone();
        let _ = std::thread::Builder::new()
            .name("afm-axross-probe".into())
            .spawn(move || {
                let (state, probe) = probe_axross(given, known);
                let _ = tx.send(Bg::Axross(state, probe));
                wake();
            });
    }

    /// Whether the chosen thing can be opened at all (and how): None for
    /// the machine's and the made-up islands.
    pub fn can_open(&self, how: OpenHow) -> bool {
        let Some((i, _)) = self.selected else {
            return false;
        };
        match self.islands.get(i).map(|isl| &isl.source) {
            Some(IslandSource::Local { .. }) => true,
            Some(IslandSource::Mcp { .. }) => how == OpenHow::Axross,
            _ => false,
        }
    }

    /// What may happen when the default program opens a file on this
    /// island's medium (None: a local disk).
    fn untrusted_medium(&self, i: usize) -> Option<&'static str> {
        match &self.islands.get(i)?.source {
            IslandSource::Local { mount: Some(m), .. } => match m.kind {
                MountKind::Network => Some("a network file system"),
                MountKind::Removable => Some("a removable medium"),
                _ => None,
            },
            _ => None,
        }
    }

    /// Opens the chosen thing: in axross (when it can open paths - said
    /// plainly when it cannot), with the default program (a question
    /// first for anything that may run), or in the file manager.
    pub fn open_selected(&mut self, how: OpenHow) {
        let Some((i, node)) = self.selected else {
            return;
        };
        let Some(isl) = self.islands.get(i) else {
            return;
        };
        let local = match &isl.source {
            IslandSource::Local { root, .. } => Some(isl.tree.local_path(root, node)),
            _ => None,
        };
        if !self.can_open(how) {
            self.toast("This is not a file on a disk: there is nothing to open".into());
            return;
        }
        match how {
            OpenHow::Axross => {
                let ax = match &self.axross {
                    Axross::Ready(p) => p.clone(),
                    other => {
                        let why = match other {
                            Axross::Looking => "axross is still being looked for".to_string(),
                            Axross::Missing => {
                                "axross is not installed (not on the PATH)".to_string()
                            }
                            Axross::TooOld(p) => format!(
                                "the axross at {} is too old to open a path (that needs axross 0.15.186 or later)",
                                crate::tree::display_text(p.as_os_str())
                            ),
                            Axross::Ready(_) => unreachable!(),
                        };
                        self.open_ask = Some(OpenAsk::NoAxross { why, path: local });
                        return;
                    }
                };
                let launch = match &isl.source {
                    IslandSource::Local { .. } => match &local {
                        Some(p) => Ok(open::axross_local(&ax, p)),
                        None => return,
                    },
                    IslandSource::Mcp { backend, root, .. } => match &backend.profile {
                        Some(profile) => open::axross_remote(
                            &ax,
                            profile,
                            &isl.tree.remote_path_bytes(root, node),
                        ),
                        None => Err(format!(
                            "{} has no saved profile: axross cannot open it by name",
                            isl.title
                        )),
                    },
                    _ => return,
                };
                match launch {
                    Ok(l) => self.launch(l),
                    Err(e) => self.toast(e),
                }
            }
            OpenHow::FileManager => match local
                .as_deref()
                .and_then(|p| open::file_manager_via(self.opts.opener.as_deref(), p))
            {
                Some(l) => self.launch(l),
                None => self.toast("No file manager was found".into()),
            },
            OpenHow::DefaultProgram => {
                let Some(path) = local else { return };
                // (the file's mode is read on a thread: never on the window's)
                let untrusted = self.untrusted_medium(i);
                let trusted = self.settings.open_types.clone();
                let tx = self.bg_tx.clone();
                let wake = self.wake.clone();
                let delay = self.opts.open_check_delay;
                let _ = std::thread::Builder::new()
                    .name("afm-open-check".into())
                    .spawn(move || {
                        if let Some(d) = delay {
                            // (the tests' stand-in for a disk that does not answer)
                            std::thread::sleep(d);
                        }
                        let id = open::identity(&path);
                        let risk = open::risk_of(&path, untrusted, &trusted);
                        let _ = tx.send(Bg::OpenChecked(Box::new(OpenCheck { path, risk, id })));
                        wake();
                    });
                self.open_waiting = Some((Instant::now(), false));
            }
        }
    }

    /// The default program's check came back: opened, or asked first.
    fn open_checked(&mut self, check: OpenCheck) {
        self.open_waiting = None;
        match check.risk {
            None => self.open_default(&check.path, check.id),
            Some(risk) => {
                self.open_ask = Some(OpenAsk::Risky {
                    path: check.path,
                    why: risk.why,
                    unknown: risk.unknown,
                    id: check.id,
                })
            }
        }
    }

    /// Opens with the default program - once the file is looked at again
    /// (on a thread) and is still what was checked: the same file, not a
    /// link put in its place since.
    fn open_default(&mut self, path: &std::path::Path, id: Option<open::Identity>) {
        let path = path.to_path_buf();
        let tx = self.bg_tx.clone();
        let wake = self.wake.clone();
        let _ = std::thread::Builder::new()
            .name("afm-open-recheck".into())
            .spawn(move || {
                let same = id.is_some() && open::identity(&path) == id;
                let _ = tx.send(Bg::OpenRechecked(path, same));
                wake();
            });
    }

    fn open_rechecked(&mut self, path: PathBuf, same: bool) {
        if !same {
            self.toast(format!(
                "{} changed since it was checked (another file or a link in its place): not opened",
                crate::tree::display_text(path.as_os_str())
            ));
            return;
        }
        match open::system_via(self.opts.opener.as_deref(), &path) {
            Some(l) => self.launch(l),
            None => self.toast("No program to open it with was found".into()),
        }
    }

    /// Starts a program (the tests: writes it down instead).
    fn launch(&mut self, l: Launch) {
        if let Some(log) = &self.opts.launch_log {
            if let Ok(mut v) = log.lock() {
                v.push(l.clone());
            }
            self.toast(format!("Opened with {}", l.label));
            return;
        }
        let tx = self.bg_tx.clone();
        let wake = self.wake.clone();
        let how = l.label.clone();
        open::spawn(&l, move |r| {
            let _ = tx.send(Bg::Opened(r.map(|()| how)));
            wake();
        });
    }

    /// The question before opening, as a dialog.
    fn open_ask_window(&mut self, ctx: &egui::Context) {
        let Some(ask) = self.open_ask.clone() else {
            return;
        };
        let t = self.theme;
        let mut answer: Option<Option<OpenHow>> = None;
        let mut risky_path: Option<(PathBuf, Option<open::Identity>)> = None;
        let mut trust: Option<String> = None;
        let mut unlock: Option<(String, bool)> = None;
        let mut swap: Option<(String, String)> = None;
        let manager = match &self.axross {
            Axross::Ready(p) if self.axross_manager => Some(p.clone()),
            _ => None,
        };
        let modal = egui::Modal::new(egui::Id::unique("afm-open-ask")).show(ctx, |ui| {
            ui.set_max_width(460.0);
            match &ask {
                OpenAsk::Risky {
                    path,
                    why,
                    unknown,
                    id,
                } => {
                    ui.label(
                        RichText::new("Open this with the default program?")
                            .strong()
                            .size(16.0),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(crate::tree::display_text(path.as_os_str()))
                            .monospace()
                            .color(t.strong),
                    );
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("Careful: {why}.")).color(t.warn));
                    ui.label(
                        RichText::new("Showing it in the file manager runs nothing.")
                            .color(t.muted),
                    );
                    if let Some(ext) = unknown {
                        // (only for a type nobody listed - never for one that runs)
                        ui.checkbox(
                            &mut self.trust_type,
                            format!("Do not ask again for .{ext} files (Settings can undo it)"),
                        );
                    }
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        let safe = ui.button(RichText::new("Show it in the file manager").strong());
                        if ui.memory(|m| m.focused().is_none()) {
                            safe.request_focus();
                        }
                        if safe.clicked() {
                            answer = Some(Some(OpenHow::FileManager));
                        }
                        if ui.button("Open it anyway").clicked() {
                            answer = Some(Some(OpenHow::DefaultProgram));
                            risky_path = Some((path.clone(), *id));
                            if self.trust_type
                                && let Some(ext) = unknown
                            {
                                trust = Some(ext.clone());
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            answer = Some(None);
                        }
                    });
                }
                OpenAsk::Unlock { name } => {
                    ui.label(RichText::new(format!("{name} needs unlocking in axross")).strong().size(16.0));
                    ui.add_space(4.0);
                    ui.label(
                        "It needs a password or a key passphrase that axross has not stored. \
                         This program never asks for a secret and never sends one: store it in \
                         axross's connection manager (Store password), then try again.",
                    );
                    if manager.is_none() {
                        ui.label(
                            RichText::new("(This axross cannot open its connection manager from here: open it in axross.)")
                                .color(t.muted),
                        );
                    }
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(manager.is_some(), egui::Button::new("Open axross's connection manager"))
                            .clicked()
                        {
                            unlock = Some((name.clone(), true));
                        }
                        if ui.button("Try again").clicked() {
                            unlock = Some((name.clone(), false));
                        }
                        if ui.button("Cancel").clicked() {
                            answer = Some(None);
                        }
                    });
                }
                OpenAsk::TooMany { name, open } => {
                    ui.label(RichText::new(format!("{name} could not be opened")).strong().size(16.0));
                    ui.add_space(4.0);
                    ui.label(format!(
                        "axross's server keeps at most {} systems open at once. Open now: {}.",
                        open.len().max(1),
                        if open.is_empty() { "-".to_string() } else { open.join(", ") }
                    ));
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        if let Some(oldest) = open.first()
                            && ui
                                .button(format!("Close {oldest} (visited longest ago) and open {name}"))
                                .clicked()
                        {
                            swap = Some((oldest.clone(), name.clone()));
                        }
                        if ui.button("Cancel").clicked() {
                            answer = Some(None);
                        }
                    });
                }
                OpenAsk::NoAxross { why, path } => {
                    ui.label(RichText::new("axross cannot open this").strong().size(16.0));
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("{why}.")).color(t.warn));
                    if path.is_some() {
                        ui.label(
                            RichText::new(
                                "The default program or the file manager can open it instead.",
                            )
                            .color(t.muted),
                        );
                    }
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        if path.is_some() {
                            if ui.button("Show it in the file manager").clicked() {
                                answer = Some(Some(OpenHow::FileManager));
                            }
                            if ui.button("Open with the default program").clicked() {
                                answer = Some(Some(OpenHow::DefaultProgram));
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            answer = Some(None);
                        }
                    });
                }
            }
        });
        if let Some((old, new)) = swap {
            self.open_ask = None;
            self.close_system(&old);
            if let Some(l) = self.live.get_mut(&new) {
                l.state = SysState::NotOpen;
            }
            self.teleport_system(&new, Vec::new());
            return;
        }
        if let Some((name, manager_wanted)) = unlock {
            self.open_ask = None;
            if manager_wanted {
                if let Some(ax) = manager.filter(|_| open::valid_profile(&name)) {
                    let mut arg = std::ffi::OsString::from("--connection-manager=");
                    arg.push(&name);
                    self.launch(Launch {
                        program: ax,
                        args: vec![arg],
                        label: "axross's connection manager".into(),
                    });
                }
            } else {
                // (listed again: a secret stored meanwhile counts)
                self.retry_after_refresh = Some(name);
                self.refresh_systems();
            }
            return;
        }
        // (Esc closes it; a click beside it does not - the click that
        // opened it must not close it again)
        let _ = modal;
        if answer.is_none() && ctx.input(|i| i.key_pressed(Key::Escape)) {
            answer = Some(None);
        }
        let Some(answer) = answer else { return };
        self.open_ask = None;
        self.trust_type = false;
        if let Some(ext) = trust
            && !self.settings.open_types.contains(&ext)
        {
            self.settings.open_types.push(ext);
            self.settings = std::mem::take(&mut self.settings).sanitized();
            let _ = settings::save(&self.settings_path, &self.settings);
        }
        match (answer, risky_path) {
            // (asked and answered: no second question)
            (Some(OpenHow::DefaultProgram), Some((p, id))) => self.open_default(&p, id),
            (Some(how), _) => self.open_selected(how),
            (None, _) => {}
        }
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
        if input.move_dir != [0.0; 3]
            || input.look != [0.0; 2]
            || input.orbit != [0.0; 2]
            || input.pan != [0.0; 2]
            || input.scroll != 0.0
        {
            // (moved by hand: the first view is left alone from now on)
            self.auto_frame = false;
        }
        {
            let cities = self.cities();
            let solid = |p: Vec3| city::solid(&cities, p);
            let mut cam = self.camera.clone();
            let before = cam.pos;
            cam.update(dt, &input, &solid);
            // (the keys always do something - or say why not)
            let pressed = input.move_dir != [0.0; 3] && cam.mode == Mode::Fly && !cam.flying();
            if pressed && (cam.pos - before).length() < 1e-4 {
                self.stuck_for += dt;
                if self.stuck_for > 0.4 && !self.stuck_said {
                    self.stuck_said = true;
                    self.toast(
                        "A wall or the ground is in the way: turn, rise with E, or Home".into(),
                    );
                }
            } else {
                self.stuck_for = 0.0;
                if !pressed {
                    self.stuck_said = false;
                }
            }
            self.camera = cam;
        }
        let clearance = {
            // (how near the nearest roofs and walls are: a flight along a
            // wall sweeps the picture as fast as one low over the ground)
            let cities = self.cities();
            let p = self.camera.pos;
            let mut roof = 0f32;
            for d in [
                vec3(15.0, 0.0, 0.0),
                vec3(-15.0, 0.0, 0.0),
                vec3(0.0, 0.0, 15.0),
                vec3(0.0, 0.0, -15.0),
            ] {
                roof = roof.max(city::ceiling(&cities, p, p + d));
            }
            if roof >= p.y {
                3.0
            } else {
                (p.y - roof).max(3.0)
            }
        };
        self.motion = view_motion(
            self.motion,
            self.last_cam,
            &self.camera,
            clearance,
            dt,
            self.cut,
        );
        self.last_cam = Some((self.camera.pos, self.camera.yaw, self.camera.pitch));
        if let Some((p, y, pi)) = self.queued_teleport
            && self.time() - self.last_teleport >= TELEPORT_GAP
        {
            self.queued_teleport = None;
            self.teleport(p, y, pi);
        }
        if let Some((i, n)) = self.queued_target
            && self.time() - self.last_teleport >= TELEPORT_GAP
        {
            self.teleport_to(i, n);
        }
        let aspect = rect.width() / rect.height().max(1.0);
        if (aspect - self.view_aspect).abs() > 0.02 && self.auto_frame {
            // (a window of another shape: framed again for it)
            self.framed = None;
        }
        self.view_aspect = aspect;
        // hover and click
        let pointer = response.hover_pos().filter(|_| !self.captured);
        let t_pick = Instant::now();
        // (the same pointer, view and world as the frame before: the same
        // answer - no ray cast while nothing moves)
        let pick_key = pointer.map(|p| {
            (
                [
                    p.x,
                    p.y,
                    self.camera.pos.x,
                    self.camera.pos.y,
                    self.camera.pos.z,
                    self.camera.yaw,
                    self.camera.pitch,
                    aspect,
                ],
                self.world_version,
            )
        });
        let cached = match (&self.last_pick, pick_key) {
            (Some((k, h)), Some(now)) if *k == now => Some(*h),
            _ => None,
        };
        self.hover = match cached {
            Some(h) => h,
            None => pointer.and_then(|p| {
                let ndc = [
                    (p.x - rect.left()) / rect.width() * 2.0 - 1.0,
                    1.0 - (p.y - rect.top()) / rect.height() * 2.0,
                ];
                let ray = self.camera.ray(ndc, aspect);
                let cities = self.cities();
                city::pick(&cities, &ray, &self.raise()).map(|h| Hit {
                    island: self.island_of_city(h.island).unwrap_or(0),
                    ..h
                })
            }),
        };
        self.last_pick = pick_key.map(|k| (k, self.hover));
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
        self.parts.pick_ms = t_pick.elapsed().as_secs_f32() * 1000.0;
        let t_lod = Instant::now();
        let (_, globals) = self.update_frame_set(aspect);
        self.parts.lod_ms = t_lod.elapsed().as_secs_f32() * 1000.0;
        let frame = Arc::new(FrameData {
            globals,
            instances: self.instances.clone(),
            opaque: self.opaque,
            version: self.version,
            guard: self.opts.flash_guard,
            dt,
            msaa: self.settings.msaa,
            cut: std::mem::take(&mut self.cut),
        });
        if self.has_gpu {
            ui.painter().add(render::callback(rect, frame));
        } else {
            ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
        }
        let t_labels = Instant::now();
        self.labels(ui, rect, aspect);
        self.parts.labels_ms = t_labels.elapsed().as_secs_f32() * 1000.0;
        if self.legend_open {
            self.legend(ui, rect);
        }
        if self.camera.mode == Mode::Fly {
            // a small crosshair
            let c = rect.center();
            let col = self.theme.muted.gamma_multiply(0.7);
            ui.painter()
                .line_segment([c - vec2(6.0, 0.0), c + vec2(6.0, 0.0)], (1.0, col));
            ui.painter()
                .line_segment([c - vec2(0.0, 6.0), c + vec2(0.0, 6.0)], (1.0, col));
        }
        self.key_hint(ui, rect, &input);
        if input.scroll != 0.0 && self.camera.mode == Mode::Fly {
            self.speed_said = Some(Instant::now());
        }
        if let Some(at) = self.speed_said {
            if at.elapsed() < Duration::from_millis(1500) {
                // (the wheel's speed, said where it is looked at - not only
                // in the status line)
                let text = format!(
                    "speed ×{:.1} · {:.0} m/s here",
                    self.camera.speed / 25.0,
                    self.camera.flight_speed()
                );
                let painter = ui.painter_at(rect);
                let galley = painter.layout_no_wrap(
                    text,
                    egui::FontId::proportional(15.0),
                    self.theme.strong,
                );
                let r = Rect::from_center_size(
                    Pos2::new(rect.center().x, rect.top() + 40.0),
                    galley.size() + vec2(16.0, 8.0),
                );
                painter.rect_filled(r, 4.0, self.theme.surface.gamma_multiply(0.9));
                painter.galley(r.min + vec2(8.0, 4.0), galley, self.theme.strong);
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            } else {
                self.speed_said = None;
            }
        }
        self.empty_state(ui, rect);
        if let Some(h) = self.hover
            && !response.dragged()
            && let Some(p) = pointer
        {
            let text = self.node_title(h.island, h.node);
            let font = egui::FontId::monospace(12.0);
            let galley = ui.painter().layout_no_wrap(text, font, self.theme.strong);
            let size = galley.size() + vec2(8.0, 4.0);
            let at = hover_tip_at(p, size, rect, &self.title_rects);
            let bg = Rect::from_min_size(at - vec2(4.0, 2.0), size);
            ui.painter()
                .rect_filled(bg, 3.0, self.theme.surface.gamma_multiply(0.92));
            ui.painter().galley(at, galley, self.theme.strong);
        }
        let veiling = self
            .render_stats
            .as_ref()
            .and_then(|s| s.lock().ok().map(|s| s.veil))
            .unwrap_or(false);
        let animating = self.camera.flying()
            || self.camera.warp > 0.0
            || input.move_dir != [0.0; 3]
            || input.scroll != 0.0
            || response.dragged()
            || self.queued_teleport.is_some()
            || self.queued_target.is_some()
            || veiling
            || self.search.as_ref().is_some_and(|s| !s.done);
        let s = &self.settings;
        match repaint_policy(
            animating,
            self.hidden,
            self.focused,
            s.rain && !s.reduce_motion,
            s.idle_fps,
        ) {
            Repaint::Now => ui.ctx().request_repaint(),
            Repaint::After(d) => ui.ctx().request_repaint_after(d),
            // (nothing moves: the next input, walk or answer draws)
            Repaint::Idle => {}
        }
        if let Some((_, at)) = &self.toast {
            let left = Duration::from_secs(6).saturating_sub(at.elapsed());
            if !left.is_zero() {
                ui.ctx().request_repaint_after(left);
            }
        }
    }

    /// The colour legend, in the view's lower left corner (L hides it).
    fn legend(&self, ui: &Ui, rect: Rect) {
        let t = self.theme;
        let painter = ui.painter_at(rect);
        let font = egui::FontId::proportional(11.5);
        // (the machine's island close by: its own legend - the file types'
        // colours mean nothing there)
        let hardware = self.hardware_in_view();
        let rows: Vec<(Color32, &str)> = if hardware {
            HARDWARE_LEGEND
                .iter()
                .map(|(f, name)| (t.type_color(*f), *name))
                .collect()
        } else {
            crate::tree::FILE_TYPES
                .iter()
                .skip(1)
                .map(|f| (t.type_color(*f), f.label()))
                .collect()
        };
        let extra = if hardware {
            [
                (t.accent, "bright: busy (load, use)"),
                (t.muted, "dim: idle or empty"),
                (t.strong, "beam: a disk's mountpoint"),
            ]
        } else {
            [
                (t.accent, "bright: changed lately"),
                (t.muted, "dashed, dim: not read yet"),
                (Color32::from_rgb(230, 153, 31), "amber edge: no access"),
            ]
        };
        let Some((bg, cols)) = legend_geometry(rect) else {
            return; // (no room)
        };
        let per_col = rows.len().div_ceil(cols);
        let (line, col_w) = (LEGEND_LINE, LEGEND_COL);
        let at = bg.min;
        painter.rect_filled(bg, 3.0, t.ground.gamma_multiply(0.85));
        painter.rect_stroke(bg, 3.0, (1.0, t.border), egui::StrokeKind::Inside);
        painter.text(
            at + vec2(8.0, 5.0),
            egui::Align2::LEFT_TOP,
            if hardware {
                "this machine · L hides"
            } else {
                "height: size · L hides"
            },
            font.clone(),
            t.muted,
        );
        for (k, (c, name)) in rows.iter().enumerate() {
            let (col, row) = (k / per_col, k % per_col);
            let p = at + vec2(8.0 + col as f32 * col_w, 22.0 + row as f32 * line);
            painter.rect_filled(
                Rect::from_min_size(p + vec2(0.0, 2.0), vec2(10.0, 10.0)),
                1.0,
                *c,
            );
            painter.text(
                p + vec2(15.0, 0.0),
                egui::Align2::LEFT_TOP,
                *name,
                font.clone(),
                t.text,
            );
        }
        for (k, (c, name)) in extra.iter().enumerate() {
            let p = at + vec2(8.0, 22.0 + (per_col + k) as f32 * line);
            painter.rect_stroke(
                Rect::from_min_size(p + vec2(0.0, 2.0), vec2(10.0, 10.0)),
                1.0,
                (1.5, *c),
                egui::StrokeKind::Inside,
            );
            painter.text(
                p + vec2(15.0, 0.0),
                egui::Align2::LEFT_TOP,
                *name,
                font.clone(),
                t.text,
            );
        }
    }

    /// Whether the machine's island is what the camera looks at: near
    /// (within two and a half of its widths) and in the view's middle.
    pub fn hardware_in_view(&self) -> bool {
        let Some(c) = self
            .islands
            .iter()
            .find(|i| i.hw.is_some())
            .and_then(|i| i.city.as_ref())
        else {
            return false;
        };
        let centre = c.center();
        let to = centre - self.camera.pos;
        let near = to.length() < c.side() * 2.5 + 40.0;
        let ahead = to.normalized().dot(self.camera.forward()) > 0.8;
        near && ahead
    }

    fn legend_rect(&self, rect: Rect) -> Option<Rect> {
        self.legend_open
            .then(|| legend_geometry(rect).map(|g| g.0))
            .flatten()
    }

    /// An island's title with what it is doing: connecting, walking,
    /// failed and why, sizes the backend does not tell, read as approached.
    pub fn island_caption(&self, i: usize) -> String {
        let Some(isl) = self.islands.get(i) else {
            return String::new();
        };
        let mut s = isl.title.clone();
        let system = match &isl.source {
            IslandSource::Mcp { backend, .. } => backend.profile.clone(),
            _ => None,
        };
        if let Some(name) = &system
            && self
                .live
                .get(name)
                .is_some_and(|l| l.state == SysState::Connecting)
        {
            s.push_str("  · connecting…");
            return s;
        }
        if isl.from_cache.is_some() && isl.status.error.is_some() {
            s.push_str("  · cached (cannot be read now)");
        } else if let Some(e) = &isl.status.error {
            let short: String = e.chars().take(70).collect();
            s.push_str(&format!("  · failed: {short}"));
        } else if isl.status.done && isl.tree.len() <= 1 && isl.hw.is_none() {
            s.push_str("  · empty: nothing in it");
        } else if isl.from_cache.is_some() {
            s.push_str("  · cached, walking");
        } else if !isl.status.done && isl.hw.is_none() && (system.is_none() || isl.walker.is_some())
        {
            s.push_str(&format!("  · {} …", isl.status.entries));
        }
        let files = isl.tree.root().count;
        if isl.status.unknown_sizes > 0 && isl.status.unknown_sizes * 2 >= u64::from(files.max(1)) {
            s.push_str("  · sizes unknown: equal footprints");
        }
        if isl.status.lazy {
            s.push_str("  · read as approached");
        }
        s
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
            Kind::Dir => format!(
                "{name}/  {} in {}",
                hw::human_bytes(n.total),
                count(u64::from(n.count), "file", "files")
            ),
            _ => format!("{name}  {}", hw::human_bytes(n.size)),
        }
    }

    fn labels(&mut self, ui: &Ui, rect: Rect, aspect: f32) {
        self.title_rects.clear();
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
        let mut titles: Vec<Rect> = Vec::new();
        // (one label per text and island: a name the layout offers twice -
        // a district and its title - is drawn once)
        let mut drawn: std::collections::HashSet<(usize, String)> =
            std::collections::HashSet::new();
        if let Some(l) = self.legend_rect(rect) {
            taken.push(l);
        }
        let painter = ui.painter_at(rect);
        let t = self.theme;
        let cities = self.cities();
        let eye = self.camera.pos;
        // the chosen one first, whatever is in front of it
        if let (Some((i, n)), Some(b)) = (self.selected, self.sel_box)
            && let Some(isl) = self.islands.get(i)
            && let Some(node) = isl.tree.get(n)
            // (an island chosen: its title is its label, drawn once)
            && n != 0
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
            let p = vp.transform([c.at.x, c.at.y, c.at.z, 1.0]);
            if p[3] <= 0.1 {
                continue;
            }
            let (x, y) = (p[0] / p[3], p[1] / p[3]);
            if !(-1.1..=1.1).contains(&x) || !(-1.1..=1.1).contains(&y) {
                continue;
            }
            // (behind a building: not shown - the islands' titles always
            // are; a ray only for a candidate in the view)
            if c.kind != LabelKind::Island && city::occluded(&cities, eye, c.at) {
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
                    // (its state on a second line: a long failure's reason
                    // wraps inside the window, never off its edge)
                    let s = title_text(&self.island_caption(island));
                    let color = match &isl.source {
                        IslandSource::Mcp { backend, .. } => backend
                            .protocol
                            .as_deref()
                            .map(|p| {
                                let [r, g, b] = crate::systems::Family::of(p).color();
                                Color32::from_rgb(r, g, b)
                            })
                            .unwrap_or(t.strong),
                        _ => t.strong,
                    };
                    // (an empty or failed island: its state written large)
                    let big = isl.tree.len() <= 1 || isl.status.error.is_some();
                    (s, if big { 21.0 } else { 15.0 }, color)
                }
                LabelKind::District => {
                    let Some(n) = isl.tree.get(c.node) else {
                        continue;
                    };
                    let slash = if isl.hw.is_some() { "" } else { "/" };
                    (
                        format!("{}{slash}", display_name(&n.name, 32)),
                        12.5,
                        if isl.hw.is_some() { t.strong } else { t.label },
                    )
                }
                LabelKind::Building => {
                    let Some(n) = isl.tree.get(c.node) else {
                        continue;
                    };
                    // (the machine's parts: brighter, they are few)
                    if isl.hw.is_some() {
                        (display_name(&n.name, 28), 12.0, t.label)
                    } else {
                        (display_name(&n.name, 28), 11.0, t.label_dim)
                    }
                }
            };
            let font = if c.kind == LabelKind::Island {
                egui::FontId::proportional(size)
            } else {
                egui::FontId::monospace(size)
            };
            // (one label per text and island; the machine's and the
            // islands' titles once in the world - an island laid out again
            // stands twice for a moment while it settles)
            let scope = if c.kind == LabelKind::Island || isl.hw.is_some() {
                usize::MAX
            } else {
                island
            };
            let key = (scope, text.clone());
            if drawn.contains(&key) {
                continue;
            }
            let galley = if c.kind == LabelKind::Island {
                painter.layout(text, font, color, (rect.width() - 40.0).clamp(120.0, 620.0))
            } else {
                painter.layout_no_wrap(text, font, color)
            };
            let mut r = Rect::from_center_size(
                at - vec2(0.0, galley.size().y * 0.5 + 2.0),
                galley.size() + vec2(6.0, 2.0),
            );
            if c.kind == LabelKind::Island {
                // (a title near the window's edge is moved in, not cut off)
                let inner = rect.shrink(10.0);
                let dx = (inner.left() - r.left()).max(0.0) + (inner.right() - r.right()).min(0.0);
                let dy = (inner.top() - r.top()).max(0.0) + (inner.bottom() - r.bottom()).min(0.0);
                r = r.translate(vec2(dx, dy));
            }
            // (what a label covers: an island title's box, a little air
            // round every label; none over the legend)
            let cover = if c.kind == LabelKind::Island {
                r.expand(5.0)
            } else {
                r.expand(2.0)
            };
            if taken.iter().any(|o| o.intersects(cover)) || !rect.contains_rect(cover) {
                continue;
            }

            taken.push(cover);
            drawn.insert(key);
            if c.kind == LabelKind::Island {
                titles.push(cover);
                let chosen = self.selected == Some((island, 0));
                painter.rect_filled(
                    r.expand(3.0),
                    2.0,
                    Color32::from_rgba_unmultiplied(0, 12, 4, 190),
                );
                painter.rect_stroke(
                    r.expand(3.0),
                    2.0,
                    if chosen {
                        (1.5, t.accent)
                    } else {
                        (1.0, t.border)
                    },
                    egui::StrokeKind::Outside,
                );
            } else {
                // (a dark shadow: readable over lit towers)
                painter.galley(r.min + vec2(4.0, 2.0), galley.clone(), Color32::BLACK);
            }
            painter.galley(r.min + vec2(3.0, 1.0), galley, color);
        }
        drop(cities);
        // (the plots' protocol signs after the titles: kept clear of them)
        self.plot_tags(&painter, rect, &vp, &titles);
        self.title_rects = titles;
    }

    /// No folder at all (no mounts found, none given, all removed): what to
    /// do, in the middle of the view - never an empty grid alone.
    fn empty_state(&mut self, ui: &Ui, rect: Rect) {
        let sources = self.islands.iter().filter(|i| i.hw.is_none()).count();
        let connecting = matches!(self.mcp, McpState::Looking);
        if sources > 0 || self.starting || connecting {
            return;
        }
        let t = self.theme;
        let mut open_sources = false;
        egui::Area::new(egui::Id::unique("afm-empty"))
            .order(egui::Order::Middle)
            .pivot(egui::Align2::CENTER_CENTER)
            .fixed_pos(rect.center() - vec2(0.0, rect.height() * 0.12))
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(t.surface.gamma_multiply(0.94))
                    .stroke((1.0, t.border))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(16, 12))
                    .show(ui, |ui| {
                        ui.set_max_width(420.0);
                        ui.label(RichText::new("No folders to show yet").strong().size(17.0).color(t.strong));
                        ui.add_space(4.0);
                        ui.label(
                            "Add a folder in Sources, or start the program with one:                              axross-files-matrix /path/to/folder. Saved connections of                              axross open from Teleport (T).",
                        );
                        ui.add_space(6.0);
                        if ui.button("Sources…").clicked() {
                            open_sources = true;
                        }
                    });
            });
        if open_sources {
            self.sources_open = true;
        }
    }

    /// The first start's key hint, low in the view, until the first key
    /// that moves the camera or its close button (the Help window - H or
    /// "?" - keeps it).
    fn key_hint(&mut self, ui: &Ui, rect: Rect, input: &Input) {
        if self.settings.hint_seen {
            return;
        }
        // (gone with the first key that moves, and for a mouse user with the
        // first drag or wheel turn; T, / and Home too)
        let used = input.move_dir != [0.0; 3]
            || input.look != [0.0; 2]
            || input.orbit != [0.0; 2]
            || input.pan != [0.0; 2]
            || input.scroll != 0.0
            || self.help_open
            || self.teleport_open
            || self.camera.flying()
            || ui.ctx().text_edit_focused();
        if used {
            self.settings.hint_seen = true;
            let _ = settings::save(&self.settings_path, &self.settings);
            return;
        }
        let t = self.theme;
        let at = Pos2::new(rect.center().x, rect.bottom() - 44.0);
        let mut close = false;
        egui::Area::new(egui::Id::unique("afm-key-hint"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::CENTER_BOTTOM)
            .fixed_pos(at)
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(t.surface.gamma_multiply(0.92))
                    .stroke((1.0, t.accent))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(KEY_HINT).color(t.strong));
                            if ui
                                .small_button("×")
                                .on_hover_text("Hide this hint (H shows the keys)")
                                .clicked()
                            {
                                close = true;
                            }
                        });
                    });
            });
        if close {
            self.settings.hint_seen = true;
            let _ = settings::save(&self.settings_path, &self.settings);
        }
    }

    /// A system's island with nothing on it yet (connecting, failed,
    /// empty): its protocol written large on its plot, in its family's
    /// colour.
    fn plot_tags(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        vp: &crate::math::Mat4,
        titles: &[Rect],
    ) {
        for isl in &self.islands {
            let (IslandSource::Mcp { backend, .. }, Some(c)) = (&isl.source, isl.city.as_ref())
            else {
                continue;
            };
            if isl.tree.len() > 1 || !c.shown {
                continue;
            }
            let proto = backend.protocol.as_deref().unwrap_or("?");
            let fam = crate::systems::Family::of(proto);
            let [r, g, b] = fam.color();
            let at = c.center();
            let p = vp.transform([at.x, 0.5, at.z, 1.0]);
            if p[3] <= 0.1 {
                continue;
            }
            let (x, y) = (p[0] / p[3], p[1] / p[3]);
            if !(-0.9..=0.9).contains(&x) || !(-0.9..=0.9).contains(&y) {
                continue;
            }
            let pos = Pos2::new(
                rect.left() + (x * 0.5 + 0.5) * rect.width(),
                rect.top() + (0.5 - y * 0.5) * rect.height(),
            );
            // (as large as the plot looks, within reason)
            let size = (c.side() / p[3] * rect.height() * 0.25).clamp(18.0, 64.0);
            let text: String = proto.to_uppercase().chars().take(10).collect();
            let galley = painter.layout(
                format!("{text}\n{}", fam.label()),
                egui::FontId::monospace(size * 0.55),
                Color32::from_rgba_unmultiplied(r, g, b, 150),
                rect.width() * 0.5,
            );
            let mut at = Rect::from_center_size(pos, galley.size());
            // (a title over it: the sign moves above the title)
            for t in titles {
                if t.intersects(at) {
                    at = at.translate(vec2(0.0, t.top() - at.bottom() - 6.0));
                }
            }
            painter.galley(at.min, galley, Color32::WHITE);
        }
    }

    /// The bar above: below about 1000 points the buttons go into a menu,
    /// below 720 the title goes too; the READ-ONLY badge always stays.
    fn top_bar(&mut self, ui: &mut Ui) {
        let t = self.theme;
        let width = ui.available_width();
        let narrow = width < 1000.0;
        let tiny = width < 720.0;
        let mut toggle_teleport = false;
        let mut toggle_sources = false;
        let mut toggle_settings = false;
        let mut toggle_help = false;
        let mut overview = false;
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // the write mode: visibly off (file operations come later)
                let badge = RichText::new(" READ-ONLY ").monospace().strong().color(t.ground).background_color(t.accent);
                ui.label(badge).on_hover_text("Nothing is changed on any disk. File operations (copy, move, rename, delete) come in a later release.");
                if !narrow {
                    let mut write = false;
                    ui.add_enabled(false, egui::Checkbox::new(&mut write, "write mode")).on_disabled_hover_text("Not available yet: this release only reads.");
                    ui.separator();
                    if ui.button("?").on_hover_text("Keys and mouse (H)").clicked() {
                        toggle_help = true;
                    }
                    if ui.button("Settings").clicked() {
                        toggle_settings = true;
                    }
                    if ui.button("Sources").clicked() {
                        toggle_sources = true;
                    }
                } else {
                    ui.menu_button("Menu", |ui| {
                        if ui.button("Overview (Home)").clicked() {
                            overview = true;
                            ui.close();
                        }
                        if ui.button("Teleport (T)").clicked() {
                            toggle_teleport = true;
                            ui.close();
                        }
                        if ui.button("Sources").clicked() {
                            toggle_sources = true;
                            ui.close();
                        }
                        if ui.button("Settings").clicked() {
                            toggle_settings = true;
                            ui.close();
                        }
                        if ui.button("Colour legend (L)").clicked() {
                            self.legend_open = !self.legend_open;
                            ui.close();
                        }
                        if ui.button("Keys and mouse (H)").clicked() {
                            toggle_help = true;
                            ui.close();
                        }
                    });
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    if !tiny {
                        ui.label(RichText::new("axross files matrix").monospace().strong().color(t.accent));
                        ui.separator();
                    }
                    let field = (ui.available_width() * 0.4).clamp(110.0, 240.0);
                    let search = ui.add(
                        egui::TextEdit::singleline(&mut self.search_text)
                            .hint_text(if tiny { "search ( / )" } else { "search names or *.glob  ( / )" })
                            .desired_width(field),
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
                        let n = s.hits.len();
                        let tip = if n > PILLARS {
                            format!("Pillars of light stand over the first {PILLARS}; N and the teleport window go through all {n}")
                        } else {
                            "N flies to the next one".to_string()
                        };
                        ui.label(RichText::new(format!("{}{state}", count(n as u64, "hit", "hits"))).color(t.muted)).on_hover_text(tip);
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
                    if !narrow {
                        if ui.button("Overview").on_hover_text("The whole world (Home)").clicked() {
                            overview = true;
                        }
                        if ui.button("Teleport").on_hover_text("Islands, districts, spots and hits (T)").clicked() {
                            toggle_teleport = true;
                        }
                    }
                });
            });
        });
        if overview {
            self.overview(true);
        }
        self.teleport_open ^= toggle_teleport;
        self.sources_open ^= toggle_sources;
        self.settings_open ^= toggle_settings;
        self.help_open ^= toggle_help;
    }

    /// The status line: the frame rate and the warnings on the right, the
    /// rest on the left, cut short (all of it in the tooltip) when narrow.
    fn status_bar(&mut self, ui: &mut Ui) {
        let t = self.theme;
        let wide = ui.available_width() >= 1000.0;
        let walking: Vec<&Island> = self
            .islands
            .iter()
            .filter(|i| !i.status.done && i.hw.is_none())
            .collect();
        let nodes: usize = self.islands.iter().map(|i| i.tree.len()).sum();
        let mut left = format!(
            "{} · {}",
            count(self.islands.len() as u64, "island", "islands"),
            count(nodes as u64, "entry", "entries")
        );
        if self.starting {
            left.push_str(" · looking at the mounts…");
        }
        if !walking.is_empty() {
            let entries: u64 = walking.iter().map(|i| i.status.entries).sum();
            left.push_str(&format!(" · walking {} ({} found)", walking.len(), entries));
        }
        // what a walk could not show (said, never silent)
        let notes: Vec<String> = self
            .islands
            .iter()
            .filter_map(|i| i.status.note.as_ref().map(|n| format!("{}: {n}", i.title)))
            .collect();
        let skipped: u64 = self.islands.iter().map(|i| i.status.skipped_names).sum();
        match &self.mcp {
            McpState::Connected { label, .. } => left.push_str(&format!(" · axross: {label}")),
            McpState::Looking => left.push_str(" · looking for axross…"),
            _ => {}
        }
        let toast = self
            .toast
            .as_ref()
            .filter(|(_, at)| at.elapsed() < Duration::from_secs(6))
            .map(|(text, _)| text.clone());
        let stats = self
            .render_stats
            .as_ref()
            .and_then(|s| s.lock().ok().map(|s| s.clone()));
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ms = FrameTimes::mean_ms(&self.times.frames);
                let fps = if ms > 0.0 { 1000.0 / ms } else { 0.0 };
                let adapter = stats.as_ref().map(|s| s.adapter.clone()).unwrap_or_default();
                ui.label(RichText::new(format!("{fps:.0} fps · {} boxes", self.last_frame_instances)).monospace().color(t.muted))
                    .on_hover_text(format!("drawn by {adapter}"));
                if let Some(s) = &stats {
                    if wide {
                        ui.add(egui::Label::new(RichText::new(&s.adapter).color(t.muted)).truncate());
                    }
                    if s.software {
                        ui.label(RichText::new(if wide { "software renderer: slow" } else { "software" }).color(t.warn)).on_hover_text(
                            "No GPU was found (or none that can draw this window): a software renderer draws the city, a few frames a second. \
                             With a GPU, see the README's GPU section (WGPU_BACKEND, WGPU_ADAPTER_NAME, Settings > GPU).",
                        );
                    }
                    if s.veil {
                        ui.label(RichText::new(if wide { "flash guard veiling" } else { "guard" }).color(t.warn))
                            .on_hover_text("The picture was about to flash faster than three times a second here; it is softened.");
                    }
                }
                if !notes.is_empty() || skipped > 0 {
                    let mut tip = notes.join("\n");
                    if skipped > 0 {
                        if !tip.is_empty() {
                            tip.push('\n');
                        }
                        tip.push_str(&format!("{skipped} names a server sent were refused (a slash, a dot-dot or a control character in them)"));
                    }
                    ui.label(RichText::new(if wide { "not all shown" } else { "partial" }).color(t.warn)).on_hover_text(tip);
                }
                if wide {
                    ui.label(RichText::new(match self.camera.mode {
                        Mode::Fly => format!("fly {:.0} m/s", self.camera.flight_speed()),
                        Mode::Orbit => "orbit".into(),
                    }).color(t.muted));
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let (text, color) = match &toast {
                        Some(x) => (x.clone(), t.strong),
                        None => (left.clone(), t.muted),
                    };
                    ui.add(egui::Label::new(RichText::new(&text).color(color)).truncate())
                        .on_hover_text(if toast.is_some() { format!("{text}\n{left}") } else { text.clone() });
                });
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
                        format!(
                            "{} in {}",
                            hw::human_bytes(n.total),
                            count(u64::from(n.count), "file", "files")
                        ),
                    ));
                    rows.push(("entries".into(), n.children.len().to_string()));
                    rows.push(("read".into(), n.listing.label().into()));
                    rows.push(("newest change".into(), preview::time_local(n.newest)));
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
        let path_text = match &isl.source {
            IslandSource::Local { root, .. } => {
                crate::tree::display_text(isl.tree.local_path(root, node).as_os_str())
            }
            IslandSource::Mcp { root, .. } => crate::tree::display_text(
                &crate::tree::os_from_bytes(&isl.tree.remote_path_bytes(root, node)),
            ),
            _ => String::new(),
        };
        let raw_path = match &isl.source {
            IslandSource::Local { root, .. } => Some(isl.tree.local_path(root, node)),
            _ => None,
        };
        let can_ax = self.can_open(OpenHow::Axross);
        let can_local = self.can_open(OpenHow::DefaultProgram);
        let ax_hint = match &self.axross {
            Axross::Ready(_) => "O".to_string(),
            Axross::Looking => "axross is being looked for".to_string(),
            Axross::Missing => "axross is not installed".to_string(),
            Axross::TooOld(_) => {
                "This axross is too old to open a path (0.15.186 or later can)".to_string()
            }
        };
        let ax_ready = matches!(self.axross, Axross::Ready(_));
        let mut fly = false;
        let mut frame = false;
        let mut open_how: Option<OpenHow> = None;
        ui.horizontal_wrapped(|ui| {
            fly = ui.button("Fly here").clicked();
            frame = ui
                .button("Frame island")
                .on_hover_text("See its whole island (Z); kept framed while it grows")
                .clicked();
            if can_ax {
                let label = if ax_ready { "Open in axross" } else { "Open in axross…" };
                if ui.button(label).on_hover_text(ax_hint).clicked() {
                    open_how = Some(OpenHow::Axross);
                }
            }
            if can_local {
                if ui
                    .button("Show in file manager")
                    .on_hover_text("Its folder, in the file manager: runs nothing")
                    .clicked()
                {
                    open_how = Some(OpenHow::FileManager);
                }
                if ui
                    .button("Open")
                    .on_hover_text("With the system's default program (Shift+O); programs, scripts and files on removable or network media ask first")
                    .clicked()
                {
                    open_how = Some(OpenHow::DefaultProgram);
                }
            }
            if !path_text.is_empty() && ui.button("Copy path").clicked() {
                // (the path as it is where it can be: escaped only when it is no text)
                let text = raw_path
                    .as_ref()
                    .and_then(|p| p.to_str().map(String::from))
                    .unwrap_or_else(|| path_text.clone());
                ui.ctx().copy_text(text);
            }
        });
        if fly {
            self.fly_to(i, node);
        }
        if frame {
            self.frame_island(i);
        }
        if let Some(how) = open_how {
            self.open_selected(how);
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
                    ui.label(
                        RichText::new(format!(
                            "the beginning (at most 64 KB, {} lines)",
                            preview::TEXT_LINES
                        ))
                        .color(t.muted),
                    );
                }
                egui::ScrollArea::both()
                    .id_salt("preview")
                    .max_height(480.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(text).monospace().size(11.5).color(t.text),
                            )
                            .extend(),
                        );
                    });
            }
            Content::Hex(text) => {
                ui.label(RichText::new("binary: the first bytes").color(t.muted));
                egui::ScrollArea::both()
                    .id_salt("preview")
                    .max_height(480.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(text).monospace().size(11.0).color(t.text),
                            )
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

    /// A place for a window: below the bar, `dx` from the view's left.
    /// A window `width` wide at the right, below the bar (Sources,
    /// Settings: the Teleport menu has the left - they never cover it).
    fn right_of_bar(&self, width: f32, dy: f32) -> Pos2 {
        Pos2::new(
            (self.view_rect.right() - width - 24.0).max(self.view_rect.left() + 8.0),
            self.view_rect.top() + 8.0 + dy,
        )
    }

    /// A tall window's default height: most of the view (Help, Settings
    /// open at a usable size, not 420 points with a scroll bar).
    fn tall(&self, ctx: &egui::Context) -> f32 {
        (ctx.content_rect().bottom() - self.view_rect.top() - 60.0).clamp(240.0, 820.0)
    }

    /// Where windows may stand: the screen below the bar.
    fn below_bar_rect(&self, ctx: &egui::Context) -> Rect {
        let screen = ctx.content_rect();
        Rect::from_min_max(Pos2::new(screen.left(), self.view_rect.top()), screen.max)
    }

    /// Where Settings and Sources may stand: the 3D view's area - never
    /// over the inspect panel at the right, the bar or the status line.
    fn view_area(&self) -> Rect {
        self.view_rect
    }

    /// The Teleport menu's systems rows as they stand: name and state
    /// (tests).
    pub fn teleport_rows(&mut self) -> Vec<(String, SysState)> {
        self.system_rows()
            .map(|r| {
                r.iter()
                    .map(|r| (r.name.clone(), r.state.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The Teleport menu's systems rows, made again only when the list,
    /// the filter or (four times a second at most) a state changed; sorted
    /// by name - a row never moves under the mouse.
    fn system_rows(&mut self) -> Result<std::rc::Rc<Vec<SysRow>>, String> {
        // (the server ended: its last list, marked, while it starts again)
        let stale = matches!(self.mcp, McpState::Looking | McpState::Failed(_))
            && self.stale_systems.is_some();
        let profiles = match &self.mcp {
            McpState::Connected { systems: Ok(s), .. } => &s.profiles,
            _ if stale => &self.stale_systems.as_ref().expect("stale").profiles,
            McpState::Connected {
                systems: Err(e), ..
            } => return Err(e.clone()),
            McpState::Looking => return Err("looking for axross's MCP server…".into()),
            McpState::Off => return Err("axross's MCP server is off (Settings)".into()),
            McpState::NotFound => {
                return Err(
                    "axross's MCP server was not found: no saved connections to show".into(),
                );
            }
            McpState::Failed(e) => {
                return Err(format!("axross's MCP server could not be started: {e}"));
            }
        };
        let fresh = self.tp_cache.key == (self.systems_version, self.teleport_filter.clone())
            && self
                .tp_cache
                .stamp
                .is_some_and(|t| t.elapsed() < Duration::from_millis(250));
        if fresh {
            return Ok(self.tp_cache.rows.clone());
        }
        // which island each system has (once, not per row)
        let mut island_of: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for (i, isl) in self.islands.iter().enumerate() {
            if let IslandSource::Mcp { backend, .. } = &isl.source
                && let Some(p) = backend.profile.as_deref()
            {
                island_of.insert(p, i);
            }
        }
        let q = self.teleport_filter.clone();
        let mut rows = Vec::new();
        for p in profiles {
            let fam = crate::systems::Family::of(&p.protocol);
            let proto = p
                .protocol_name
                .clone()
                .unwrap_or_else(|| p.protocol.clone());
            let host = p.host.clone().unwrap_or_default();
            let state = if stale {
                SysState::Unavailable(
                    "axross's server ended: the list as it was last known; it is being started again"
                        .into(),
                )
            } else {
                self.system_state_with(&p.name, island_of.get(p.name.as_str()).copied(), p)
            };
            let label = state.label();
            if !crate::systems::matches(
                &q,
                &[&p.name, &p.protocol, &proto, &host, fam.label(), &label],
            ) {
                continue;
            }
            let mut where_ = proto;
            if !host.is_empty() {
                where_.push_str(" · ");
                where_.push_str(&host);
            }
            let [r, g, b] = fam.color();
            rows.push(SysRow {
                name: p.name.clone(),
                sign: fam.sign(),
                family: fam.label(),
                color: Color32::from_rgb(r, g, b),
                where_,
                state,
                label,
                has_island: island_of.contains_key(p.name.as_str()),
            });
        }
        let rows = std::rc::Rc::new(rows);
        self.tp_cache.key = (self.systems_version, q);
        self.tp_cache.stamp = Some(Instant::now());
        self.tp_cache.rows = rows.clone();
        Ok(rows)
    }

    /// The hits the filter takes, found a little each frame (at most 1.5
    /// ms): (indices into the hits, all looked at).
    fn filtered_hits(&mut self) -> (usize, bool) {
        let Some(s) = &self.search else {
            return (0, true);
        };
        let q = self.teleport_filter.trim().to_lowercase();
        if q.is_empty() {
            return (s.hits.len(), true);
        }
        let f = &mut self.tp_cache.hits;
        if f.query != q || f.search != s.query || f.scanned > s.hits.len() {
            *f = HitFilter {
                query: q.clone(),
                search: s.query.clone(),
                ..Default::default()
            };
        }
        let start = Instant::now();
        while f.scanned < s.hits.len() {
            let (i, node) = s.hits[f.scanned];
            if let Some(n) = self.islands.get(i).and_then(|isl| isl.tree.get(node))
                && crate::systems::matches(&q, &[&display_name(&n.name, 80)])
            {
                f.matched.push(f.scanned as u32);
            }
            f.scanned += 1;
            if f.scanned.is_multiple_of(256) && start.elapsed() > Duration::from_micros(1500) {
                break;
            }
        }
        (f.matched.len(), f.scanned >= s.hits.len())
    }

    fn teleport_window(&mut self, ctx: &egui::Context) {
        if !self.teleport_open {
            return;
        }
        let mut open = true;
        let mut go: Option<(usize, NodeId)> = None;
        let mut spot: Option<usize> = None;
        let mut save: Option<usize> = None;
        let mut sys_go: Option<String> = None;
        let mut sys_cancel: Option<String> = None;
        let mut sys_close: Option<String> = None;
        let mut filter = self.teleport_filter.clone();
        let focus_filter = std::mem::take(&mut self.focus_filter);
        let total_systems = self.system_profiles().len();
        let t = self.theme;
        let screen = ctx.content_rect();
        let systems = self.system_rows();
        let (hit_count, hits_done) = self.filtered_hits();
        let q = self.teleport_filter.clone();
        let m = |fields: &[&str]| crate::systems::matches(&q, fields);
        let width = 560.0_f32.min(screen.width() - 24.0);
        let wide = width >= 520.0;
        let below_bar = self.view_rect.top() + 8.0;
        // (above the legend when there is room: it stays readable)
        let room = self
            .legend_rect(self.view_rect)
            .map(|l| l.top() - below_bar - 50.0)
            .filter(|h| *h >= 260.0)
            .unwrap_or(f32::MAX);
        egui::Window::new("Teleport")
            .open(&mut open)
            .default_width(width)
            .max_width(screen.width() - 16.0)
            .default_height((screen.height() - 120.0).clamp(200.0, 640.0).min(room))
            .max_height(screen.height() - below_bar - 40.0)
            .default_pos(egui::pos2(self.view_rect.left() + 8.0, below_bar))
            .constrain_to(egui::Rect::from_min_max(egui::pos2(0.0, below_bar), screen.max))
            .vscroll(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("filter").color(t.muted));
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut filter)
                            .hint_text("name, protocol, host, …")
                            .desired_width(ui.available_width() - 8.0),
                    );
                    if focus_filter {
                        // (T: typing goes into the filter at once)
                        field.request_focus();
                    }
                });
                if let Ok(rows) = &systems {
                    ui.label(
                        RichText::new(if q.trim().is_empty() {
                            count(total_systems as u64, "saved connection", "saved connections")
                        } else {
                            format!("{} of {total_systems} saved connections match", rows.len())
                        })
                        .color(t.muted)
                        .size(12.0),
                    );
                }
                egui::CollapsingHeader::new(RichText::new("Systems (axross's connection manager)").strong())
                    .id_salt("tp-systems")
                    .default_open(true)
                    .show(ui, |ui| match &systems {
                        Err(e) => {
                            ui.label(RichText::new(e).color(t.muted));
                        }
                        Ok(rows) if rows.is_empty() => {
                            ui.label(RichText::new(if q.trim().is_empty() {
                                "No saved connections in axross."
                            } else {
                                "No saved connection matches the filter."
                            }).color(t.muted));
                        }
                        Ok(rows) => {
                            let row_h = 24.0;
                            // (only the rows in sight are laid out)
                            egui::ScrollArea::vertical()
                                .id_salt("tp-system-rows")
                                .max_height((ui.available_height() * 0.6).max(row_h * 10.0))
                                .auto_shrink([false, true])
                                .show_rows(ui, row_h, rows.len(), |ui, range| {
                                    for r in &rows[range] {
                                        ui.horizontal(|ui| {
                                            ui.set_height(row_h - 4.0);
                                            match &r.state {
                                                SysState::Connecting => {
                                                    if ui.small_button("cancel").clicked() {
                                                        sys_cancel = Some(r.name.clone());
                                                    }
                                                }
                                                SysState::Failed(_) => {
                                                    if ui.small_button("retry").clicked() {
                                                        sys_go = Some(r.name.clone());
                                                    }
                                                }
                                                SysState::NeedsSecret => {
                                                    if ui.small_button("unlock").clicked() {
                                                        sys_go = Some(r.name.clone());
                                                    }
                                                }
                                                SysState::Unavailable(_) => {
                                                    ui.add_enabled(false, egui::Button::new("go").small());
                                                }
                                                _ => {
                                                    if ui.small_button("go").clicked() {
                                                        sys_go = Some(r.name.clone());
                                                    }
                                                }
                                            }
                                            if r.has_island
                                                && !matches!(r.state, SysState::Connecting)
                                                && ui.small_button("close").on_hover_text("Close it (frees one of the server's open systems)").clicked()
                                            {
                                                sys_close = Some(r.name.clone());
                                            }
                                            ui.label(RichText::new(r.sign).monospace().color(r.color)).on_hover_text(r.family);
                                            ui.add(egui::Label::new(RichText::new(&r.name).monospace().strong()).truncate());
                                            if wide {
                                                ui.add(egui::Label::new(RichText::new(&r.where_).color(t.muted).size(12.0)).truncate())
                                                    .on_hover_text(&r.where_);
                                            }
                                            let color = match r.state {
                                                SysState::Failed(_) | SysState::Unavailable(_) | SysState::NeedsSecret => t.warn,
                                                SysState::Open | SysState::Walking | SysState::Connecting => t.accent,
                                                SysState::NotOpen => t.muted,
                                            };
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                ui.add(egui::Label::new(RichText::new(&r.label).color(color)).truncate())
                                                    .on_hover_text(format!("{}\n{}", r.where_, r.label));
                                            });
                                        });
                                    }
                                });
                        }
                    });
                egui::CollapsingHeader::new(RichText::new("Islands").strong())
                    .id_salt("tp-islands")
                    .default_open(true)
                    .show(ui, |ui| {
                        for (i, isl) in self.islands.iter().enumerate() {
                            if isl.city.is_none() {
                                continue;
                            }
                            let kids = island_districts(isl, &mut self.tp_districts);
                            let island_hit = m(&[&isl.title]);
                            let kids: Vec<NodeId> = if island_hit {
                                kids.to_vec()
                            } else {
                                kids.iter()
                                    .copied()
                                    .filter(|c| m(&[&display_name(&isl.tree.nodes[*c as usize].name, 200)]))
                                    .collect()
                            };
                            if !island_hit && kids.is_empty() {
                                continue;
                            }
                            ui.horizontal(|ui| {
                                if ui.button("go").clicked() {
                                    go = Some((i, 0));
                                }
                                ui.add(egui::Label::new(RichText::new(&isl.title).strong()).truncate());
                            });
                            let show = |ui: &mut Ui, list: &[NodeId], go: &mut Option<(usize, NodeId)>| {
                                for &c in list {
                                    ui.horizontal(|ui| {
                                        ui.add_space(18.0);
                                        if ui.small_button("go").clicked() {
                                            *go = Some((i, c));
                                        }
                                        let n = &isl.tree.nodes[c as usize];
                                        let text = if isl.hw.is_some() {
                                            display_name(&n.name, 40)
                                        } else {
                                            format!("{}/  {}", display_name(&n.name, 40), hw::human_bytes(n.total))
                                        };
                                        ui.add(egui::Label::new(RichText::new(text).color(t.muted)).truncate());
                                    });
                                }
                            };
                            show(ui, &kids[..kids.len().min(8)], &mut go);
                            if kids.len() > 8 {
                                egui::CollapsingHeader::new(format!("{} more", kids.len() - 8))
                                    .id_salt(("tp-more", isl.id))
                                    .show(ui, |ui| show(ui, &kids[8..], &mut go));
                            }
                        }
                    });
                egui::CollapsingHeader::new(RichText::new("Spots (1-9 jump, Ctrl+1-9 save)").strong())
                    .id_salt("tp-spots")
                    .default_open(true)
                    .show(ui, |ui| {
                        for k in 0..9 {
                            let s = self.settings.bookmarks.get(k).cloned().flatten();
                            let label = s.as_ref().map_or("-".to_string(), |s| s.label.clone());
                            if !q.trim().is_empty() && !m(&[&label]) {
                                continue;
                            }
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(s.is_some(), egui::Button::new(format!("{}", k + 1)).small())
                                    .clicked()
                                {
                                    spot = Some(k + 1);
                                }
                                if ui.small_button("save here").clicked() {
                                    save = Some(k + 1);
                                }
                                ui.add(egui::Label::new(RichText::new(label).color(t.muted)).truncate());
                            });
                        }
                    });
                // the search hits: every one, only those in sight laid out
                if let Some(s) = &self.search
                    && !s.hits.is_empty()
                {
                    let filtered = !q.trim().is_empty();
                    let title = format!(
                        "Search hits ({hit_count}{}{})",
                        if filtered { " match the filter" } else { "" },
                        if !s.done || !hits_done { ", still looking" } else { "" }
                    );
                    egui::CollapsingHeader::new(RichText::new(title).strong())
                        .id_salt("tp-hits")
                        .default_open(true)
                        .show(ui, |ui| {
                            let row_h = 22.0;
                            egui::ScrollArea::vertical()
                                .id_salt("tp-hit-list")
                                .max_height(row_h * 12.0)
                                .auto_shrink([false, true])
                                .show_rows(ui, row_h, hit_count, |ui, range| {
                                    for k in range {
                                        let at = if filtered { self.tp_cache.hits.matched[k] as usize } else { k };
                                        let Some(&(i, node)) = s.hits.get(at) else { continue };
                                        let name = self
                                            .islands
                                            .get(i)
                                            .and_then(|isl| isl.tree.get(node))
                                            .map(|x| display_name(&x.name, 60))
                                            .unwrap_or_default();
                                        ui.horizontal(|ui| {
                                            ui.set_height(row_h - 4.0);
                                            if ui.small_button("go").clicked() {
                                                go = Some((i, node));
                                            }
                                            ui.add(egui::Label::new(RichText::new(name).monospace()).truncate());
                                        });
                                    }
                                });
                        });
                }
            });
        self.teleport_filter = filter;
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
        if let Some(name) = sys_cancel {
            self.cancel_system(&name);
            self.tp_cache.stamp = None;
        }
        if let Some(name) = sys_close {
            self.close_system(&name);
            self.tp_cache.stamp = None;
        }
        if let Some(name) = sys_go {
            if let Some(l) = self.live.get_mut(&name)
                && matches!(l.state, SysState::Failed(_))
            {
                l.state = SysState::NotOpen;
            }
            self.teleport_system(&name, Vec::new());
            self.tp_cache.stamp = None;
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
        let mut stop: Option<usize> = None;
        let mut remove: Option<usize> = None;
        let mut add: Option<String> = None;
        let mut reconnect = false;
        let width = 420.0_f32.min(ctx.content_rect().width() - 24.0);
        egui::Window::new("Sources")
            .open(&mut open)
            .default_pos(self.right_of_bar(width, 0.0))
            .constrain_to(self.view_area())
            .default_width(width)
            .max_height(ctx.content_rect().height() - 80.0)
            .vscroll(true)
            .show(ctx, |ui| {
            for (i, isl) in self.islands.iter().enumerate() {
                ui.horizontal(|ui| {
                    let state = if isl.hw.is_some() {
                        "the machine".to_string()
                    } else if let Some(e) = &isl.status.error {
                        match isl.from_cache {
                            Some(t) => format!("could not be read ({e}); the city from {}", preview::time_local(t)),
                            None => format!("could not be read: {e}"),
                        }
                    } else if isl.from_cache.is_some() {
                        format!("cached; walking ({} found)", isl.status.entries)
                    } else if isl.status.done {
                        let mut s = format!(
                            "{} in {:.1} s",
                            count(isl.tree.len() as u64, "entry", "entries"),
                            isl.status.seconds
                        );
                        if let Some(n) = &isl.status.note {
                            s.push_str(&format!(" ({n})"));
                        }
                        s
                    } else {
                        format!("walking: {} found", isl.status.entries)
                    };
                    ui.label(RichText::new(&isl.title).monospace());
                    ui.label(RichText::new(state).color(t.muted));
                    let local = matches!(isl.source, IslandSource::Local { .. } | IslandSource::Synthetic { .. });
                    if isl.hw.is_none() {
                        if isl.walker.is_some() && !isl.status.done {
                            if ui.small_button("stop").on_hover_text("Stop this walk: what it found stays").clicked() {
                                stop = Some(i);
                            }
                        } else if ui.small_button("walk again").clicked() {
                            rescan = Some(i);
                        }
                    }
                    if local && ui.small_button("remove").on_hover_text("Take this island out of the city (nothing on the disk changes)").clicked() {
                        remove = Some(i);
                    }
                });
            }
            ui.horizontal(|ui| {
                ui.label("Add a folder:");
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.add_source_text)
                        .hint_text("/path/to/folder")
                        .desired_width(220.0),
                );
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if (ui.button("add").clicked() || enter) && !self.add_source_text.trim().is_empty() {
                    add = Some(self.add_source_text.trim().to_string());
                }
            });
            ui.separator();
            ui.label(RichText::new("axross").strong());
            // the program (Open in axross, its connection manager)
            ui.label(match &self.axross {
                Axross::Ready(p) => format!(
                    "axross: {} - opens paths and saved connections ({} or later)",
                    crate::tree::display_text(p.as_os_str()),
                    AXROSS_VERSION
                ),
                Axross::TooOld(p) => format!(
                    "axross: {} is too old to open a path; {AXROSS_VERSION} or later can",
                    crate::tree::display_text(p.as_os_str())
                ),
                Axross::Missing => format!(
                    "axross not found: install it ({AXROSS_VERSION} or later: pip install axross, or the releases page) for Open in axross and its connection manager; local folders and mounts work without it"
                ),
                Axross::Looking => "axross: looking for it…".into(),
            });
            match &self.mcp {
                McpState::Off => {
                    ui.label("Off (Settings).");
                }
                McpState::Looking => {
                    ui.label("Looking for axross-headless-mcp…");
                }
                McpState::NotFound => {
                    ui.label("axross's MCP server not found: local folders and mounts work without it.");
                    ui.label(RichText::new(format!(
                        "For other systems (axross's saved connections), install axross {AXROSS_VERSION} or later \
                         (its server runs as axross --mcp-server), or axross-headless-mcp (pip install \
                         axross-headless-mcp, or uvx), or give its command in Settings."
                    )).color(t.muted));
                    ui.hyperlink_to("github.com/c0decave/axross", "https://github.com/c0decave/axross");
                }
                McpState::Failed(e) => {
                    ui.label(RichText::new(format!("axross could not be started: {e}")).color(t.warn));
                    if ui.button("try again").clicked() {
                        reconnect = true;
                    }
                }
                McpState::Connected { label, command, client, systems } => {
                    ui.label(format!("MCP server: {label} ({} {}), {} tools", client.server, client.version, client.tools.len()));
                    ui.label(RichText::new(format!("started as: {command}")).monospace().size(11.0).color(t.muted));
                    match systems {
                        Err(e) => {
                            ui.label(RichText::new(format!("No saved connections: {e}.")).color(t.warn));
                        }
                        Ok(sys) => {
                            ui.label(RichText::new(format!(
                                "{} saved connections; axross has backends for {} protocols. Teleport (T) opens them.",
                                sys.profiles.len(),
                                sys.protocols.len()
                            )).color(t.muted));
                            // (only the rows in sight are laid out: thousands
                            // of systems cost no more than a few)
                            let row_h = 22.0;
                            egui::ScrollArea::vertical()
                                .id_salt("src-system-rows")
                                .max_height(row_h * 12.0)
                                .auto_shrink([false, true])
                                .show_rows(ui, row_h, sys.profiles.len(), |ui, range| {
                                    for p in &sys.profiles[range] {
                                        ui.horizontal(|ui| {
                                            ui.set_height(row_h - 4.0);
                                            let fam = crate::systems::Family::of(&p.protocol);
                                            let [r, g, b] = fam.color();
                                            ui.label(RichText::new(fam.sign()).monospace().color(Color32::from_rgb(r, g, b)));
                                            ui.label(RichText::new(&p.name).monospace());
                                            if ui.small_button("teleport").clicked() {
                                                open_profile = Some((client.clone(), p.name.clone()));
                                            }
                                        });
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
        if let Some(i) = stop {
            self.stop_walk(i);
        }
        if let Some(i) = remove {
            let title = self.islands[i].title.clone();
            self.remove_island(i);
            self.toast(format!("{title}: taken out of the city"));
        }
        if let Some(text) = add {
            self.request_source(&text);
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
        if let Some((_, name)) = open_profile {
            self.teleport_system(&name, Vec::new());
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            self.settings_shown = false;
            return;
        }
        if !self.settings_shown {
            self.settings_shown = true;
            self.measure_cache();
        }
        let mut open = true;
        let t = self.theme;
        let before = self.settings.clone();
        let mut mode =
            UpdateMode::from_key(&self.settings.update_mode).unwrap_or(UpdateMode::Notify);
        let mut clear_cache = false;
        let width = 400.0_f32.min(ctx.content_rect().width() - 24.0);
        egui::Window::new("Settings")
            .open(&mut open)
            .default_pos(self.right_of_bar(width, 24.0))
            .constrain_to(self.view_area())
            .default_width(width)
            .default_height(self.tall(ctx))
            .max_height(ctx.content_rect().height() - 80.0)
            .vscroll(true)
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
                ui.label(RichText::new("Opening").strong());
                if s.open_types.is_empty() {
                    ui.label(
                        RichText::new("Every type this program does not know is asked about first.")
                            .color(t.muted),
                    );
                } else {
                    let list: Vec<String> = s.open_types.iter().map(|e| format!(".{e}")).collect();
                    ui.label(format!("Opened without asking: {}", list.join(" ")));
                    if ui.button("Ask again for every type").clicked() {
                        s.open_types.clear();
                    }
                }
                ui.label(
                    RichText::new("Programs, scripts, shortcuts and special files are always asked about.")
                        .color(t.muted),
                );
                ui.separator();
                ui.label(RichText::new("Islands").strong());
                ui.checkbox(
                    &mut s.show_pseudo_mounts,
                    "Show pseudo file systems (proc, sysfs, tmpfs, …) - at the next start",
                );
                ui.checkbox(
                    &mut s.show_bind_mounts,
                    "Show bind mounts of what another island shows - at the next start",
                );
                ui.checkbox(
                    &mut s.hardware_island,
                    "The machine's own island - at the next start",
                );
                ui.checkbox(
                    &mut s.size_by_allocated,
                    "Size buildings by the space on the disk (a sparse file is small)",
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
                ui.label(RichText::new("Walk cache").strong());
                ui.label(
                    RichText::new("What the walks found, kept to show the city at once next time (only you can read it).")
                        .color(t.muted),
                );
                ui.checkbox(
                    &mut s.cache_sensitive,
                    "Also keep removable media, network and remote islands",
                );
                ui.add(egui::Slider::new(&mut s.cache_max_mb, 16..=8192).logarithmic(true).text("MiB at most"));
                ui.add(egui::Slider::new(&mut s.cache_max_days, 1..=365).text("days at most"));
                ui.horizontal_wrapped(|ui| {
                    if let Some((bytes, files)) = self.cache_usage {
                        ui.label(RichText::new(format!("now {} in {}", hw::human_bytes(bytes), count(files as u64, "file", "files"))).color(t.muted));
                    }
                    if ui.button("Clear the cache").clicked() {
                        clear_cache = true;
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
                ui.checkbox(&mut s.msaa, "Smooth edges (four samples a pixel)");
                ui.add(
                    egui::Slider::new(&mut s.idle_fps, 0..=30)
                        .text("frames a second while nothing moves (the rain; 0: none)"),
                );
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
        if clear_cache {
            self.prune_cache(true);
        }
        if self.settings.size_by_allocated != before.size_by_allocated {
            // (every city laid out again, by the other size)
            for isl in &mut self.islands {
                if isl.hw.is_none() {
                    isl.dirty = true;
                    isl.epoch += 1;
                }
            }
        }
        if self.settings != before {
            self.camera.sensitivity = self.settings.mouse_sensitivity;
            self.camera.invert_y = self.settings.invert_y;
            let theme = theme::find(&self.settings.theme);
            if !std::ptr::eq(theme, self.theme) {
                self.theme = theme;
                // (the instances carry the theme's colours: made again,
                // where they stand)
                for isl in &mut self.islands {
                    let id = isl.id;
                    let Some(c) = isl.city.as_mut() else { continue };
                    if isl.hw.is_some() {
                        c.rebuild(theme);
                    } else {
                        self.layout.place(world::PlaceJob {
                            id,
                            layout: c.layout.clone(),
                            ring: c.ring,
                            offset: c.offset,
                            theme,
                        });
                    }
                }
                self.beams_dirty = true;
            }
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
        let width = 440.0_f32.min(ctx.content_rect().width() - 24.0);
        egui::Window::new("Keys and mouse")
            .open(&mut open)
            .default_pos(Pos2::new(
                (self.view_rect.center().x - width * 0.5).max(self.view_rect.left() + 8.0),
                self.view_rect.top() + 8.0,
            ))
            .constrain_to(self.below_bar_rect(ctx))
            .default_width(width)
            .default_height(self.tall(ctx))
            .max_height(ctx.content_rect().height() - 80.0)
            .vscroll(true)
            .show(ctx, |ui| {
                ui.label(RichText::new(KEY_HINT).color(self.theme.strong));
                ui.separator();
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
            let cached = self
                .islands
                .iter()
                .any(|i| i.fresh.is_some() || !i.deferred.is_empty());
            // (islands still being placed: their instances on the way)
            let placing = self.islands.iter().any(|i| {
                i.city
                    .as_ref()
                    .is_some_and(|c| !c.shown || c.target.is_some_and(|t| t != c.offset))
            });
            if !self.starting
                && !walking
                && !laying
                && !cached
                && !placing
                && self.islands.iter().all(|i| i.city.is_some())
            {
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

/// The least time between two teleports (seconds): fewer than one and a
/// half a second, well inside the three flashes a second the rule allows.
const TELEPORT_GAP: f32 = 0.7;

/// Search hits with a pillar of light over them (the rest: N, teleport).
const PILLARS: usize = 400;

const LEGEND_LINE: f32 = 15.0;
const LEGEND_COL: f32 = 92.0;

/// Where the legend goes in the view `rect` and in how many columns (two
/// where the view is low); None where there is no room for it.
/// The machine's legend: what its parts' colours are.
const HARDWARE_LEGEND: [(crate::tree::FileType, &str); 8] = [
    (crate::tree::FileType::Code, "processor cores"),
    (crate::tree::FileType::Data, "memory modules"),
    (crate::tree::FileType::Document, "memory in use"),
    (crate::tree::FileType::Archive, "disks, storage"),
    (crate::tree::FileType::Video, "graphics card"),
    (crate::tree::FileType::Config, "network ports"),
    (crate::tree::FileType::Audio, "USB devices"),
    (crate::tree::FileType::Binary, "board, chips"),
];

fn legend_geometry(rect: Rect) -> Option<(Rect, usize)> {
    let types = crate::tree::FILE_TYPES.len() - 1;
    let cols = if rect.height() < 520.0 { 2 } else { 1 };
    let per_col = types.div_ceil(cols);
    let h = LEGEND_LINE * (per_col + 3) as f32 + 26.0;
    let w = (LEGEND_COL * cols as f32).max(176.0) + 16.0;
    let at = Pos2::new(rect.left() + 10.0, rect.bottom() - h - 10.0);
    if at.y < rect.top() + 40.0 || w > rect.width() * 0.5 {
        return None;
    }
    Some((Rect::from_min_size(at, vec2(w, h)), cols))
}

/// How much bigger the machine's island (of side `base`) is drawn beside
/// a city of side `biggest`: a quarter of it, in steps of half again, 1 to 40.
pub fn machine_scale(biggest: f32, base: f32) -> f32 {
    let want = (biggest / 4.0 / base.max(1.0)).clamp(1.0, 40.0);
    if want <= 1.0 {
        return 1.0;
    }
    let steps = (want.ln() / 1.5f32.ln()).round();
    1.5f32.powf(steps).clamp(1.0, 40.0)
}

/// The ground colour of a system's island: its family's.
fn family_tint(source: &IslandSource) -> Option<u32> {
    match source {
        IslandSource::Mcp { backend, .. } => backend.protocol.as_deref().map(|p| {
            let [r, g, b] = crate::systems::Family::of(p).color();
            city::pack_color(Color32::from_rgb(r, g, b))
        }),
        _ => None,
    }
}

/// The node at `path` below the root (raw names), as far as the tree has
/// it: the deepest node found and how many names are left.
pub fn find_path(tree: &crate::tree::Tree, path: &[Vec<u8>]) -> (NodeId, usize) {
    let mut at: NodeId = 0;
    for (k, name) in path.iter().enumerate() {
        let next = tree.get(at).and_then(|n| {
            n.children
                .iter()
                .copied()
                .find(|&c| crate::tree::os_to_bytes(&tree.nodes[c as usize].name) == *name)
        });
        match next {
            Some(c) => at = c,
            None => return (at, path.len() - k),
        }
    }
    (at, 0)
}

/// When the next frame is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Repaint {
    /// At once (something moves).
    Now,
    /// After a while (the rain, slowly).
    After(Duration),
    /// Only when something happens (an input, a walk, an answer).
    Idle,
}

/// The idle policy: nothing is drawn while the window is hidden or
/// minimised; when nothing moves only the rain asks for frames (`idle_fps`
/// a second focused, one a second unfocused) - without it, none.
pub fn repaint_policy(
    animating: bool,
    hidden: bool,
    focused: bool,
    rain: bool,
    idle_fps: u32,
) -> Repaint {
    if hidden {
        return Repaint::Idle;
    }
    if animating {
        return if focused {
            Repaint::Now
        } else {
            Repaint::After(Duration::from_millis(100))
        };
    }
    if !rain || idle_fps == 0 {
        return Repaint::Idle;
    }
    let every = Duration::from_secs_f32(1.0 / idle_fps as f32);
    Repaint::After(if focused {
        every
    } else {
        every.max(Duration::from_secs(1))
    })
}

/// axross's state: the program (given or on the PATH), asked whether it
/// opens paths unless the remembered answer is for this very file.
pub fn probe_axross(
    given: Option<PathBuf>,
    known: Option<settings::AxrossProbe>,
) -> (Axross, Option<settings::AxrossProbe>) {
    let Some(path) = given.or_else(open::axross_on_path) else {
        return (Axross::Missing, None);
    };
    let Some((size, mtime)) = open::file_stamp(&path) else {
        return (Axross::Missing, None);
    };
    let key = path.to_string_lossy().into_owned();
    let (opens, manager) = match known {
        Some(k) if k.path == key && k.size == size && k.mtime == mtime => {
            (k.opens_paths, k.connection_manager)
        }
        _ => {
            let help = open::axross_help(&path);
            (
                help.contains("--profile") && help.contains("PATH"),
                help.contains("--connection-manager"),
            )
        }
    };
    let probe = settings::AxrossProbe {
        path: key,
        size,
        mtime,
        opens_paths: opens,
        connection_manager: manager,
    };
    let state = if opens {
        Axross::Ready(path)
    } else {
        Axross::TooOld(path)
    };
    (state, Some(probe))
}

/// The first start's hint (and the Help window's first line).
pub const KEY_HINT: &str =
    "WASD fly · F orbit · / search · T teleport · Home back · Z frame island · H help";

pub const HELP: &[(&str, &str)] = &[
    ("W A S D", "fly forward, left, back, right (arrows too)"),
    ("Q / E", "down / up"),
    (
        "Shift",
        "five times as fast (flight is faster the higher it flies)",
    ),
    ("wheel", "flight speed factor; in orbit: zoom"),
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
    (
        "T",
        "teleport: other systems, islands, districts, spots, hits",
    ),
    ("1 - 9", "jump to a saved spot"),
    ("Ctrl+1 - 9", "save this spot"),
    ("Home", "back to the first view (the biggest city, low)"),
    ("Z", "frame the chosen island whole (kept while it grows)"),
    (
        "Overview",
        "the map: the whole world from above (the bar's button)",
    ),
    (
        "O / Shift+O",
        "open in axross / with the default program (asks first for programs)",
    ),
    ("L", "the colour legend"),
    (
        "Esc",
        "let the mouse go, close the menu and windows, give a connect up, then clear the choice",
    ),
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
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        // (a font the checks let through that still fails while a glyph is
        // drawn: the frame is caught, the fonts found are dropped for the
        // built-in ones, said so, and the window goes on)
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.frame_ui(ui, frame)));
        if let Err(e) = r {
            if !self.fonts_ready {
                std::panic::resume_unwind(e);
            }
            self.drop_fonts(ui.ctx());
        }
    }

    fn on_exit(&mut self) {
        self.save_all();
    }
}

impl App {
    /// The fonts found are given up for egui's own (a frame failed while
    /// they were in use).
    pub fn drop_fonts(&mut self, ctx: &egui::Context) {
        ctx.set_fonts(egui::FontDefinitions::default());
        self.fonts_ready = false;
        self.fonts_dropped = true;
        self.toast(
            "A font of this system could not be drawn: the built-in fonts are used (some scripts show as boxes)"
                .into(),
        );
        eprintln!("axross-files-matrix: a font failed while drawing; the built-in fonts are used");
        ctx.request_repaint();
    }

    fn frame_ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        if std::mem::take(&mut self.test_font_failure) {
            // (the tests' stand-in for a glyph that fails while drawn)
            panic!("a font failed while drawing (test)");
        }
        let t0 = Instant::now();
        if let Some(r) = ui.ctx().input(|i| i.viewport().inner_rect) {
            self.window_size = Some([r.width(), r.height()]);
        }
        let ctx = ui.ctx().clone();
        let (focused, hidden) = ctx.input(|i| {
            let v = i.viewport();
            let lost = i
                .events
                .iter()
                .any(|e| matches!(e, egui::Event::WindowFocused(false)));
            (
                i.focused && v.focused != Some(false) && !lost,
                v.minimized == Some(true) || v.occluded == Some(true),
            )
        });
        self.focused = focused;
        self.hidden = hidden;
        if let Some(sm) = &self.sampler {
            // (resting: the load is not worth a frame)
            let rest =
                hidden || !focused || self.settings.idle_fps == 0 || self.settings.reduce_motion;
            sm.quiet.store(rest, std::sync::atomic::Ordering::Relaxed);
        }
        if !focused && self.captured {
            // (another window has the keyboard: the mouse is let go)
            self.set_capture(&ctx, false);
        }
        self.pump(Duration::from_millis(6));
        let t_pump = t0.elapsed();
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
        let full = ctx.content_rect();
        if self.selected.is_some() {
            let side = (full.width() * 0.4).clamp(200.0, 340.0);
            egui::Panel::right("inspect")
                .default_size(side)
                .max_size((full.width() * 0.6).max(220.0))
                .frame(
                    egui::Frame::new()
                        .fill(t.surface)
                        .inner_margin(egui::Margin::same(10)),
                )
                .show(ui, |ui| {
                    ui.set_min_width(side.min(260.0));
                    egui::ScrollArea::vertical()
                        .id_salt("inspect-scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.inspect_panel(ui));
                });
        }
        let t_panels = t0.elapsed();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::BLACK))
            .show(ui, |ui| self.view(ui));
        let t_view = t0.elapsed();
        self.teleport_window(&ctx);
        self.sources_window(&ctx);
        self.settings_window(&ctx);
        self.help_window(&ctx);
        self.ask_mode_window(&ctx);
        self.open_ask_window(&ctx);
        let mut mode = mode;
        if self.updater.window(&ctx, &mut mode) {
            self.settings.update_mode = mode.key().to_string();
            let _ = settings::save(&self.settings_path, &self.settings);
        }
        // the adaptive level of detail: fewer boxes when frames are slow
        let work = t0.elapsed().as_secs_f32();
        if self.opts.frame_stats && work > 0.025 {
            // (a slow frame: where its time went)
            eprintln!(
                "slow frame {:.1} ms: pump {:.1}, bars and panels {:.1}, view {:.1}, windows {:.1}",
                work * 1000.0,
                t_pump.as_secs_f32() * 1000.0,
                (t_panels - t_pump).as_secs_f32() * 1000.0,
                (t_view - t_panels).as_secs_f32() * 1000.0,
                work * 1000.0 - t_view.as_secs_f32() * 1000.0
            );
        }
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
                let max_work = self.times.work.iter().copied().fold(0.0f32, f32::max) * 1000.0;
                eprintln!(
                    "frame {:.2} ms (p95 {:.2}), work {:.2} ms (max {max_work:.2}), {} boxes, lod {:.3}",
                    FrameTimes::mean_ms(&self.times.frames),
                    FrameTimes::p95_ms(&self.times.frames),
                    FrameTimes::mean_ms(&self.times.work),
                    self.last_frame_instances,
                    self.lod.collapse
                );
            }
            // (no repaint asked for here: the frames measured are the ones
            // the window draws anyway - an idle window stays idle)
        }
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

    #[test]
    fn an_idle_window_draws_little_or_nothing() {
        let ms = Duration::from_millis;
        // hidden or minimised: nothing, whatever moves
        assert_eq!(repaint_policy(true, true, true, true, 8), Repaint::Idle);
        // moving: every frame (focused), ten a second (not)
        assert_eq!(repaint_policy(true, false, true, true, 8), Repaint::Now);
        assert_eq!(
            repaint_policy(true, false, false, true, 8),
            Repaint::After(ms(100))
        );
        // still: the rain's slow tick, one a second unfocused, none without rain
        assert_eq!(
            repaint_policy(false, false, true, true, 8),
            Repaint::After(ms(125))
        );
        assert_eq!(
            repaint_policy(false, false, false, true, 8),
            Repaint::After(ms(1000))
        );
        assert_eq!(repaint_policy(false, false, true, false, 8), Repaint::Idle);
        assert_eq!(repaint_policy(false, false, true, true, 0), Repaint::Idle);
    }

    #[test]
    fn previews_get_a_fresh_thread_when_the_three_hang() {
        let ms = Duration::from_millis;
        // three under way, fresh: the request waits a moment...
        let three = [ms(100), ms(200), ms(250)];
        assert!(!preview_may_start(&three, ms(0)));
        assert!(!preview_may_start(&three, ms(299)));
        // ...then gets a fourth thread, not five seconds later
        assert!(preview_may_start(&three, PREVIEW_PATIENCE));
        // two under way: at once
        assert!(preview_may_start(&three[..2], ms(0)));
        // hung past the limit: they count no more
        assert!(preview_may_start(&[ms(6000); 3], ms(0)));
        // never more than PREVIEW_ALIVE threads in all
        assert!(!preview_may_start(
            &vec![ms(9000); PREVIEW_ALIVE],
            Duration::from_secs(60)
        ));
    }

    #[test]
    fn the_hover_tip_keeps_off_the_islands_titles() {
        let view = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let size = vec2(160.0, 18.0);
        let p = Pos2::new(400.0, 300.0);
        // nothing in the way: below right of the pointer
        assert_eq!(hover_tip_at(p, size, view, &[]), p + vec2(14.0, 10.0));
        // a title (with its failure's reason) below right: elsewhere
        let title = Rect::from_min_size(Pos2::new(395.0, 305.0), vec2(300.0, 40.0));
        let at = hover_tip_at(p, size, view, &[title]);
        let tip = Rect::from_min_size(at - vec2(4.0, 2.0), size);
        assert!(!tip.intersects(title), "{tip:?}");
        // at the window's edge: inside
        let at = hover_tip_at(Pos2::new(790.0, 590.0), size, view, &[]);
        assert!(view.contains_rect(Rect::from_min_size(at - vec2(4.0, 2.0), size)));
    }

    #[test]
    fn a_failed_island_says_why_on_its_own_line() {
        assert_eq!(
            title_text("sftp-box  · failed: no route to host"),
            "sftp-box\nfailed: no route to host"
        );
        assert_eq!(title_text("home  · 1200 …"), "home  · 1200 …");
    }

    #[test]
    fn the_view_s_motion_rises_at_once_and_settles() {
        let cam = crate::camera::Camera {
            pos: vec3(0.0, 100.0, 0.0),
            ..Default::default()
        };
        let still = (cam.pos, cam.yaw, cam.pitch);
        assert_eq!(
            view_motion(0.0, Some(still), &cam, 100.0, 1.0 / 30.0, false),
            0.0
        );
        // 120 m/s at 100 m: fast
        let mut moved = cam.clone();
        moved.pos.z -= 6.0;
        // (eased: a quarter of a second to come)
        let mut m = 0.0;
        for _ in 0..15 {
            m = view_motion(m, Some(still), &moved, 100.0, 1.0 / 30.0, false);
        }
        assert!(m > 0.8, "{m}");
        let first = view_motion(0.0, Some(still), &moved, 100.0, 1.0 / 30.0, false);
        assert!(first < 0.2, "never at once: {first}");
        // along a wall (19 m/s, things 3 m away): fast too
        let mut along = cam.clone();
        along.pos.z -= 19.0 / 30.0;
        let mut m = 0.0;
        for _ in 0..15 {
            m = view_motion(m, Some(still), &along, 3.0, 1.0 / 30.0, false);
        }
        assert!(m > 0.8, "{m}");
        // a slow turn (10 degrees a second): barely
        let mut turned = cam.clone();
        turned.yaw += 10f32.to_radians() / 30.0;
        assert!(view_motion(0.0, Some(still), &turned, 100.0, 1.0 / 30.0, false) < 0.05);
        // it settles over half a second, not at once
        let after = view_motion(1.0, Some(still), &cam, 100.0, 1.0 / 30.0, false);
        assert!(after > 0.9 && after < 1.0, "{after}");
        // a teleport's jump is no motion
        let mut jumped = cam.clone();
        jumped.pos.x += 5000.0;
        assert_eq!(
            view_motion(0.2, Some(still), &jumped, 100.0, 1.0 / 30.0, false),
            0.2
        );
        assert_eq!(
            view_motion(0.2, Some(still), &moved, 100.0, 1.0 / 30.0, true),
            0.2
        );
    }

    #[test]
    fn a_tilde_is_the_home_folder() {
        let home = std::path::Path::new("/u/me");
        assert_eq!(expand_home("~", Some(home)), PathBuf::from("/u/me"));
        assert_eq!(expand_home("~/x/y", Some(home)), PathBuf::from("/u/me/x/y"));
        assert_eq!(expand_home("/abs", Some(home)), PathBuf::from("/abs"));
        assert_eq!(expand_home("~other", Some(home)), PathBuf::from("~other"));
        assert_eq!(expand_home("~", None), PathBuf::from("~"));
    }

    #[test]
    fn counts_say_one_and_many() {
        assert_eq!(count(1, "island", "islands"), "1 island");
        assert_eq!(count(0, "island", "islands"), "0 islands");
        assert_eq!(count(2, "entry", "entries"), "2 entries");
    }

    #[test]
    fn the_machine_grows_with_the_biggest_city() {
        assert_eq!(machine_scale(0.0, 220.0), 1.0);
        assert_eq!(machine_scale(1000.0, 220.0), 1.0);
        let k = machine_scale(9000.0, 220.0);
        assert!(k > 4.0 && k < 15.0, "{k}");
        // in steps: a little growth changes nothing
        assert_eq!(machine_scale(9000.0, 220.0), machine_scale(9300.0, 220.0));
        assert!(machine_scale(1e9, 220.0) <= 40.0);
    }

    #[test]
    fn the_legend_fits_or_stays_away() {
        let big = Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0));
        let (r, cols) = legend_geometry(big).unwrap();
        assert!(big.contains_rect(r));
        assert_eq!(cols, 1);
        let low = Rect::from_min_size(Pos2::ZERO, vec2(640.0, 380.0));
        if let Some((r, cols)) = legend_geometry(low) {
            assert!(low.contains_rect(r));
            assert_eq!(cols, 2);
        }
        assert!(legend_geometry(Rect::from_min_size(Pos2::ZERO, vec2(200.0, 150.0))).is_none());
    }
}
