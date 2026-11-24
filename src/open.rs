//! Opening what the city shows: in axross, or with the system's default
//! program, or in the file manager. The program and its arguments are
//! passed as they are - never through a shell - and a path always comes
//! after `--` or is absolute: it can never be read as an option.
//!
//! - **axross** is used only when it can open a path: `axross --help` names
//!   `--profile` and `PATH` (axross 0.15.186 and later; probed once, off the
//!   window's thread, and remembered for that program file). An older one
//!   is said to be too old - nothing falls through to another program.
//! - **The default program** is guarded: a program, a script, a desktop
//!   entry, a shortcut, anything with the executable bit, and anything on a
//!   removable medium or a network file system is opened only after a
//!   question that says what may happen; showing it in the file manager is
//!   the safe answer offered first.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How a thing is opened: the program and its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub label: String,
}

/// What is known of axross.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Axross {
    /// Still being looked for.
    Looking,
    /// Not on the PATH.
    Missing,
    /// There, but it cannot open a path (older than 0.15.186).
    TooOld(PathBuf),
    Ready(PathBuf),
}

/// axross, if it is on the PATH.
pub fn axross_on_path() -> Option<PathBuf> {
    crate::mcp::on_path("axross")
}

/// Whether the axross at `path` opens paths: its `--help` names
/// `--profile` and `PATH` (run with a time limit; nothing through a shell).
pub fn axross_opens_paths(path: &Path) -> bool {
    let help = axross_help(path);
    help.contains("--profile") && help.contains("PATH")
}

/// What `axross --help` says (empty when it cannot be run; a time limit,
/// nothing through a shell).
pub fn axross_help(path: &Path) -> String {
    let mut cmd = Command::new(path);
    cmd.arg("--help")
        .env("QT_QPA_PLATFORM", "offscreen")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let Ok(mut child) = cmd.spawn() else {
        return String::new();
    };
    let Some(mut out) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return String::new();
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("afm-axross-help".into())
        .spawn(move || {
            use std::io::Read;
            let mut text = Vec::new();
            let _ = (&mut out).take(1 << 20).read_to_end(&mut text);
            let _ = tx.send(text);
        });
    let text = rx.recv_timeout(Duration::from_secs(20)).unwrap_or_default();
    let _ = child.kill();
    let _ = child.wait();
    String::from_utf8_lossy(&text).into_owned()
}

/// A program file's identity for the probe's memory: its size and time.
pub fn file_stamp(path: &Path) -> Option<(u64, i64)> {
    let m = std::fs::metadata(path).ok()?;
    let t = m
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    Some((m.len(), t))
}

/// axross opening a local path: `axross -- PATH`.
pub fn axross_local(axross: &Path, path: &Path) -> Launch {
    Launch {
        program: axross.to_path_buf(),
        args: vec!["--".into(), path.as_os_str().to_os_string()],
        label: "axross".into(),
    }
}

/// A saved profile's name as axross is handed it: no control characters,
/// not empty, not too long.
pub fn valid_profile(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 256 && !name.chars().any(|c| c.is_control())
}

/// axross opening a remote path on a saved profile:
/// `axross --profile=NAME -- PATH` (the name glued to its option, the path
/// after `--`: neither can be read as an option, whatever they hold).
pub fn axross_remote(axross: &Path, profile: &str, path: &[u8]) -> Result<Launch, String> {
    if !valid_profile(profile) {
        return Err("the profile's name cannot be handed to axross".into());
    }
    if path.is_empty() || path.contains(&0) {
        return Err("the path cannot be handed to axross".into());
    }
    let mut p = OsString::from("--profile=");
    p.push(profile);
    Ok(Launch {
        program: axross.to_path_buf(),
        args: vec![p, "--".into(), crate::tree::os_from_bytes(path)],
        label: "axross".into(),
    })
}

/// The system's way to open a path with its default program.
pub fn system(path: &Path) -> Option<Launch> {
    system_via(None, path)
}

