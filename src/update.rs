//! Updates from axross-files-matrix's GitHub releases, checked before anything runs.
//!
//! How the updates are looked for is a setting ([`UpdateMode`]): not at all,
//! said when one is out, or put in place by themselves - at the start, then
//! once a day; a new installation says when one is out
//! ([`crate::settings::Settings::update_mode`]).
//! `AXROSS_FILES_MATRIX_NO_UPDATE_CHECK=1` keeps the checks in the
//! background off whatever the setting says. A check is one
//! request to `api.github.com/repos/c0decave/axross-files-matrix/releases/latest`.
//!
//! Which file is fetched ([`Install`], [`asset_name`]):
//!
//! | how it runs                    | the release's file                           |
//! |--------------------------------|----------------------------------------------|
//! | the Linux binary (x86-64)      | `axross-files-matrix-X.Y.Z-linux-x86_64`              |
//! | the AppImage                   | `axross-files-matrix-X.Y.Z-x86_64.AppImage` (replaces the AppImage file itself) |
//! | macOS (the .app or the binary) | `axross-files-matrix-X.Y.Z-macos-universal` (the program inside the .app) |
//! | Windows x64                    | `axross-files-matrix-X.Y.Z-windows-x64.exe`           |
//! | `cargo install`, a source tree | none: `cargo install axross-files-matrix`, or the checkout |
//!
//! Checks, in this order, before a byte of the program is fetched or
//! anything replaced:
//!
//! * every request and every redirect goes over https to one of
//!   [`ALLOWED_HOSTS`] only, redirects are followed one by one here (at most
//!   [`MAX_REDIRECTS`]), every answer is bounded ([`MAX_API_BYTES`],
//!   [`MAX_SUMS_BYTES`], [`SIG_BYTES`], [`MAX_ASSET_BYTES`]);
//! * the release's version is strictly newer than this program's;
//! * a file's URL is the release's own download URL, its name plain;
//! * `SHA256SUMS.sig` is the raw 64-byte Ed25519 signature of the exact
//!   bytes of `SHA256SUMS`, made with a key pinned in [`TRUSTED_KEYS`] - no
//!   signature, a wrong size or a key not pinned: refused, before any line
//!   of `SHA256SUMS` is read;
//! * the program's file has an entry in the signed `SHA256SUMS`; its size is
//!   the one the release states (and below the cap), its SHA-256 the
//!   entry's (and GitHub's digest, where the API gives one).
//!
//! Replacing ([`install`]): the file is downloaded next to the program (the
//! same file system), gets its mode, the old program stays as
//! `<name>.previous` until the new one has started ([`cleanup_previous`]),
//! then one rename puts the new one in place. Windows lets a running program
//! be renamed, not overwritten: the old one is renamed to `.previous` first.
//! A folder that cannot be written keeps the program as it is; the checked
//! file is left in the cache folder, and the window says where.

use ring::digest::{Context, SHA256};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const REPOSITORY: &str = "c0decave/axross-files-matrix";
pub const LATEST_URL: &str =
    "https://api.github.com/repos/c0decave/axross-files-matrix/releases/latest";
pub const RELEASES_PAGE: &str = "https://github.com/c0decave/axross-files-matrix/releases";
const DOWNLOAD_PREFIX: &str = "https://github.com/c0decave/axross-files-matrix/releases/download/";
/// The only hosts a request or a redirect may reach (https only).
pub const ALLOWED_HOSTS: [&str; 4] = [
    "api.github.com",
    "github.com",
    "release-assets.githubusercontent.com",
    "objects.githubusercontent.com",
];
pub const SUMS_NAME: &str = "SHA256SUMS";
pub const SIG_NAME: &str = "SHA256SUMS.sig";
/// An Ed25519 signature is exactly this long; it is also its size bound.
pub const SIG_BYTES: u64 = 64;
pub const MAX_API_BYTES: u64 = 1 << 20;
pub const MAX_SUMS_BYTES: u64 = 64 << 10;
pub const MAX_ASSET_BYTES: u64 = 512 << 20;
pub const MAX_REDIRECTS: usize = 5;
const MAX_ASSETS: usize = 200;
const MAX_SUMS_LINES: usize = 500;
const MAX_NOTES: usize = 20_000;
const CHUNK: usize = 64 * 1024;
/// Set to 1 / true / yes / on: no check at the start, whatever the setting.
pub const NO_CHECK_ENV: &str = "AXROSS_FILES_MATRIX_NO_UPDATE_CHECK";
const USER_AGENT: &str = concat!("axross-files-matrix/", env!("CARGO_PKG_VERSION"));

/// PLACEHOLDER - the release-signing public key is not set yet.
///
/// The maintainer makes the Ed25519 key pair on their own machine
/// (`scripts/sign_release.py keygen`, docs/RELEASING.md) and puts its raw
/// 32-byte public half here, replacing the zeros, before the first release
/// that is to update itself. Until then no release is ever put in place:
/// an all-zero key verifies nothing ([`verify_signature`] skips it), so the
/// updater says that updates are not signed yet and only points to the
/// releases page.
pub const UPDATE_PUBLIC_KEY: [u8; 32] = [0; 32];

/// The public halves (raw 32 bytes) of the keys a `SHA256SUMS` may be
/// signed with; one verifying is enough. More than one only while a key is
/// rotated: the new key is pinned here a release before releases are signed
/// with it. (The private key never leaves the maintainer's machine:
/// RELEASING.md.)
pub const TRUSTED_KEYS: &[[u8; 32]] = &[UPDATE_PUBLIC_KEY];

/// Whether a real release key is pinned (not the placeholder).
pub fn signing_key_set() -> bool {
    TRUSTED_KEYS.iter().any(|k| *k != [0u8; 32])
}

