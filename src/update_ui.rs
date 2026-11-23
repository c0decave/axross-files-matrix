//! The Updates window (*Options › Check for updates…*): the check, the
//! download with a plain bar, *Restart now* - the work on threads of its
//! own (src/update.rs), the window calm: no spinner, no blinking, the bar
//! fills and nothing else moves.

use crate::update::{self, Fetch, Install, Installed, Release, UpdateMode, Version, Web};
use egui::{RichText, Ui};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, channel};

#[derive(Clone, Debug)]
pub enum Stage {
    Idle,
    Checking,
    UpToDate(Version),
    Available(Release),
    Downloading(Version),
    Ready(Installed),
    /// *Restart now*: the new program started, its start watched.
    Restarting(Installed),
    /// Checked, but the program's folder cannot be written: the file there.
    Saved(PathBuf, Version),
    Failed(String),
}

enum Done {
    Checked(Result<Release, String>),
    /// A check in the background not made: offline, or a metered network.
    NotNow(String),
    Downloaded(Result<Result<Installed, (PathBuf, Version)>, String>),
    /// The new program's window is up (Ok), or it ended at its start and
    /// the old one was put back (Err: what happened).
    Restarted(Result<(), String>),
}

pub struct Updater {
    pub open: bool,
    pub stage: Stage,
    install: Install,
    rx: Option<Receiver<Done>>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    /// The check at the start found something (for the status line).
    notice: Option<String>,
    quiet: bool,
    cleaned: bool,
    /// When this program started (the old one is removed only after it
    /// has run a while: [`update::KEEP_PREVIOUS_SECS`]), and whether its
    /// download leftovers were looked for.
    started: std::time::Instant,
    swept: bool,
    /// Where releases come from and how they are checked (tests: a fake
    /// GitHub, their own key and self-test).
    source: Source,
    /// The last check in the background (when, by the wall clock - a
    /// machine asleep keeps the day - and whether it could
    /// not be made), and whether what it finds is put in place by itself.
    last_check: Option<(std::time::SystemTime, bool)>,
    auto: bool,
    /// The mode as the last frame had it (a check's answer coming after
    /// *Off* was chosen does nothing).
    mode: UpdateMode,
    /// Why what is found is not put in place by itself though that is
    /// chosen (the program's folder cannot be written).
    not_by_itself: Option<String>,
    /// Frames seen (the first start's watch ends after the first).
    frames: u32,
    /// The new program's window is up: this one closes.
    close: bool,
    /// The program in place was replaced while this one ran (said once).
    replaced: bool,
}

impl Default for Updater {
    fn default() -> Self {
        Updater::new(Install::current())
    }
}

/// Where releases come from, how they are checked, what the network is.
#[derive(Clone)]
pub struct Source {
    pub web: Arc<dyn Fetch>,
    pub keys: Vec<[u8; 32]>,
    pub cache: PathBuf,
    pub self_test: fn(&std::path::Path, Version) -> Result<(), String>,
    pub network: fn() -> update::Network,
    /// The version running (tests: an older one).
    pub current: Version,
}

impl Default for Source {
    fn default() -> Self {
        Source {
            web: Arc::new(Web::default()),
            keys: update::TRUSTED_KEYS.to_vec(),
            cache: update::cache_dir(),
            self_test: update::self_test,
            network: update::network,
            current: Version::current(),
        }
    }
}

impl Updater {
    pub fn new(install: Install) -> Updater {
        Updater::with_source(install, Source::default())
    }

    pub fn with_source(install: Install, source: Source) -> Updater {
        Updater {
            open: false,
            stage: Stage::Idle,
            install,
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            done: Arc::new(AtomicU64::new(0)),
            total: Arc::new(AtomicU64::new(0)),
            notice: None,
            quiet: false,
            cleaned: false,
            started: std::time::Instant::now(),
            swept: false,
            close: false,
            source,
            last_check: None,
            auto: false,
            mode: UpdateMode::Off,
            not_by_itself: None,
            frames: 0,
            replaced: false,
        }
    }