/// [`system`], with this opener program in place of the system's (tests).
pub fn system_via(opener: Option<&Path>, path: &Path) -> Option<Launch> {
    if !path.is_absolute() {
        return None;
    }
    if let Some(o) = opener {
        return Some(Launch {
            program: o.to_path_buf(),
            args: vec![path.as_os_str().to_os_string()],
            label: "the default program".into(),
        });
    }
    let arg = path.as_os_str().to_os_string();
    #[cfg(target_os = "macos")]
    {
        Some(Launch {
            program: PathBuf::from("/usr/bin/open"),
            args: vec![arg],
            label: "the default program".into(),
        })
    }
    #[cfg(windows)]
    {
        let windir = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        Some(Launch {
            program: windir.join("explorer.exe"),
            args: vec![arg],
            label: "the default program".into(),
        })
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        crate::mcp::on_path("xdg-open").map(|p| Launch {
            program: p,
            args: vec![arg],
            label: "the default program".into(),
        })
    }
}

/// Showing a path in the file manager (its folder, it selected where the
/// system can): never runs it.
pub fn file_manager(path: &Path) -> Option<Launch> {
    file_manager_via(None, path)
}

/// [`file_manager`], with this opener program in place of the system's
/// (tests): it is given the folder.
pub fn file_manager_via(opener: Option<&Path>, path: &Path) -> Option<Launch> {
    if !path.is_absolute() {
        return None;
    }
    if let Some(o) = opener {
        return Some(Launch {
            program: o.to_path_buf(),
            args: vec![path.parent().unwrap_or(path).as_os_str().to_os_string()],
            label: "the file manager".into(),
        });
    }
    #[cfg(target_os = "macos")]
    {
        Some(Launch {
            program: PathBuf::from("/usr/bin/open"),
            args: vec!["-R".into(), path.as_os_str().to_os_string()],
            label: "the file manager".into(),
        })
    }
    #[cfg(windows)]
    {
        let windir = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let mut select = OsString::from("/select,");
        select.push(path.as_os_str());
        Some(Launch {
            program: windir.join("explorer.exe"),
            args: vec![select],
            label: "the file manager".into(),
        })
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        // (the folder: a folder is never run)
        let folder = path.parent().unwrap_or(path);
        crate::mcp::on_path("xdg-open").map(|p| Launch {
            program: p,
            args: vec![folder.as_os_str().to_os_string()],
            label: "the file manager".into(),
        })
    }
}

/// What a type of file is, by its extension: what opens with the default
/// program without a question (everyday files that run nothing when
/// opened), by family - their first bytes must still agree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Text,
    Image,
    Audio,
    Video,
    Pdf,
    /// Office files that are zip archives (docx, odt, epub ...).
    OfficeZip,
    /// The old office formats (doc, xls, ppt: OLE2), rtf.
    OfficeOld,
    Archive,
}