// --------------------------------------------------------------------------
// Versions
// --------------------------------------------------------------------------

/// X.Y.Z, each at most six digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// `X.Y.Z` or `vX.Y.Z`, nothing else (no pre-release, no build part).
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.strip_prefix('v').unwrap_or(text);
        let mut parts = text.split('.');
        let mut next = || -> Option<u32> {
            let p = parts.next()?;
            (!p.is_empty() && p.len() <= 6 && p.bytes().all(|b| b.is_ascii_digit()))
                .then(|| p.parse().ok())
                .flatten()
        };
        let v = Version(next()?, next()?, next()?);
        parts.next().is_none().then_some(v)
    }

    /// This program's version.
    pub fn current() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version(0, 0, 0))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

// --------------------------------------------------------------------------
// The release
// --------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// GitHub's `digest` (`sha256:<hex>`), where the API gives one.
    pub digest: Option<[u8; 32]>,
}

#[derive(Clone, Debug)]
pub struct Release {
    pub tag: String,
    pub version: Version,
    pub assets: Vec<Asset>,
    /// The release notes (plain text, bounded).
    pub notes: String,
}

impl Release {
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }

    /// The release's page on GitHub.
    pub fn page(&self) -> String {
        format!("{RELEASES_PAGE}/tag/{}", self.tag)
    }
}

/// A plain file name: letters, digits, `. _ -`, not starting with a dot.
pub fn plain_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 200
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(c))
}

fn hex32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Text from the release for the window: control and bidi characters out,
/// bounded.
pub fn clean_text(text: &str, max: usize) -> String {
    text.chars()
        .filter(|c| {
            (*c == '\n' || *c == '\t' || !c.is_control())
                && !matches!(*c as u32, 0x200b..=0x200f | 0x202a..=0x202e | 0x2066..=0x2069 | 0xfeff)
        })
        .take(max)
        .collect()
}

/// The release from the API's answer (`releases/latest`). A file whose name
/// is not plain or whose URL is not the release's own download URL is left
/// out; a draft, a pre-release or a tag that is no `vX.Y.Z` is refused.
pub fn parse_release(body: &[u8]) -> Result<Release, String> {
    let data: Value =
        serde_json::from_slice(body).map_err(|_| "GitHub's answer is not JSON".to_string())?;
    let tag = data
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or("the release has no tag")?;
    let version = Version::parse(tag)
        .filter(|v| tag == format!("v{v}"))
        .ok_or_else(|| format!("the release's tag {:?} is no version", clean_text(tag, 40)))?;
    if data.get("draft").and_then(Value::as_bool) == Some(true)
        || data.get("prerelease").and_then(Value::as_bool) == Some(true)
    {
        return Err("the latest release is a draft or a pre-release".into());
    }
    let mut assets = Vec::new();
    for raw in data
        .get("assets")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .take(MAX_ASSETS)
    {
        let (Some(name), Some(url), Some(size)) = (
            raw.get("name").and_then(Value::as_str),
            raw.get("browser_download_url").and_then(Value::as_str),
            raw.get("size").and_then(Value::as_u64),
        ) else {
            continue;
        };
        if !plain_name(name) || url != format!("{DOWNLOAD_PREFIX}{tag}/{name}") {
            continue;
        }
        let digest = raw
            .get("digest")
            .and_then(Value::as_str)
            .and_then(|d| d.strip_prefix("sha256:"))
            .and_then(hex32);
        assets.push(Asset {
            name: name.into(),
            url: url.into(),
            size,
            digest,
        });
    }
    let notes = clean_text(
        data.get("body").and_then(Value::as_str).unwrap_or(""),
        MAX_NOTES,
    );
    Ok(Release {
        tag: tag.into(),
        version,
        assets,
        notes,
    })
}

// --------------------------------------------------------------------------
// How this program runs
// --------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A release binary (Linux, macOS, Windows), the macOS .app's program.
    Binary,
    MacApp,
    AppImage,
    /// `cargo install`: updated with cargo.
    Cargo,
    /// A build in a source tree (`target/debug`, `target/release`).
    Source,
    /// No release file for this system.
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Install {
    pub kind: Kind,
    /// The file replaced: the program, or the AppImage.
    pub target: PathBuf,
    /// "linux", "macos", "windows".
    pub os: &'static str,
    pub arch: &'static str,
    /// The file as it was when this program looked at it (its size and
    /// time): another copy that put a new program there meanwhile is
    /// not followed by a second update, which kept the new program as
    /// `.previous` and lost the old one).
    pub seen: Option<(u64, Option<std::time::SystemTime>)>,
}

/// A file's size and time (None: not there).
fn seen(path: &Path) -> Option<(u64, Option<std::time::SystemTime>)> {
    fs::metadata(path)
        .ok()
        .map(|m| (m.len(), m.modified().ok()))
}

impl Install {
    /// This program.
    pub fn current() -> Install {
        let exe = std::env::current_exe().unwrap_or_default();
        let exe = fs::canonicalize(&exe).unwrap_or(exe);
        detect(
            std::env::consts::OS,
            std::env::consts::ARCH,
            &exe,
            std::env::var_os("APPIMAGE").map(PathBuf::from),
            std::env::var_os("APPDIR").map(PathBuf::from),
        )
    }

    pub fn can_update(&self) -> bool {
        matches!(self.kind, Kind::Binary | Kind::MacApp | Kind::AppImage)
    }

    /// Whether the program's folder can be written - a new program put in
    /// place there - (a file made and removed again); None when it can.
    /// The program in place is no longer the one this program found at
    /// its start: another copy (or a hand) put another there.
    pub fn replaced(&self) -> bool {
        self.seen.is_some() && seen(&self.target) != self.seen
    }