    /// The checks in the background, as `mode` asks (each frame): at the
    /// start, then every [`update::CHECK_EVERY`] - on a thread of its own,
    /// never on an offline or metered network, never a restart (the
    /// window and the flight go on as they are). AXROSS_FILES_MATRIX_NO_UPDATE_CHECK
    /// turns them off.
    pub fn tick(&mut self, ctx: &egui::Context, mode: UpdateMode) {
        self.mode = mode;
        // (no release key pinned yet: nothing could be verified - no check)
        if self.source.keys.iter().all(|k| *k == [0u8; 32]) {
            return;
        }
        // (by themselves no more: a download of theirs under way stops)
        if mode != UpdateMode::Install && self.quiet && matches!(self.stage, Stage::Downloading(_))
        {
            self.cancel.store(true, Ordering::Relaxed);
        }
        let env = std::env::var(update::NO_CHECK_ENV).ok();
        if mode == UpdateMode::Off
            || !update::start_check_allowed(true, env.as_deref())
            || !self.install.can_update()
        {
            return;
        }
        let since = self
            .last_check
            .map(|(at, _)| at.elapsed().unwrap_or(std::time::Duration::MAX));
        let failed = self.last_check.is_some_and(|(_, f)| f);
        if self.busy() || matches!(self.stage, Stage::Ready(_) | Stage::Saved(..)) {
            return;
        }
        if !update::check_due(since, failed) {
            // (an idle window wakes for the next one)
            let every = if failed {
                update::CHECK_AGAIN
            } else {
                update::CHECK_EVERY
            };
            ctx.request_repaint_after(every.saturating_sub(since.unwrap_or_default()));
            return;
        }
        self.last_check = Some((std::time::SystemTime::now(), false));
        // (another copy - a second one, --new-instance - put the new
        // program in place: downloaded in full and refused every day after,
        // "changed since this program started")
        if self.install.replaced() {
            if !std::mem::replace(&mut self.replaced, true) {
                self.notice = Some(
                    "axross-files-matrix was updated by another copy meanwhile: \
                     restart it to use the new one"
                        .into(),
                );
            }
            return;
        }
        self.quiet = true;
        // (by themselves only where the program's folder can be written:
        // else downloaded at every start and never put in place - said)
        self.not_by_itself = None;
        if mode == UpdateMode::Install {
            self.not_by_itself = self.install.unwritable();
        }
        self.auto = mode == UpdateMode::Install && self.not_by_itself.is_none();
        self.stage = Stage::Checking;
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let (ctx, source) = (ctx.clone(), self.source.clone());
        std::thread::spawn(move || {
            let net = (source.network)();
            let done = if update::network_allows(net) {
                Done::Checked(update::latest(&*source.web))
            } else {
                Done::NotNow(if net.offline == Some(true) {
                    "offline".into()
                } else {
                    "a metered network".into()
                })
            };
            let _ = tx.send(done);
            ctx.request_repaint();
        });
    }

    /// Whether this window is to close (*Restart now*'s new program up):
    /// never by itself (tests).
    #[doc(hidden)]
    pub fn closing(&self) -> bool {
        self.close
    }