/// The everyday types: text, source code, configuration, documents (the
/// old office ones too), archives, pictures, sound, video. Everything else
/// asks first (an allow-list: a type nobody thought of is asked about,
/// not opened).
const SAFE: &[(Family, &[&str])] = &[
    (
        Family::Text,
        &[
            // plain text and markup
            "txt",
            "text",
            "md",
            "markdown",
            "rst",
            "adoc",
            "asciidoc",
            "org",
            "log",
            "csv",
            "tsv",
            "json",
            "jsonl",
            "ndjson",
            "yaml",
            "yml",
            "toml",
            "ini",
            "cfg",
            "conf",
            "properties",
            "env",
            "xml",
            "xsd",
            "xsl",
            "dtd",
            "plist",
            "tex",
            "bib",
            "sty",
            "cls",
            "srt",
            "vtt",
            "sub",
            "ass",
            "diff",
            "patch",
            "nfo",
            "lst",
            "po",
            "pot",
            // source code (opened in an editor; nothing runs)
            "rs",
            "c",
            "h",
            "cc",
            "cpp",
            "cxx",
            "hpp",
            "hh",
            "hxx",
            "m",
            "mm",
            "go",
            "java",
            "kt",
            "kts",
            "scala",
            "swift",
            "cs",
            "fs",
            "fsi",
            "vb",
            "dart",
            "zig",
            "nim",
            "d",
            "hs",
            "ml",
            "mli",
            "ex",
            "exs",
            "erl",
            "hrl",
            "clj",
            "cljs",
            "lisp",
            "el",
            "scm",
            "rkt",
            "lua",
            "r",
            "jl",
            "sql",
            "css",
            "scss",
            "sass",
            "less",
            "ts",
            "tsx",
            "jsx",
            "mjs",
            "cjs",
            "vue",
            "svelte",
            "proto",
            "graphql",
            "gql",
            "cmake",
            "mk",
            "mak",
            "gradle",
            "sbt",
            "cabal",
            "nix",
            "tf",
            "hcl",
            "dockerfile",
            "lock",
            "gitignore",
            "gitattributes",
            "editorconfig",
            "asm",
            "s",
            "v",
            "sv",
            "vhdl",
            "f",
            "f90",
            "pas",
            "ada",
            "adb",
            "ads",
            "cob",
            "tcl",
            "awk",
            "sed",
            "ipynb",
        ],
    ),
    (
        Family::Image,
        &[
            "png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff", "heic", "heif", "avif",
            "ico", "cr2", "cr3", "nef", "arw", "dng", "raf", "orf", "rw2", "psd", "xcf", "kra",
            "exr", "hdr", "tga", "pcx", "ppm", "pgm", "pbm", "pnm", "jxl", "jp2", "qoi",
        ],
    ),
    (
        Family::Audio,
        &[
            "mp3", "flac", "ogg", "oga", "opus", "wav", "aac", "m4a", "aiff", "aif", "wma", "mid",
            "midi", "ape", "wv", "alac", "amr", "au", "mka",
        ],
    ),
    (
        Family::Video,
        &[
            "mp4", "m4v", "mkv", "webm", "mov", "avi", "mpg", "mpeg", "ogv", "wmv", "flv", "3gp",
            "m2ts", "mts", "vob",
        ],
    ),
    (Family::Pdf, &["pdf", "djvu", "xps"]),
    (
        Family::OfficeZip,
        &[
            "epub", "odt", "ods", "odp", "odg", "docx", "xlsx", "pptx", "pages", "numbers", "key",
        ],
    ),
    (
        Family::OfficeOld,
        &["doc", "xls", "ppt", "rtf", "wpd", "wps"],
    ),
    (
        Family::Archive,
        &[
            "zip", "tar", "gz", "tgz", "bz2", "tbz", "tbz2", "xz", "txz", "zst", "lz", "lz4",
            "lzma", "7z", "rar", "cpio", "z",
        ],
    ),
];

/// Names without a type that are everyday text (README, LICENSE ...).
const TEXT_NAMES: &[&str] = &[
    "readme",
    "license",
    "licence",
    "copying",
    "copyright",
    "notice",
    "authors",
    "contributors",
    "changelog",
    "changes",
    "news",
    "history",
    "install",
    "todo",
    "makefile",
    "gnumakefile",
    "dockerfile",
    "containerfile",
    "vagrantfile",
    "gemfile",
    "rakefile",
    "procfile",
    "jenkinsfile",
    "justfile",
    "cmakelists.txt",
    "manifest",
    "codeowners",
    "version",
];

/// Types that run code or change the system when opened: said so, and
/// always asked about (never "do not ask again").
const RUNS: &[&str] = &[
    "exe",
    "com",
    "bat",
    "cmd",
    "msi",
    "msp",
    "mst",
    "msix",
    "msixbundle",
    "appx",
    "appxbundle",
    "appinstaller",
    "ps1",
    "psm1",
    "psd1",
    "ps1xml",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "wsc",
    "sct",
    "hta",
    "scr",
    "pif",
    "cpl",
    "msc",
    "chm",
    "hlp",
    "jar",
    "jnlp",
    "application",
    "appref-ms",
    "gadget",
    "reg",
    "inf",
    "lnk",
    "url",
    "scf",
    "settingcontent-ms",
    "library-ms",
    "search-ms",
    "searchconnector-ms",
    "website",
    "diagcab",
    "xll",
    "xlam",
    "ppam",
    "docm",
    "dotm",
    "xlsm",
    "xltm",
    "pptm",
    "potm",
    "ppsm",
    "sldm",
    "iso",
    "img",
    "vhd",
    "vhdx",
    "desktop",
    "sh",
    "bash",
    "zsh",
    "csh",
    "ksh",
    "fish",
    "command",
    "tool",
    "app",
    "pkg",
    "mpkg",
    "dmg",
    "deb",
    "rpm",
    "run",
    "bin",
    "appimage",
    "flatpak",
    "flatpakref",
    "snap",
    "py",
    "pyw",
    "pyz",
    "pl",
    "rb",
    "php",
    "scpt",
    "applescript",
    "action",
    "workflow",
    "mobileconfig",
    "terminal",
    "webloc",
    "inetloc",
    "kext",
    "xpi",
    "crx",
    "vsix",
    "apk",
    "xapk",
    "ipa",
    "elf",
    "out",
    "so",
    "dll",
    "dylib",
    "sys",
    "drv",
    "ocx",
    "mde",
    "accde",
    "ade",
    "adp",
    "mda",
    "ins",
    "isp",
    "shb",
    "shs",
    "ws",
];