    pub fn unwritable(&self) -> Option<String> {
        let folder = self.target.parent()?;
        let probe = folder.join(format!(".axross-files-matrix-update-{}", unique()));
        match create_new(&probe) {
            Ok(_) => {
                let _ = fs::remove_file(&probe);
                None
            }
            Err(e) => Some(format!("{} cannot be written ({e})", folder.display())),
        }
    }

    /// Why this program is not updated here (None: it is).
    pub fn why_not(&self) -> Option<String> {
        match self.kind {
            Kind::Cargo => Some(
                "installed with cargo: update with \"cargo install axross-files-matrix\"".into(),
            ),
            Kind::Source => {
                Some("a build from the source tree: update the checkout and build again".into())
            }
            Kind::Unsupported => Some(format!(
                "the releases have no program for {} on {}",
                self.os, self.arch
            )),
            _ => None,
        }
    }
}

/// Whether *exe* lies in a cargo build folder of any name (a shared or
/// custom `CARGO_TARGET_DIR`): the profile's folder - where cargo keeps its
/// `.cargo-lock` - holds it, or its `deps` or `build` folder (the tests'
/// binaries): only a folder named `target` was known, a
/// developer's build elsewhere was offered to be replaced by a release.
fn built_by_cargo(exe: &Path) -> bool {
    let profile = |dir: &Path| dir.join(".cargo-lock").is_file();
    if exe.parent().is_some_and(profile) {
        return true;
    }
    exe.ancestors().skip(1).take(5).any(|dir| {
        dir.file_name().is_some_and(|n| n == "deps" || n == "build")
            && dir.parent().is_some_and(profile)
    })
}

/// How the program at *exe* runs. *appimage* / *appdir*: the AppImage
/// runtime's `APPIMAGE` and `APPDIR` - an AppImage only when the program
/// runs from inside `APPDIR` and `APPIMAGE` names an absolute
/// `….AppImage` (a variable set by hand changes nothing).
pub fn detect(
    os: &'static str,
    arch: &'static str,
    exe: &Path,
    appimage: Option<PathBuf>,
    appdir: Option<PathBuf>,
) -> Install {
    let install = |kind, target: &Path| Install {
        kind,
        target: target.to_path_buf(),
        os,
        arch,
        seen: seen(target),
    };
    let parts: Vec<String> = exe
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let n = parts.len();
    // (a build in a source tree: target/debug, target/release, also
    // target/<triple>/release and the tests' target/debug/deps)
    let in_tree = (0..n.saturating_sub(1)).any(|i| {
        parts[i] == "target"
            && parts[i + 1..n - 1]
                .iter()
                .take(2)
                .any(|p| p == "debug" || p == "release")
    });
    if in_tree || built_by_cargo(exe) {
        return install(Kind::Source, exe);
    }
    if n >= 3 && parts[n - 3] == ".cargo" && parts[n - 2] == "bin" {
        return install(Kind::Cargo, exe);
    }
    let supported = matches!(
        (os, arch),
        ("linux", "x86_64") | ("macos", _) | ("windows", "x86_64")
    );
    if !supported {
        return install(Kind::Unsupported, exe);
    }
    if os == "linux"
        && let (Some(image), Some(dir)) = (appimage, appdir)
        // (from / on: absolute on Linux - and so also in the tests on any system)
        && image.has_root()
        && image.to_string_lossy().ends_with(".AppImage")
        && exe.starts_with(&dir)
    {
        return install(Kind::AppImage, &image);
    }
    if os == "macos"
        && n >= 4
        && parts[n - 2] == "MacOS"
        && parts[n - 3] == "Contents"
        && parts[n - 4].ends_with(".app")
    {
        return install(Kind::MacApp, exe);
    }
    install(Kind::Binary, exe)
}

/// The release file for *install* at *version* (None: none is fetched).
pub fn asset_name(install: &Install, version: Version) -> Option<String> {
    match (install.kind, install.os) {
        (Kind::AppImage, _) => Some(format!("axross-files-matrix-{version}-x86_64.AppImage")),
        (Kind::Binary | Kind::MacApp, "macos") => {
            Some(format!("axross-files-matrix-{version}-macos-universal"))
        }
        (Kind::Binary, "linux") => Some(format!("axross-files-matrix-{version}-linux-x86_64")),
        (Kind::Binary, "windows") => Some(format!("axross-files-matrix-{version}-windows-x64.exe")),
        _ => None,
    }
}

// --------------------------------------------------------------------------
// The network: https to GitHub only, redirects one by one
// --------------------------------------------------------------------------

/// One answer, redirects not followed.
pub struct Reply {
    pub status: u16,
    pub location: Option<String>,
    pub length: Option<u64>,
    pub body: Box<dyn Read + Send>,
}

/// What makes one request (the real one: [`Web`]; tests a fake GitHub).
pub trait Fetch: Send + Sync {
    fn get(&self, url: &str) -> Result<Reply, String>;
}

/// The host of an https URL that may be asked: one of [`ALLOWED_HOSTS`],
/// no user part, no port but 443.
pub fn allowed_url(url: &str) -> Result<&str, String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| "not an https address".to_string())?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err("an address with a user name".into());
    }
    let host = match authority.rsplit_once(':') {
        Some((host, "443")) => host,
        Some(_) => return Err("an address with a port".into()),
        None => authority,
    };
    ALLOWED_HOSTS
        .iter()
        .find(|h| h.eq_ignore_ascii_case(host))
        .copied()
        .ok_or_else(|| format!("{} is not GitHub", clean_text(host, 80)))
}