    /// What the start check found, once.
    pub fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }

    fn busy(&self) -> bool {
        matches!(
            self.stage,
            Stage::Checking | Stage::Downloading(_) | Stage::Restarting(_)
        )
    }

    pub fn check(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        self.stage = Stage::Checking;
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let (ctx, web) = (ctx.clone(), self.source.web.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Done::Checked(update::latest(&*web)));
            ctx.request_repaint();
        });
    }

    fn download(&mut self, ctx: &egui::Context, release: Release) {
        self.stage = Stage::Downloading(release.version);
        self.cancel.store(false, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
        self.total.store(0, Ordering::Relaxed);
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let (install, cancel, done, total, source) = (
            self.install.clone(),
            self.cancel.clone(),
            self.done.clone(),
            self.total.clone(),
            self.source.clone(),
        );
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut last = 0u64;
            let result = update::download(
                &release,
                &install,
                source.current,
                &*source.web,
                &source.keys,
                &source.cache,
                &mut |d, t| {
                    done.store(d, Ordering::Relaxed);
                    total.store(t, Ordering::Relaxed);
                    // (a repaint per half per cent at most: the bar fills calmly)
                    if t > 0 && (d - last) * 200 >= t {
                        last = d;
                        ctx.request_repaint();
                    }
                },
                &cancel,
            )
            .and_then(|got| {
                if got.in_place {
                    update::install(&got, source.self_test).map(|installed| {
                        // (its first start watched, whenever it comes)
                        update::mark_first_start(&installed);
                        Ok(installed)
                    })
                } else {
                    Ok(Err((got.path, got.version)))
                }
            });
            let _ = tx.send(Done::Downloaded(result));
            ctx.request_repaint();
        });
    }

    /// *Restart now*: the new program started, its start watched on a
    /// thread; ended at its start, the old one is put back (and this one,
    /// still running, says so).
    fn restart(&mut self, ctx: &egui::Context, installed: Installed) {
        self.stage = Stage::Restarting(installed.clone());
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = update::restart(
                &installed.path,
                &installed.sha256,
                std::time::Duration::from_secs(10),
            )
            .map_err(|why| match update::roll_back(&installed) {
                Ok(()) => format!(
                    "{why}; the old program was put back - this is axross-files-matrix {}",
                    Version::current()
                ),
                Err(e) => format!(
                    "{why}; the old program could not be put back ({e}): it is {}",
                    installed.previous.display()
                ),
            });
            let _ = tx.send(Done::Restarted(result));
            ctx.request_repaint();
        });
    }

    /// What the threads said.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.rx else { return };
        let Ok(done) = rx.try_recv() else { return };
        self.rx = None;
        let quiet = std::mem::take(&mut self.quiet);
        let auto = std::mem::take(&mut self.auto);
        self.stage = match done {
            Done::NotNow(why) => {
                self.last_check = Some((std::time::SystemTime::now(), true));
                eprintln!("axross-files-matrix: the update check waits ({why})");
                Stage::Idle
            }
            // (Off chosen while it was asked: nothing more)
            Done::Checked(_) if quiet && self.mode == UpdateMode::Off => Stage::Idle,
            Done::Checked(Ok(release)) if release.version > self.source.current => {
                // (one that did not start here before: never put in place
                // by itself again)
                let refused = update::refused(&self.install.target).contains(&release.version);
                if quiet && auto && self.mode == UpdateMode::Install && !refused {
                    // (put in place by itself: checked, run once, renamed -
                    // and nothing restarted)
                    self.download(ctx, release);
                    self.quiet = true;
                    return;
                }
                if quiet {
                    let v = release.version;
                    self.notice = Some(if refused {
                        format!(
                            "axross-files-matrix {v} did not start here before: it is not put in \
                             place by itself again (Options › Check for updates…)"
                        )
                    } else if let Some(why) = self
                        .not_by_itself
                        .as_ref()
                        .filter(|_| self.mode == UpdateMode::Install)
                    {
                        format!(
                            "axross-files-matrix {v} is out; it cannot be put in place by itself: \
                             {why} (Options › Check for updates…)"
                        )
                    } else {
                        format!("axross-files-matrix {v} is out: Options › Check for updates…")
                    });
                }
                Stage::Available(release)
            }
            Done::Checked(Ok(release)) => Stage::UpToDate(release.version),
            Done::Checked(Err(e)) if quiet => {
                self.last_check = Some((std::time::SystemTime::now(), true));
                eprintln!("axross-files-matrix: the update check failed: {e}");
                Stage::Idle
            }
            Done::Checked(Err(e)) => Stage::Failed(e),
            Done::Downloaded(Ok(Ok(installed))) => {
                // (the old program now is this one: kept until the new runs)
                self.cleaned = true;
                if quiet {
                    self.notice = Some(format!(
                        "axross-files-matrix {} is installed: it runs from the next start \
                         (Options › Check for updates… restarts it now)",
                        installed.version
                    ));
                }
                Stage::Ready(installed)
            }
            Done::Downloaded(Err(e)) if quiet => {
                eprintln!("axross-files-matrix: the update was not installed: {e}");
                self.notice = Some(format!("The update was not installed: {e}"));
                Stage::Failed(e)
            }
            Done::Restarted(Ok(())) => {
                self.close = true;
                return;
            }
            Done::Restarted(Err(e)) => Stage::Failed(e),
            Done::Downloaded(Ok(Err((path, version)))) => {
                if quiet {
                    self.notice = Some(format!(
                        "axross-files-matrix {version} is checked and kept as {}: this program's \
                         folder cannot be written - put it in place by hand",
                        path.display()
                    ));
                }
                Stage::Saved(path, version)
            }
            Done::Downloaded(Err(e)) => Stage::Failed(e),
        };
    }

    /// The menu's entries; true when the setting changed.
    pub fn menu(&mut self, ui: &mut Ui, mode: &mut UpdateMode) -> bool {
        if ui.button("Check for updates…").clicked() {
            self.open = true;
            if !self.busy() && self.install.can_update() {
                self.check(ui.ctx());
            }
            ui.close();
        }
        let changed = mode_choice(ui, mode);
        if changed {
            // (a new setting: looked for at once)
            self.last_check = None;
        }
        changed
    }

    /// The work in the background, each frame - from the window's logic,
    /// which runs also while it is minimised or hidden (from its drawing, a
    /// program left minimised would never look): the checks as
    /// `mode` asks, the leftovers swept, the old program let go, what the
    /// threads said.
    pub fn background(&mut self, ctx: &egui::Context, mode: UpdateMode) {
        self.frames = self.frames.saturating_add(1);
        if self.frames == 2 && self.install.can_update() {
            // (the first frame drawn: a first start, watched, went well)
            update::first_start_done(&self.install.target);
        }
        self.tick(ctx, mode);
        if !self.swept {
            self.swept = true;
            // (only an old program there at the start is this one's to
            // remove: not the one an update made of this one meanwhile)
            self.cleaned = !update::previous_of(&self.install.target).exists();
            if self.install.can_update() {
                if let Some(folder) = self.install.target.parent() {
                    update::cleanup_downloads(folder);
                }
                update::cleanup_downloads(&update::cache_dir());
            }
        }
        if !self.cleaned {
            // (this program has run a while - not only drawn a frame: the
            // one it replaced may go; until then it is there to be put back)
            let ran = self.started.elapsed().as_secs();
            if ran >= update::KEEP_PREVIOUS_SECS {
                self.cleaned = true;
                if self.install.can_update() {
                    update::cleanup_previous(&self.install.target);
                }
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs(
                    update::KEEP_PREVIOUS_SECS - ran,
                ));
            }
        }
        self.poll(ctx);
        if std::mem::take(&mut self.close) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// The window; true when the setting changed.
    pub fn window(&mut self, ctx: &egui::Context, mode: &mut UpdateMode) -> bool {
        if !self.open {
            return false;
        }
        let mut open = true;
        let mut changed = false;
        egui::Window::new("Updates")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(format!("This is axross-files-matrix {}.", Version::current()));
                if let Some(why) = self.install.why_not() {
                    ui.label(format!("Not updated here: {why}."));
                    ui.hyperlink_to("The releases on GitHub", update::RELEASES_PAGE);
                    return;
                }
                if self.source.keys.iter().all(|k| *k == [0u8; 32]) {
                    // (the release key is not pinned yet: nothing could be
                    // verified, so nothing is fetched)
                    ui.label("This build has no release-signing key yet: it cannot check updates. Get new releases from GitHub.");
                    ui.hyperlink_to("The releases on GitHub", update::RELEASES_PAGE);
                    return;
                }
                ui.add_space(6.0);
                self.stage_ui(ui);
                ui.add_space(6.0);
                changed = mode_choice(ui, mode);
            });
        if changed {
            // (a new setting: looked for at once - as from the menu)
            self.last_check = None;
        }
        if !open {
            self.open = false;
            // (a download asked for here stops with its window; one by
            // itself goes on)
            if matches!(self.stage, Stage::Downloading(_)) && !self.quiet {
                self.cancel.store(true, Ordering::Relaxed);
            }
        }
        changed
    }

    fn stage_ui(&mut self, ui: &mut Ui) {
        match self.stage.clone() {
            Stage::Idle | Stage::Failed(_) | Stage::UpToDate(_) => {
                match &self.stage {
                    Stage::UpToDate(v) => {
                        ui.label(format!("The latest release is {v}: nothing to update."));
                    }
                    Stage::Failed(e) => {
                        ui.label(RichText::new(format!("The update failed: {e}")).strong());
                        ui.label("The program is as it was.");
                    }
                    _ => {}
                }
                if ui.button("Check now").clicked() {
                    self.check(ui.ctx());
                }
            }
            Stage::Checking => {
                ui.label("Asking GitHub for the latest release…");
            }
            Stage::Available(release) => {
                ui.label(
                    RichText::new(format!("axross-files-matrix {} is out.", release.version))
                        .strong(),
                );
                if !release.notes.is_empty() {
                    egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .show(ui, |ui| ui.label(&release.notes));
                }
                ui.label(
                    "Download and install fetches it, checks its signature and checksum and \
                     puts it in place of this program; it takes effect when the program starts \
                     again.",
                );
                ui.horizontal(|ui| {
                    if ui.button("Download and install").clicked() {
                        self.download(ui.ctx(), release.clone());
                    }
                    ui.hyperlink_to("The release on GitHub", release.page());
                });
            }
            Stage::Downloading(version) => {
                let (done, total) = (
                    self.done.load(Ordering::Relaxed),
                    self.total.load(Ordering::Relaxed),
                );
                ui.label(format!("Downloading axross-files-matrix {version}…"));
                let part = if total > 0 {
                    done as f32 / total as f32
                } else {
                    0.0
                };
                ui.add(egui::ProgressBar::new(part).text(format!(
                    "{} of {} MB",
                    done >> 20,
                    total >> 20
                )));
                if ui.button("Cancel").clicked() {
                    self.cancel.store(true, Ordering::Relaxed);
                }
            }
            Stage::Ready(installed) => {
                ui.label(format!(
                    "axross-files-matrix {} is in place; it runs from the next start on.",
                    installed.version
                ));
                ui.label(format!(
                    "Should it not start, the old program is kept as {} for its first \
                     minute: put it back in place of {}.",
                    installed
                        .previous
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy(),
                    installed.path.display()
                ));
                if ui.button("Restart now").clicked() {
                    self.restart(ui.ctx(), installed.clone());
                }
            }
            Stage::Restarting(installed) => {
                ui.label(format!(
                    "Starting axross-files-matrix {}…",
                    installed.version
                ));
            }
            Stage::Saved(path, version) => {
                ui.label(format!(
                    "axross-files-matrix {version} is checked, but this program's folder cannot be \
                     written. The new program is {}: put it in place of this one.",
                    path.display()
                ));
            }
        }
    }
}

/// The setting: updates looked for - not at all, said, or put in place by
/// themselves (once a day, and at the start).
fn mode_choice(ui: &mut Ui, mode: &mut UpdateMode) -> bool {
    let mut changed = false;
    ui.label("Updates (at the start, then once a day):");
    for m in UpdateMode::ALL {
        if ui
            .radio(*mode == m, m.label())
            .on_hover_text(match m {
                UpdateMode::Off => "No request to GitHub unless asked",
                UpdateMode::Notify => "One request to GitHub a day: a word when one is out",
                UpdateMode::Install => {
                    "Fetched, its signature and checksum checked, run once and put in \
                     place; it runs from the next start - nothing is restarted"
                }
            })
            .clicked()
            && *mode != m
        {
            *mode = m;
            changed = true;
        }
    }
    changed
}