/// The family of a lower-case extension (None: not an everyday type).
pub fn family_of(ext: &str) -> Option<Family> {
    SAFE.iter()
        .find(|(_, exts)| exts.contains(&ext))
        .map(|(f, _)| *f)
}

/// What a file's first bytes say it is, when that is something that runs.
pub fn sniff_runs(head: &[u8]) -> Option<&'static str> {
    let text = String::from_utf8_lossy(&head[..head.len().min(4096)]);
    if head.starts_with(b"#!") {
        Some("it is a script (it starts with #!): opening it can run it")
    } else if text.contains("[Desktop Entry]") {
        Some("it is a desktop entry (by its content): opening it can run the command in it")
    } else if head.starts_with(b"\x7fELF")
        || head.starts_with(b"MZ")
        || head.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
        || head.starts_with(&[0xce, 0xfa, 0xed, 0xfe])
        || head.starts_with(&[0xca, 0xfe, 0xba, 0xbe])
    {
        Some("it is a program (by its content): opening it can run it")
    } else {
        None
    }
}

/// Content that does not belong in an allowed type's family, where that is
/// a risk: a web page under a picture's name (it may open in a browser and
/// run its scripts), a zip under a picture's or a text's name (an archive
/// that may hold anything), an old Office file (OLE2: macros) under any
/// name but an old Office one. (A web page in a text file, a zip in a
/// docx: as expected.)
pub fn sniff_mismatch(family: Family, ext: &str, head: &[u8]) -> Option<String> {
    let start: Vec<u8> = head
        .iter()
        .skip_while(|b| b.is_ascii_whitespace() || **b == 0xef || **b == 0xbb || **b == 0xbf)
        .take(64)
        .map(u8::to_ascii_lowercase)
        .collect();
    let html = start.starts_with(b"<!doctype html")
        || start.starts_with(b"<html")
        || start.starts_with(b"<script")
        || start.starts_with(b"<svg") && family != Family::Text;
    let zip = head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06");
    let ole2 = head.starts_with(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]);
    let content = if html && family != Family::Text {
        "a web page (it may open in a browser and run its scripts)"
    } else if zip && !matches!(family, Family::OfficeZip | Family::Archive) {
        "a zip archive (it may hold anything)"
    } else if ole2 && family != Family::OfficeOld {
        "an old Office file (it can hold macros)"
    } else {
        return None;
    };
    Some(format!(
        "its content is {content}, not what a .{ext} file holds"
    ))
}

/// Why opening a file with the default program needs a question first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Risk {
    pub why: String,
    /// A type this program does not know (its extension, lower case): the
    /// question may be switched off for it. None: always asked.
    pub unknown: Option<String>,
}

impl Risk {
    fn always(why: impl Into<String>) -> Risk {
        Risk {
            why: why.into(),
            unknown: None,
        }
    }
}