/// Fetch *url* into *sink*: at most *max* bytes, redirects followed here
/// to allowed hosts only. Returns the bytes read.
pub fn fetch_into(
    fetch: &dyn Fetch,
    url: &str,
    max: u64,
    sink: &mut dyn FnMut(&[u8]) -> Result<(), String>,
    cancel: &AtomicBool,
) -> Result<u64, String> {
    let mut url = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let host = allowed_url(&url)?.to_string();
        let reply = fetch.get(&url)?;
        if matches!(reply.status, 301 | 302 | 303 | 307 | 308) {
            url = reply
                .location
                .filter(|l| l.starts_with("https://"))
                .ok_or_else(|| format!("{host} redirected without an https address"))?;
            continue;
        }
        match reply.status {
            200 => {}
            403 | 429 => {
                return Err(format!(
                    "{host} refused ({}): perhaps too many checks; try again in an hour",
                    reply.status
                ));
            }
            404 => return Err(format!("{host}: not found (404)")),
            status => return Err(format!("{host} answered {status}")),
        }
        if reply.length.is_some_and(|n| n > max) {
            return Err(format!("{host}: the file is larger than {max} bytes"));
        }
        let mut body = reply.body;
        let mut buf = vec![0u8; CHUNK];
        let mut total = 0u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let n = body.read(&mut buf).map_err(|e| format!("{host}: {e}"))?;
            if n == 0 {
                return Ok(total);
            }
            total += n as u64;
            if total > max {
                return Err(format!("{host}: the file is larger than {max} bytes"));
            }
            sink(&buf[..n])?;
        }
    }
    Err(format!("more than {MAX_REDIRECTS} redirects"))
}

pub fn fetch_bytes(fetch: &dyn Fetch, url: &str, max: u64) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    fetch_into(
        fetch,
        url,
        max,
        &mut |chunk| {
            out.extend_from_slice(chunk);
            Ok(())
        },
        &AtomicBool::new(false),
    )?;
    Ok(out)
}

/// The network, through ureq: no redirect followed by it, https only,
/// time-outs on connect, answer and every read; the environment's proxy.
pub struct Web {
    agent: ureq::Agent,
}

impl Default for Web {
    fn default() -> Self {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(20)))
            .timeout_per_read(Some(Duration::from_secs(30)))
            .user_agent(USER_AGENT)
            .build();
        Web {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

impl Fetch for Web {
    fn get(&self, url: &str) -> Result<Reply, String> {
        let mut request = self.agent.get(url);
        if url.starts_with("https://api.github.com/") {
            request = request
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28");
        }
        let response = request.call().map_err(|e| e.to_string())?;
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let location = header("location");
        let length = header("content-length").and_then(|v| v.trim().parse().ok());
        let status = response.status().as_u16();
        Ok(Reply {
            status,
            location,
            length,
            body: Box::new(response.into_body().into_reader()),
        })
    }
}

/// The latest release.
pub fn latest(fetch: &dyn Fetch) -> Result<Release, String> {
    parse_release(&fetch_bytes(fetch, LATEST_URL, MAX_API_BYTES)?)
}

// --------------------------------------------------------------------------
// The signature and the sums
// --------------------------------------------------------------------------

/// *signature* is a valid Ed25519 signature of *data* by one of *keys*.
pub fn verify_signature(data: &[u8], signature: &[u8], keys: &[[u8; 32]]) -> Result<(), String> {
    if signature.len() as u64 != SIG_BYTES {
        return Err(format!(
            "{SIG_NAME} is {} bytes, not an Ed25519 signature",
            signature.len()
        ));
    }
    // (the placeholder key - all zeros - verifies nothing)
    let ok = keys.iter().filter(|k| **k != [0u8; 32]).any(|key| {
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
            .verify(data, signature)
            .is_ok()
    });
    if ok {
        Ok(())
    } else {
        Err(format!(
            "{SUMS_NAME} is not signed with axross-files-matrix's release key"
        ))
    }
}

/// `SHA256SUMS` (`<hex>  <name>` or `<hex> *<name>` lines).
pub fn parse_sums(text: &str) -> Result<HashMap<String, [u8; 32]>, String> {
    let mut sums = HashMap::new();
    for line in text.lines().take(MAX_SUMS_LINES + 1) {
        if sums.len() >= MAX_SUMS_LINES {
            return Err(format!("{SUMS_NAME} has too many lines"));
        }
        if line.trim().is_empty() {
            continue;
        }
        let (digest, name) = line
            .split_once(' ')
            .ok_or_else(|| format!("{SUMS_NAME}: a line without a name"))?;
        let name = name
            .strip_prefix(' ')
            .or_else(|| name.strip_prefix('*'))
            .unwrap_or(name);
        let digest =
            hex32(digest).ok_or_else(|| format!("{SUMS_NAME}: a line without a SHA-256"))?;
        if !plain_name(name) {
            return Err(format!("{SUMS_NAME}: a file name that is not plain"));
        }
        if sums.insert(name.to_string(), digest).is_some() {
            return Err(format!("{SUMS_NAME} names a file twice"));
        }
    }
    Ok(sums)
}

// --------------------------------------------------------------------------
// Download, install
// --------------------------------------------------------------------------

/// The new program, checked, beside the old one (or in the cache folder).
#[derive(Clone, Debug)]
pub struct Downloaded {
    pub path: PathBuf,
    pub sha256: [u8; 32],
    pub version: Version,
    pub target: PathBuf,
    /// Beside the program: [`install`] can put it in place.
    pub in_place: bool,
    /// The program in place as this one found it ([`Install::seen`]).
    pub seen: Option<(u64, Option<std::time::SystemTime>)>,
}

/// The new program in place.
#[derive(Clone, Debug)]
pub struct Installed {
    pub path: PathBuf,
    pub previous: PathBuf,
    pub sha256: [u8; 32],
    pub version: Version,
}

fn sha256_of(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut context = Context::new(&SHA256);
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        context.update(&buf[..n]);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(context.finish().as_ref());
    Ok(out)
}

fn unique() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut bytes = [0u8; 6];
    if ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes).is_err() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        bytes.copy_from_slice(&nanos.to_le_bytes()[..6]);
    }
    format!("{}-{}", std::process::id(), hex(&bytes))
}

