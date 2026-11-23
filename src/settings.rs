//! The settings, in the configuration folder
//! (`$XDG_CONFIG_HOME/axross-files-matrix/settings.json`, macOS
//! `~/Library/Application Support`, Windows `%APPDATA%`). Every field has
//! its default: a file from an older or newer version, or one with fields
//! missing, reads; a file that is no JSON at all (or not this program's)
//! is set aside as `settings.json.corrupt` and the defaults are used - the
//! program starts either way. Written atomically (a file beside it, then
//! renamed over).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FILE: &str = "settings.json";
const MAX_BYTES: u64 = 1 << 20;

/// A saved spot (Ctrl+1..9) - where the camera was and where it looked.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Spot {
    pub pos: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpSettings {
    /// Look for axross's MCP server and show its backends.
    pub enabled: bool,
    /// The server's command, program first (empty: found by itself).
    pub command: Vec<String>,
    /// Let `uvx` fetch axross-headless-mcp when it does not have it yet
    /// (off: `uvx --offline`, nothing is downloaded by itself).
    pub allow_uvx_download: bool,
    /// Also show the server's local backend (the local disks are islands
    /// already).
    pub show_local_backend: bool,
    /// Saved remote profiles opened at the start.
    pub open_profiles: Vec<String>,
}

impl Default for McpSettings {
    fn default() -> McpSettings {
        McpSettings {
            enabled: true,
            command: Vec::new(),
            allow_uvx_download: false,
            show_local_backend: false,
            open_profiles: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Read-only: nothing is changed on any disk. (Write operations come
    /// in a later release; until then this stays on whatever the file
    /// says - see [`Settings::effective_read_only`].)
    pub read_only: bool,
    /// Ask "read-only or write?" before the flight starts.
    pub ask_mode_before_flight: bool,
    /// Show proc, sysfs, cgroup, tmpfs and the like as islands.
    pub show_pseudo_mounts: bool,
    /// The machine's own island.
    pub hardware_island: bool,
    pub theme: String,
    pub mcp: McpSettings,
    /// Entries a local island reads by itself (the rest on demand).
    pub walk_budget: usize,
    pub walk_workers: usize,
    pub mouse_sensitivity: f32,
    pub invert_y: bool,
    /// The flight speed (metres a second; the wheel changes it).
    pub fly_speed: f32,
    pub labels: bool,
    pub bloom: bool,
    pub scanlines: bool,
    pub rain: bool,
    /// Less motion: no warp streaks, slower camera flights, still rain.
    pub reduce_motion: bool,
    /// Ctrl+1..9 save, 1..9 jump.
    pub bookmarks: Vec<Option<Spot>>,
    /// "off", "notify" or "auto" (src/update.rs).
    pub update_mode: String,
    /// When the last check ran (seconds since 1970) and whether it failed.
    pub update_last_check: Option<i64>,
    pub update_last_failed: bool,
    pub window: Option<[f32; 2]>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            read_only: true,
            ask_mode_before_flight: false,
            show_pseudo_mounts: false,
            hardware_island: true,
            theme: "matrix".into(),
            mcp: McpSettings::default(),
            walk_budget: 1_500_000,
            walk_workers: 4,
            mouse_sensitivity: 1.0,
            invert_y: false,
            fly_speed: 25.0,
            labels: true,
            bloom: true,
            scanlines: true,
            rain: true,
            reduce_motion: false,
            bookmarks: vec![None; 9],
            update_mode: "notify".into(),
            update_last_check: None,
            update_last_failed: false,
            window: None,
        }
    }
}

impl Settings {
    /// Read-only in effect: always, in this release (the write operations
    /// are not built yet; the setting is kept for when they are).
    pub fn effective_read_only(&self) -> bool {
        true
    }