/// Why opening `path` with the default program needs a question first
/// (None: it does not). `untrusted` names a medium it lies on (a removable
/// one, a network file system); `trusted` are the types the user said not
/// to be asked about again. Reads the file's mode and first bytes (off the
/// window's thread).
pub fn risk_of(path: &Path, untrusted: Option<&str>, trusted: &[String]) -> Option<Risk> {
    let meta = std::fs::symlink_metadata(path).ok();
    if meta.as_ref().is_some_and(|m| m.is_dir()) {
        // (a folder opens in the file manager)
        return untrusted.map(|u| Risk::always(format!("it lies on {u}")));
    }
    if let Some(m) = &meta {
        if m.file_type().is_symlink() {
            return Some(Risk::always(
                "it is a symbolic link: it may open something else",
            ));
        }
        if !m.is_file() {
            // a FIFO, a socket, a device: opening one can hang or read a
            // device
            return Some(Risk::always(
                "it is no regular file (a pipe, a socket or a device): opening it can hang or \
                 touch a device",
            ));
        }
    }
    #[cfg(unix)]
    if let Some(m) = &meta {
        use std::os::unix::fs::PermissionsExt;
        if m.permissions().mode() & 0o7111 != 0 {
            return Some(Risk::always("it is executable: opening it can run it"));
        }
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    // (no trimming: "x.desktop." and "x.desktop " are no "desktop")
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_string())
        .unwrap_or_default();
    let family = if TEXT_NAMES.contains(&name.as_str()) {
        Some(Family::Text)
    } else {
        family_of(&ext)
    };
    if family.is_none() {
        if RUNS.contains(&ext.as_str()) {
            return Some(Risk::always(match ext.as_str() {
                "desktop" => "it is a desktop entry: opening it can run the command in it".into(),
                "lnk" | "url" | "webloc" | "inetloc" | "appref-ms" | "application" | "scf" => {
                    "it is a shortcut: opening it can run what it points to".into()
                }
                e => format!("files of type .{e} can run code or change the system when opened"),
            }));
        }
        let valid = !ext.is_empty()
            && ext.len() <= 16
            && ext
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !(valid && trusted.contains(&ext)) {
            return Some(if valid {
                Risk {
                    why: format!(
                        "it is of a type this program does not know (.{ext}): opening it may \
                         run it"
                    ),
                    unknown: Some(ext),
                }
            } else {
                Risk::always("it has no type the system can tell safely: opening it may run it")
            });
        }
    }
    // an allowed (or trusted) type: its first bytes must agree
    if let Ok(head) = crate::preview::read_regular(path, 4096) {
        if let Some(why) = sniff_runs(&head) {
            return Some(Risk::always(why));
        }
        if let Some(f) = family
            && let Some(why) = sniff_mismatch(f, &ext, &head)
        {
            return Some(Risk::always(why));
        }
    }
    untrusted.map(|u| {
        Risk::always(format!(
            "it lies on {u}, which this machine does not control"
        ))
    })
}