/// The cache folder for a program that cannot be put in place.
pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("axross-files-matrix")
        .join("update")
}

/// Fetch, check and keep the program of *release* for *install*: the
/// signature first, then the entry, the size and the SHA-256 (see the
/// module's text). *progress(done, total)* for the bar; *cancel* stops it.
/// Nothing is left behind when a check fails.
#[allow(clippy::too_many_arguments)]
pub fn download(
    release: &Release,
    install: &Install,
    current: Version,
    fetch: &dyn Fetch,
    keys: &[[u8; 32]],
    cache: &Path,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<Downloaded, String> {
    if release.version <= current {
        return Err(format!(
            "{} is not newer than this program ({current})",
            release.version
        ));
    }
    if let Some(why) = install.why_not() {
        return Err(why);
    }
    let name = asset_name(install, release.version).ok_or("no release file for this program")?;
    let asset = release
        .asset(&name)
        .ok_or_else(|| format!("the release has no {name}"))?;
    let (sums_asset, sig_asset) = match (release.asset(SUMS_NAME), release.asset(SIG_NAME)) {
        (Some(s), Some(g)) => (s, g),
        _ => return Err(format!("the release is not signed (no {SIG_NAME})")),
    };
    let signature = fetch_bytes(fetch, &sig_asset.url, SIG_BYTES)?;
    let sums = fetch_bytes(fetch, &sums_asset.url, MAX_SUMS_BYTES)?;
    verify_signature(&sums, &signature, keys)?;
    let sums =
        parse_sums(std::str::from_utf8(&sums).map_err(|_| format!("{SUMS_NAME} is not text"))?)?;
    let expected = *sums
        .get(&name)
        .ok_or_else(|| format!("{SUMS_NAME} has no entry for {name}"))?;
    if asset.digest.is_some_and(|d| d != expected) {
        return Err(format!("GitHub's digest of {name} is not the signed one"));
    }
    if asset.size == 0 || asset.size > MAX_ASSET_BYTES {
        return Err(format!(
            "{name} is {} bytes; at most {MAX_ASSET_BYTES} are fetched",
            asset.size
        ));
    }
    // (the folder cannot be written and the checked file is in the cache
    // already: kept, not fetched again at every start)
    let cached = cache.join(&name);
    if install.unwritable().is_some()
        && fs::symlink_metadata(&cached).is_ok_and(|m| m.file_type().is_file())
        && sha256_of(&cached).is_ok_and(|d| d == expected)
    {
        return Ok(Downloaded {
            path: cached,
            sha256: expected,
            version: release.version,
            target: install.target.clone(),
            in_place: false,
            seen: install.seen,
        });
    }
    // beside the program (one rename puts it in place), else the cache
    let folder = install
        .target
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let temp_name = format!(".axross-files-matrix-update-{}", unique());
    let (path, mut file, in_place) = match create_new(&folder.join(&temp_name)) {
        Ok(file) => (folder.join(&temp_name), file, true),
        Err(_) => {
            fs::create_dir_all(cache).map_err(|e| format!("{}: {e}", cache.display()))?;
            let path = cache.join(&temp_name);
            let file = create_new(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            (path, file, false)
        }
    };
    let result = (|| -> Result<[u8; 32], String> {
        let mut context = Context::new(&SHA256);
        let mut done = 0u64;
        let total = asset.size;
        let got = fetch_into(
            fetch,
            &asset.url,
            asset.size,
            &mut |chunk| {
                context.update(chunk);
                file.write_all(chunk).map_err(|e| e.to_string())?;
                done += chunk.len() as u64;
                progress(done, total);
                Ok(())
            },
            cancel,
        )?;
        if got != asset.size {
            return Err(format!("{name}: {got} of {} bytes came", asset.size));
        }
        let mut digest = [0u8; 32];
        digest.copy_from_slice(context.finish().as_ref());
        if digest != expected {
            return Err(format!("{name}: the SHA-256 is not the signed one"));
        }
        file.sync_all().map_err(|e| e.to_string())?;
        Ok(digest)
    })();
    drop(file);
    let sha256 = match result {
        Ok(d) => d,
        Err(e) => {
            let _ = fs::remove_file(&path);
            return Err(e);
        }
    };
    let (path, in_place) = if in_place {
        (path, true)
    } else {
        // (a name of its own in the cache: the user takes it from there)
        let named = cache.join(&name);
        let _ = fs::remove_file(&named);
        fs::rename(&path, &named).map_err(|e| {
            let _ = fs::remove_file(&path);
            e.to_string()
        })?;
        (named, false)
    };
    executable(&path, &install.target);
    Ok(Downloaded {
        path,
        sha256,
        version: release.version,
        target: install.target.clone(),
        in_place,
        seen: install.seen,
    })
}

fn create_new(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// The old program's mode (rwxr-xr-x when there is none).
fn executable(path: &Path, like: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // (the old one's read and run bits, never more than rwxr-xr-x; the
        // owner may always run it)
        let mode = fs::metadata(like)
            .map(|m| m.permissions().mode() & 0o755)
            .unwrap_or(0o755)
            | 0o700;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = (path, like);
}

/// The old program's name with `.previous`.
pub fn previous_of(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".previous");
    target.with_file_name(name)
}

/// What the new program's `--self-test` says when it can run here.
pub fn self_test_answer(version: Version) -> String {
    format!("axross-files-matrix {version} ok")
}

/// The new program run once before it replaces this one (a release that
/// cannot start on this system - a raised glibc floor, a library missing -
/// would leave a broken program): `--self-test` must say
/// [`self_test_answer`] within [`SELF_TEST_SECS`].
pub fn self_test(path: &Path, version: Version) -> Result<(), String> {
    let mut command = std::process::Command::new(path);
    command
        .arg("--self-test")
        .env(NO_CHECK_ENV, "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // (a group of its own: what it starts ends with it)
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("it cannot be run here ({e})"))?;
    // (its output read as it comes, on threads of their own, the first
    // 64 KB kept: unread, a program saying more than a pipe holds blocked
    // and was judged dead; read after it ended, a child of it holding the
    // pipe kept the reading - and the update - waiting past the bound for
    // good)
    let reader = |pipe: Option<Box<dyn Read + Send>>| {
        let (tx, rx) = std::sync::mpsc::channel();
        if let Some(mut pipe) = pipe {
            std::thread::spawn(move || {
                let mut kept = Vec::new();
                let mut buf = [0u8; 4096];
                while let Ok(n) = pipe.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    if kept.len() < 64 << 10 {
                        kept.extend_from_slice(&buf[..n.min((64 << 10) - kept.len())]);
                        // (the answer is a line: sent as it is complete)
                        let _ = tx.send(kept.clone());
                    }
                }
                let _ = tx.send(kept);
            });
        }
        rx
    };
    let out = reader(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let err = reader(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let end = std::time::Instant::now() + std::time::Duration::from_secs(SELF_TEST_SECS);
    let kill_group = |child: &mut std::process::Child| {
        #[cfg(unix)]
        // SAFETY: a signal to the process group the child leads (its own,
        // made at its spawn); no memory is touched.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < end => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Ok(None) => {
                kill_group(&mut child);
                return Err(format!("it did not answer within {SELF_TEST_SECS} s"));
            }
            Err(e) => {
                kill_group(&mut child);
                return Err(e.to_string());
            }
        }
    };
    // (what it said: the latest of each pipe, waited for a moment at most -
    // a child it left may hold the pipe open; then its group ended)
    let said = |rx: std::sync::mpsc::Receiver<Vec<u8>>| {
        let mut text = Vec::new();
        let until = std::time::Instant::now() + std::time::Duration::from_millis(500);
        while let Some(left) = until.checked_duration_since(std::time::Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(t) => text = t,
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&text).into_owned()
    };
    let out = said(out);
    let err = said(err);
    #[cfg(unix)]
    // SAFETY: as above - the child's own group, a signal only.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    if status.success() && out.trim() == self_test_answer(version) {
        return Ok(());
    }
    let said = err
        .lines()
        .chain(out.lines())
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(200)
        .collect::<String>();
    Err(if said.is_empty() {
        format!("it does not start here ({status})")
    } else {
        format!("it does not start here ({status}: {said})")
    })
}