    /// The values brought into range (a hand-edited file).
    pub fn sanitized(mut self) -> Settings {
        let d = Settings::default();
        if !(0.05..=20.0).contains(&self.mouse_sensitivity) || !self.mouse_sensitivity.is_finite() {
            self.mouse_sensitivity = d.mouse_sensitivity;
        }
        if !(0.5..=20_000.0).contains(&self.fly_speed) || !self.fly_speed.is_finite() {
            self.fly_speed = d.fly_speed;
        }
        self.walk_budget = self.walk_budget.clamp(1_000, 20_000_000);
        self.walk_workers = self.walk_workers.clamp(1, 16);
        self.bookmarks.resize(9, None);
        for s in self.bookmarks.iter_mut().flatten() {
            if !s.pos.iter().all(|v| v.is_finite() && v.abs() < 1e7)
                || !s.yaw.is_finite()
                || !s.pitch.is_finite()
            {
                *s = Spot::default();
            }
            s.label = s.label.chars().take(120).collect();
        }
        if !["off", "notify", "auto"].contains(&self.update_mode.as_str()) {
            self.update_mode = d.update_mode;
        }
        if let Some(w) = self.window
            && !w
                .iter()
                .all(|v| v.is_finite() && (200.0..=20_000.0).contains(v))
        {
            self.window = None;
        }
        self.mcp.command.truncate(64);
        self
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("axross-files-matrix")
}

/// What reading the settings found.
#[derive(Clone, Debug, PartialEq)]
pub enum Loaded {
    Fresh,
    Read,
    /// The file was not good: set aside (where), defaults used.
    SetAside(PathBuf),
}

/// Reads the settings at `path` (defaults where there is no file or no
/// good one).
pub fn load(path: &Path) -> (Settings, Loaded) {
    let Ok(meta) = std::fs::metadata(path) else {
        return (Settings::default(), Loaded::Fresh);
    };
    let parsed = if meta.len() > MAX_BYTES {
        None
    } else {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Settings>(&b).ok())
    };
    match parsed {
        Some(s) => (s.sanitized(), Loaded::Read),
        None => {
            let aside = path.with_extension("json.corrupt");
            let _ = std::fs::rename(path, &aside);
            (Settings::default(), Loaded::SetAside(aside))
        }
    }
}

pub fn save(path: &Path, s: &Settings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_vec_pretty(s).map_err(std::io::Error::other)?;
    let tmp = path.with_extension(format!("json.tmp{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::Scratch;

    #[test]
    fn missing_partial_and_corrupt_files() {
        let dir = Scratch::new("settings");
        let p = dir.path().join(FILE);
        let (s, how) = load(&p);
        assert_eq!(how, Loaded::Fresh);
        assert!(s.read_only && s.effective_read_only());
        // a file with a few fields: the rest default
        std::fs::write(
            &p,
            br#"{"labels": false, "bookmarks": [null], "future_field": 3}"#,
        )
        .unwrap();
        let (s, how) = load(&p);
        assert_eq!(how, Loaded::Read);
        assert!(!s.labels);
        assert_eq!(s.bookmarks.len(), 9);
        assert!(s.rain);
        // garbage: set aside, defaults
        std::fs::write(&p, b"\x00\xff{{{ not json").unwrap();
        let (s, how) = load(&p);
        assert!(matches!(how, Loaded::SetAside(_)));
        assert_eq!(s, Settings::default());
        assert!(!p.exists());
        // a JSON of the wrong shape
        std::fs::write(&p, b"[1, 2, 3]").unwrap();
        assert!(matches!(load(&p).1, Loaded::SetAside(_)));
    }

    #[test]
    fn values_out_of_range_are_brought_back() {
        let dir = Scratch::new("settings-range");
        let p = dir.path().join(FILE);
        std::fs::write(
            &p,
            br#"{"mouse_sensitivity": 1e30, "fly_speed": -4, "walk_workers": 999, "update_mode": "sometimes",
                 "bookmarks": [{"pos": [1e30, 0, 0], "yaw": 1}], "window": [10, 10], "read_only": false}"#,
        )
        .unwrap();
        let (s, _) = load(&p);
        assert_eq!(s.mouse_sensitivity, 1.0);
        assert_eq!(s.fly_speed, 25.0);
        assert_eq!(s.walk_workers, 16);
        assert_eq!(s.update_mode, "notify");
        assert_eq!(s.bookmarks[0], Some(Spot::default()));
        assert_eq!(s.window, None);
        // the write mode is not built: read-only in effect whatever the file says
        assert!(!s.read_only);
        assert!(s.effective_read_only());
    }

    #[test]
    fn saves_and_reads_back() {
        let dir = Scratch::new("settings-save");
        let p = dir.path().join("sub").join(FILE);
        let mut s = Settings::default();
        s.bookmarks[3] = Some(Spot {
            pos: [1.0, 2.0, 3.0],
            yaw: 0.5,
            pitch: -0.2,
            label: "home".into(),
        });
        s.mcp.command = vec![
            "/opt/axross-headless-mcp".into(),
            "--mcp-root".into(),
            "/srv".into(),
        ];
        save(&p, &s).unwrap();
        assert_eq!(load(&p).0, s);
    }
}