/// Starts it; Err with what went wrong. A program that ends at once with a
/// failure is said too: it is watched for a moment on a thread of its own,
/// `done` hears the outcome.
pub fn spawn(l: &Launch, done: impl FnOnce(Result<(), String>) + Send + 'static) {
    let mut cmd = Command::new(&l.program);
    cmd.args(&l.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let label = l.label.clone();
    match cmd.spawn() {
        Err(e) => done(Err(format!("{label} could not be started: {e}"))),
        Ok(mut child) => {
            let _ = std::thread::Builder::new()
                .name("afm-open".into())
                .spawn(move || {
                    let start = Instant::now();
                    while start.elapsed() < Duration::from_millis(2500) {
                        match child.try_wait() {
                            Ok(Some(status)) if !status.success() => {
                                done(Err(format!("{label} ended at once ({status})")));
                                return;
                            }
                            Ok(Some(_)) => break,
                            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                            Err(_) => break,
                        }
                    }
                    done(Ok(()));
                    // (still running: it is its own program now; reaped when it ends)
                    let _ = child.wait();
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_name_and_no_path_can_become_an_option() {
        let l = axross_remote(Path::new("/usr/bin/axross"), "-x", b"-rf /").unwrap();
        assert_eq!(
            l.args,
            vec![OsString::from("--profile=-x"), "--".into(), "-rf /".into()]
        );
        let l = axross_local(Path::new("/usr/bin/axross"), Path::new("/srv/-x"));
        assert_eq!(l.args, vec![OsString::from("--"), "/srv/-x".into()]);
        assert!(axross_remote(Path::new("/a"), "bad\nname", b"/x").is_err());
        assert!(axross_remote(Path::new("/a"), "  ", b"/x").is_err());
        assert!(axross_remote(Path::new("/a"), "p", b"").is_err());
        #[cfg(unix)]
        {
            // a name that is no UTF-8 reaches axross as its bytes
            use std::os::unix::ffi::OsStrExt;
            let l = axross_remote(Path::new("/a"), "p", b"/bad\xff.txt").unwrap();
            assert_eq!(l.args[2].as_bytes(), b"/bad\xff.txt");
        }
    }

    #[test]
    fn relative_paths_are_never_passed() {
        assert!(system(Path::new("-rf")).is_none());
        assert!(file_manager(Path::new("relative/x")).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn programs_scripts_and_untrusted_media_need_a_question() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::testdir::Scratch::new("risk");
        let d = dir.path();
        let file = |name: &str, mode: u32, body: &[u8]| {
            let p = d.join(name);
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
            p
        };
        assert!(
            risk_of(&file("evil.desktop", 0o644, b"x"), None, &[])
                .unwrap()
                .why
                .contains("desktop entry")
        );
        assert!(risk_of(&file("run.sh", 0o644, b"x"), None, &[]).is_some());
        assert!(
            risk_of(&file("tool", 0o755, b"x"), None, &[])
                .unwrap()
                .why
                .contains("executable")
        );
        assert!(risk_of(&file("SETUP.EXE", 0o644, b"x"), None, &[]).is_some());
        assert_eq!(
            risk_of(&file("notes.txt", 0o644, b"plain"), None, &[]),
            None
        );
        assert_eq!(
            risk_of(&file("photo.JPG", 0o644, b"\xff\xd8\xff"), None, &[]),
            None
        );
        assert!(
            risk_of(
                &file("photo.jpg", 0o644, b"x"),
                Some("a removable medium"),
                &[]
            )
            .unwrap()
            .why
            .contains("removable")
        );
        std::os::unix::fs::symlink(d.join("notes.txt"), d.join("link.txt")).unwrap();
        assert!(
            risk_of(&d.join("link.txt"), None, &[])
                .unwrap()
                .why
                .contains("link")
        );
        assert_eq!(
            risk_of(d, None, &[]),
            None,
            "a folder opens in the file manager"
        );
        // what a deny-list missed: asked about now
        for name in [
            "a.msc",
            "a.chm",
            "a.xll",
            "a.application",
            "a.appref-ms",
            "a.settingcontent-ms",
            "a.library-ms",
            "a.scf",
            "a.wsc",
            "a.sct",
            "a.diagcab",
            "a.gadget",
            "a.msix",
            "a.jnlp",
            "a.iso",
            "a.docm",
            "a.xlsm",
            "a.inetloc",
            "a.webloc",
            "a.scpt",
            "a.action",
            "a.mobileconfig",
            "x.desktop.",
            "x.desktop ",
            "noextension",
        ] {
            assert!(
                risk_of(&file(name, 0o644, b"data"), None, &[]).is_some(),
                "{name}"
            );
        }
        // by content: a desktop entry or a script under a harmless name
        assert!(
            risk_of(
                &file("readme", 0o644, b"[Desktop Entry]\nExec=sh"),
                None,
                &[]
            )
            .is_some()
        );
        assert!(
            risk_of(
                &file("hello.txt", 0o644, b"[Desktop Entry]\nExec=sh"),
                None,
                &[]
            )
            .unwrap()
            .why
            .contains("content")
        );
        assert!(
            risk_of(&file("go.txt", 0o644, b"#!/bin/sh\nrm -rf ~"), None, &[])
                .unwrap()
                .why
                .contains("script")
        );
        assert!(
            risk_of(&file("pic.png", 0o644, b"\x7fELF\x02"), None, &[])
                .unwrap()
                .why
                .contains("program")
        );
        assert!(risk_of(&file("setuid.txt", 0o4644, b"x"), None, &[]).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn everyday_files_open_unknown_types_say_so_and_can_be_trusted() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::testdir::Scratch::new("risk-everyday");
        let d = dir.path();
        let file = |name: &str, body: &[u8]| {
            let p = d.join(name);
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
            p
        };
        // everyday names that used to ask: all open now
        for name in [
            "README",
            "LICENSE",
            "Makefile",
            "x.rs",
            "x.c",
            "x.h",
            "x.go",
            "x.java",
            "x.css",
            "x.xml",
            "x.ini",
            "x.conf",
            "x.cfg",
            "x.doc",
            "x.xls",
            "x.ppt",
            "x.rtf",
            "x.zip",
            "x.tar.gz",
            "x.srt",
            "x.diff",
            "x.tex",
            "x.ts",
            "x.toml",
            ".gitignore",
            "x.7z",
            "x.heic",
            "x.flac",
            "x.mkv",
            "x.pdf",
            "x.odt",
            "x.epub",
        ] {
            let body: &[u8] = match name {
                "x.zip" | "x.odt" | "x.epub" => b"PK\x03\x04rest",
                "x.doc" | "x.xls" | "x.ppt" => b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1rest",
                _ => b"plain words",
            };
            assert_eq!(risk_of(&file(name, body), None, &[]), None, "{name}");
        }
        // a type nobody listed: asked, said to be unknown, and can be trusted
        let odd = file("song.xyz", b"data");
        let r = risk_of(&odd, None, &[]).expect("asked");
        assert!(r.why.contains("does not know (.xyz)"), "{}", r.why);
        assert_eq!(r.unknown.as_deref(), Some("xyz"));
        assert_eq!(risk_of(&odd, None, &["xyz".to_string()]), None);
        // what runs is never "do not ask again", trusted or not
        let exe = file("setup.exe", b"data");
        let r = risk_of(&exe, None, &["exe".to_string()]).expect("asked");
        assert_eq!(r.unknown, None);
        assert!(r.why.contains("can run code"));
        // a trusted type whose content is a program still asks
        assert!(
            risk_of(&file("a.xyz", b"\x7fELF\x02"), None, &["xyz".to_string()])
                .unwrap()
                .why
                .contains("program")
        );
    }

    #[cfg(unix)]
    #[test]
    fn special_files_and_content_under_a_false_name_ask() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::testdir::Scratch::new("risk-special");
        let d = dir.path();
        let file = |name: &str, body: &[u8]| {
            let p = d.join(name);
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
            p
        };
        // a FIFO named like text: asked, never trusted away
        let fifo = d.join("fifo.txt");
        let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid C string; mkfifo only creates the node
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
        let r = risk_of(&fifo, None, &["txt".to_string()]).expect("asked");
        assert!(r.why.contains("no regular file"), "{}", r.why);
        assert_eq!(r.unknown, None);
        // a socket too
        let sock = d.join("s.txt");
        let _l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        assert!(
            risk_of(&sock, None, &[])
                .unwrap()
                .why
                .contains("no regular file")
        );
        // content that does not belong under the name
        for (name, body, says) in [
            (
                "g.png",
                &b"<!DOCTYPE html><script>x()</script>"[..],
                "web page",
            ),
            ("g2.jpg", &b"  <html><body>"[..], "web page"),
            ("m.jpg", &b"PK\x03\x04data"[..], "zip archive"),
            ("t.txt", &b"PK\x03\x04data"[..], "zip archive"),
            (
                "k.docx",
                &b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1rest"[..],
                "old Office",
            ),
            (
                "p.pdf",
                &b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1rest"[..],
                "old Office",
            ),
        ] {
            let r = risk_of(&file(name, body), None, &[]).unwrap_or_else(|| panic!("{name}"));
            assert!(r.why.contains(says), "{name}: {}", r.why);
            assert_eq!(r.unknown, None);
        }
        // and what does belong: a web page in a text file, a zip in a docx
        assert_eq!(risk_of(&file("page.md", b"<html>"), None, &[]), None);
        assert_eq!(risk_of(&file("ok.docx", b"PK\x03\x04x"), None, &[]), None);
        assert_eq!(
            risk_of(
                &file("old.doc", b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1"),
                None,
                &[]
            ),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn axross_is_probed_for_the_path_argument() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::testdir::Scratch::new("axross-probe");
        let fake = |name: &str, help: &str| {
            let p = dir.path().join(name);
            std::fs::write(&p, format!("#!/bin/sh\nprintf '%s\\n' '{help}'\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        let new = fake("axross-new", "usage: axross [--profile NAME] [PATH ...]");
        let old = fake(
            "axross-old",
            "usage: axross [-h] [--version] [--mcp-server]",
        );
        assert!(axross_opens_paths(&new));
        assert!(!axross_opens_paths(&old));
        assert!(!axross_opens_paths(&dir.path().join("missing")));
        assert!(file_stamp(&new).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_program_is_said() {
        let (tx, rx) = std::sync::mpsc::channel();
        let l = Launch {
            program: PathBuf::from("/bin/false"),
            args: vec![],
            label: "false".into(),
        };
        if l.program.exists() {
            spawn(&l, move |r| tx.send(r).unwrap());
            let r = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(r.is_err());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        spawn(
            &Launch {
                program: PathBuf::from("/nonexistent/prog"),
                args: vec![],
                label: "x".into(),
            },
            move |r| tx.send(r).unwrap(),
        );
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap().is_err());
    }
}