/// The longest the new program's self-test may take.
pub const SELF_TEST_SECS: u64 = 20;

/// Put *downloaded* in place of the program: checked once more, run once
/// (`test`: [`self_test`]), the old program kept as `<name>.previous`, one
/// rename.
pub fn install(
    downloaded: &Downloaded,
    test: impl Fn(&Path, Version) -> Result<(), String>,
) -> Result<Installed, String> {
    if !downloaded.in_place {
        return Err(format!(
            "the program's folder cannot be written; the new program is in {}",
            downloaded.path.display()
        ));
    }
    if sha256_of(&downloaded.path).map_err(|e| e.to_string())? != downloaded.sha256 {
        let _ = fs::remove_file(&downloaded.path);
        return Err("the downloaded file changed since it was checked".into());
    }
    let target = &downloaded.target;
    // (one copy at a time puts a program in place)
    let _lock = Lock::take(target)?;
    if downloaded.seen.is_some() && seen(target) != downloaded.seen {
        let _ = fs::remove_file(&downloaded.path);
        return Err(format!(
            "{} changed since this program started (another one updated it): \
             it is the new program already",
            target.display()
        ));
    }
    if let Err(why) = test(&downloaded.path, downloaded.version) {
        let _ = fs::remove_file(&downloaded.path);
        // (never put in place by itself again)
        refuse(target, downloaded.version);
        return Err(format!(
            "axross-files-matrix {} was not installed: {why}; this one is kept",
            downloaded.version
        ));
    }
    let previous = previous_of(target);
    let fail = |e: std::io::Error| {
        let _ = fs::remove_file(&downloaded.path);
        format!("{}: {e}", target.display())
    };
    // (`.previous` a file, or a link - removed itself, never followed: a
    // link's file was overwritten by the copy below; a
    // folder of that name is not ours to remove)
    match fs::symlink_metadata(&previous) {
        Ok(m) if m.is_dir() => {
            let _ = fs::remove_file(&downloaded.path);
            return Err(format!(
                "{} is a folder: move it away to update",
                previous.display()
            ));
        }
        Ok(_) => {
            let _ = fs::remove_file(&previous);
        }
        Err(_) => {}
    }
    #[cfg(not(windows))]
    {
        // the old one stays under .previous (a hard link: the same file;
        // else a copy made under a name of its own, then renamed)
        if fs::hard_link(target, &previous).is_err() {
            let temp = target.with_file_name(format!(".axross-files-matrix-update-{}", unique()));
            let copied = create_new(&temp).and_then(|mut to| {
                let mut from = File::open(target)?;
                std::io::copy(&mut from, &mut to)?;
                to.sync_all()
            });
            if let Err(e) = copied.and_then(|_| fs::rename(&temp, &previous)) {
                let _ = fs::remove_file(&temp);
                return Err(fail(e));
            }
            executable(&previous, target);
        }
        fs::rename(&downloaded.path, target).map_err(fail)?;
    }
    #[cfg(windows)]
    {
        // (a running program may be renamed, not overwritten)
        fs::rename(target, &previous).map_err(fail)?;
        if let Err(e) = fs::rename(&downloaded.path, target) {
            let _ = fs::rename(&previous, target);
            return Err(fail(e));
        }
    }
    if let Some(folder) = target.parent()
        && let Ok(dir) = File::open(folder)
    {
        let _ = dir.sync_all();
    }
    Ok(Installed {
        path: target.clone(),
        previous,
        sha256: downloaded.sha256,
        version: downloaded.version,
    })
}

/// The new program has run [`KEEP_PREVIOUS_SECS`] (not only drawn its
/// first frame): the old one (`.previous`) goes. Not a link, not a folder -
/// a regular file of that name only.
pub fn cleanup_previous(target: &Path) {
    let previous = previous_of(target);
    if fs::symlink_metadata(&previous).is_ok_and(|m| m.file_type().is_file()) {
        let _ = fs::remove_file(&previous);
    }
}

/// How long the new program runs before the old one (`.previous`) goes.
pub const KEEP_PREVIOUS_SECS: u64 = 60;

/// Leftovers of a download that ended with its program (a closed window, a
/// crash): `.axross-files-matrix-update-*` in `folder` an hour old (up
/// to 512 MiB each, kept for good).
pub fn cleanup_downloads(folder: &Path) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name
            .to_string_lossy()
            .starts_with(".axross-files-matrix-update-")
        {
            continue;
        }
        let old = entry
            .metadata()
            .ok()
            .filter(|m| m.file_type().is_file())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > std::time::Duration::from_secs(3600));
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// The arguments the new program is started with: it waits for this one's
/// name and takes over when this one ends.
pub const RESTART_ARGS: [&str; 1] = ["--after-update"];

/// Start the program at *path* when it is still the file checked, and
/// watch its start for up to *wait*: Ok once its window is up (or it still
/// runs after *wait*), Err when it ended before - then [`roll_back`]. The
/// caller closes this one after Ok.
pub fn restart(path: &Path, sha256: &[u8; 32], wait: std::time::Duration) -> Result<(), String> {
    if sha256_of(path).map_err(|e| e.to_string())? != *sha256 {
        return Err("the new program changed since it was checked; not started".into());
    }
    let mut command = std::process::Command::new(path);
    command
        .args(RESTART_ARGS)
        .stdin(std::process::Stdio::null());
    let mut ready = crate::detach::ready_pipe(&mut command);
    let child = command.spawn();
    if let Some(r) = ready.as_mut() {
        r.spawned();
    }
    let mut child = child.map_err(|e| format!("{}: {e}", path.display()))?;
    // its word once its window is up; without one, a while of it running
    let watch = match ready.map(|r| r.wait(wait)) {
        Some(crate::detach::Readiness::Up | crate::detach::Readiness::Starting) => return Ok(()),
        Some(crate::detach::Readiness::Ended) => std::time::Duration::from_secs(2),
        None => wait,
    };
    let end = std::time::Instant::now() + watch;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(format!("the new program ended at its start ({status})"));
            }
            Ok(None) if std::time::Instant::now() < end => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => return Ok(()),
        }
    }
}

/// The old program (`.previous`) put back in place of the new one, which
/// did not start.
pub fn roll_back(installed: &Installed) -> Result<(), String> {
    // (never put in place by itself again)
    refuse(&installed.path, installed.version);
    if !fs::symlink_metadata(&installed.previous).is_ok_and(|m| m.file_type().is_file()) {
        return Err(format!("{} is not there", installed.previous.display()));
    }
    crate::fspath::rename_over(&installed.previous, &installed.path)
        .map_err(|e| format!("{}: {e}", installed.path.display()))
}

/// `<name>` with a suffix (beside the program).
fn beside(target: &Path, suffix: &str) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    target.with_file_name(name)
}

/// The releases that did not start here (`<name>.refused`, beside the
/// program: a version a line): never put in place by themselves again.
pub fn refused_of(target: &Path) -> PathBuf {
    beside(target, ".refused")
}

/// *version* noted as one that did not start here (its self-test failed,
/// or it ended at its start and the old program was put back).
pub fn refuse(target: &Path, version: Version) {
    if refused(target).contains(&version) {
        return;
    }
    let path = refused_of(target);
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{version}");
    }
}

/// The versions noted by [`refuse`].
pub fn refused(target: &Path) -> Vec<Version> {
    let mut text = String::new();
    if let Ok(file) = File::open(refused_of(target)) {
        let _ = file.take(64 << 10).read_to_string(&mut text);
    }
    text.lines().filter_map(Version::parse).collect()
}

/// One copy at a time putting a program in place: `<name>.update-lock`,
/// made new (one left by a copy that ended meanwhile, ten minutes old,
/// is taken over), removed when dropped.
struct Lock(PathBuf);

impl Lock {
    fn take(target: &Path) -> Result<Lock, String> {
        let path = beside(target, ".update-lock");
        for _ in 0..2 {
            match create_new(&path) {
                Ok(_) => return Ok(Lock(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = fs::symlink_metadata(&path)
                        .ok()
                        .filter(|m| m.is_file())
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > Duration::from_secs(600));
                    if !stale {
                        return Err(
                            "another axross-files-matrix is putting an update in place".into()
                        );
                    }
                    let _ = fs::remove_file(&path);
                }
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
        Err("another axross-files-matrix is putting an update in place".into())
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// The new program's first start, watched: `<name>.first-start` says the
/// version put in place and whether its first start has begun. Written when
/// a program is put in place; at the start of a program of that version,
/// "installed" becomes "starting"; the first frame drawn removes it
/// ([`first_start_done`]). A start that finds "starting" of its own version
/// follows one that never drew its window: the old program is put back
/// (an update put in place by itself would otherwise run at the next start
/// with nothing watching).
pub fn first_start_of(target: &Path) -> PathBuf {
    beside(target, ".first-start")
}

/// The marker for *installed*'s first start.
pub fn mark_first_start(installed: &Installed) {
    let _ = fs::write(
        first_start_of(&installed.path),
        format!("{}\ninstalled\n", installed.version),
    );
}

/// What a program's start found of its first start.
#[derive(Debug, PartialEq, Eq)]
pub enum FirstStart {
    /// Nothing to watch.
    None,
    /// This is the first start of a program put in place: watched.
    Watched,
    /// The first start before this one never drew its window: the old
    /// program is back in place (and this version refused) - it is to run
    /// instead.
    RolledBack(PathBuf),
}

/// At the start of the program at *target* (version *current*): see
/// [`first_start_of`].
pub fn first_start(target: &Path, current: Version) -> FirstStart {
    let marker = first_start_of(target);
    let Ok(text) = fs::read_to_string(&marker) else {
        return FirstStart::None;
    };
    let mut lines = text.lines();
    let version = lines.next().and_then(Version::parse);
    let state = lines.next().unwrap_or("");
    if version != Some(current) {
        // (another version runs: the marker is not this one's)
        let _ = fs::remove_file(&marker);
        return FirstStart::None;
    }
    if state == "installed" {
        let _ = fs::write(&marker, format!("{current}\nstarting\n"));
        return FirstStart::Watched;
    }
    let _ = fs::remove_file(&marker);
    let previous = previous_of(target);
    if !fs::symlink_metadata(&previous).is_ok_and(|m| m.file_type().is_file()) {
        return FirstStart::None;
    }
    refuse(target, current);
    match crate::fspath::rename_over(&previous, target) {
        Ok(()) => FirstStart::RolledBack(target.to_path_buf()),
        Err(_) => FirstStart::None,
    }
}

/// The first frame drawn: the first start went well.
pub fn first_start_done(target: &Path) {
    let marker = first_start_of(target);
    if fs::read_to_string(&marker).is_ok_and(|t| t.lines().nth(1) == Some("starting")) {
        let _ = fs::remove_file(&marker);
    }
}

/// How the updates are looked for (Options): not at all, said when one is
/// out (the default), or fetched, checked and put in place by themselves -
/// never a restart: the new version runs from the next start, or at once
/// when asked (*Restart now*).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateMode {
    Off,
    Notify,
    Install,
}

impl UpdateMode {
    pub const ALL: [UpdateMode; 3] = [UpdateMode::Off, UpdateMode::Notify, UpdateMode::Install];

    pub fn key(self) -> &'static str {
        match self {
            UpdateMode::Off => "off",
            UpdateMode::Notify => "notify",
            UpdateMode::Install => "install",
        }
    }

    pub fn from_key(key: &str) -> Option<UpdateMode> {
        UpdateMode::ALL.into_iter().find(|m| m.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            UpdateMode::Off => "Off",
            UpdateMode::Notify => "Say when one is out",
            UpdateMode::Install => "Download and install by themselves",
        }
    }
}

/// How often the updates are looked for while the program runs (and at
/// its start).
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);
/// After a check that could not be made (no network, a metered one, an
/// error): looked for again this much later.
pub const CHECK_AGAIN: Duration = Duration::from_secs(3600);

/// Whether a check is due: at the start (none made yet), then every
/// [`CHECK_EVERY`] after the last one; [`CHECK_AGAIN`] after one that
/// could not be made.
pub fn check_due(since_last: Option<Duration>, last_failed: bool) -> bool {
    match since_last {
        None => true,
        Some(d) if last_failed => d >= CHECK_AGAIN,
        Some(d) => d >= CHECK_EVERY,
    }
}

/// What the system says of the network (NetworkManager, where there is one).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Network {
    /// No connection to the internet known (None: not said).
    pub offline: Option<bool>,
    /// Paid by the amount (a phone's hotspot, a mobile line).
    pub metered: Option<bool>,
}

/// Whether the background checks and downloads may use the network: not
/// when it is said to be offline or metered (a check asked for by hand is
/// always made).
pub fn network_allows(net: Network) -> bool {
    net.offline != Some(true) && net.metered != Some(true)
}

/// What NetworkManager says (Linux; nothing elsewhere or without it):
/// `Connectivity` (1: none) and `Metered` (1 yes, 3 guessed yes).
pub fn network() -> Network {
    #[cfg(target_os = "linux")]
    {
        let ask = || -> Option<Network> {
            let conn = zbus::blocking::Connection::system().ok()?;
            let proxy = zbus::blocking::Proxy::new(
                &conn,
                "org.freedesktop.NetworkManager",
                "/org/freedesktop/NetworkManager",
                "org.freedesktop.NetworkManager",
            )
            .ok()?;
            let connectivity: Option<u32> = proxy.get_property("Connectivity").ok();
            let metered: Option<u32> = proxy.get_property("Metered").ok();
            Some(Network {
                offline: connectivity.map(|c| c == 1),
                metered: metered.map(|m| m == 1 || m == 3),
            })
        };
        ask().unwrap_or_default()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Network::default()
    }
}

/// Whether the start check may run: the setting, unless
/// [`NO_CHECK_ENV`] says no.
pub fn start_check_allowed(setting: bool, env: Option<&str>) -> bool {
    let off = env.is_some_and(|v| {
        ["1", "true", "yes", "on"].contains(&v.trim().to_ascii_lowercase().as_str())
    });
    setting && !off
}

#[cfg(test)]
mod tests;
