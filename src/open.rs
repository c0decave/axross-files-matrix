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

/// Extensions of what a default program may run (or install).
const RUNNABLE: &[&str] = &[
    "desktop",
    "sh",
    "bash",
    "zsh",
    "csh",
    "ksh",
    "fish",
    "py",
    "pyw",
    "pl",
    "rb",
    "php",
    "jar",
    "exe",
    "com",
    "bat",
    "cmd",
    "msi",
    "msp",
    "scr",
    "pif",
    "ps1",
    "psm1",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "hta",
    "cpl",
    "lnk",
    "url",
    "reg",
    "inf",
    "app",
    "command",
    "tool",
    "workflow",
    "terminal",
    "appimage",
    "run",
    "bin",
    "deb",
    "rpm",
    "apk",
    "pkg",
    "dmg",
    "flatpakref",
    "snap",
    "elf",
    "out",
    "so",
    "dylib",
    "dll",
    "kext",
];

/// Why opening `path` with the default program needs a question first
/// (None: it does not). `untrusted` names a medium it lies on (a removable
/// one, a network file system). Reads the file's mode (off the window's
/// thread).
pub fn risk_of(path: &Path, untrusted: Option<&str>) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok();
    if meta.as_ref().is_some_and(|m| m.is_dir()) {
        // (a folder opens in the file manager)
        return untrusted.map(|u| format!("it lies on {u}"));
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if let Some(ext) = name.rsplit_once('.').map(|(_, e)| e.to_string())
        && RUNNABLE.contains(&ext.as_str())
    {
        return Some(match ext.as_str() {
            "desktop" => "it is a desktop entry: opening it can run the command in it".into(),
            "lnk" | "url" => "it is a shortcut: opening it can run what it points to".into(),
            _ => format!("it is a program or a script (.{ext}): opening it can run it"),
        });
    }
    #[cfg(unix)]
    if let Some(m) = &meta {
        use std::os::unix::fs::PermissionsExt;
        if m.is_file() && m.permissions().mode() & 0o111 != 0 {
            return Some("it is executable: opening it can run it".into());
        }
        if m.file_type().is_symlink() {
            return Some("it is a symbolic link: it may open something else".into());
        }
    }
    untrusted.map(|u| format!("it lies on {u}, which this machine does not control"))
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
        let file = |name: &str, mode: u32| {
            let p = d.join(name);
            std::fs::write(&p, b"x").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
            p
        };
        assert!(
            risk_of(&file("evil.desktop", 0o644), None)
                .unwrap()
                .contains("desktop entry")
        );
        assert!(
            risk_of(&file("run.sh", 0o644), None)
                .unwrap()
                .contains("script")
        );
        assert!(
            risk_of(&file("tool", 0o755), None)
                .unwrap()
                .contains("executable")
        );
        assert!(risk_of(&file("SETUP.EXE", 0o644), None).is_some());
        assert_eq!(risk_of(&file("notes.txt", 0o644), None), None);
        assert!(
            risk_of(&file("photo.jpg", 0o644), Some("a removable medium"))
                .unwrap()
                .contains("removable")
        );
        std::os::unix::fs::symlink(d.join("notes.txt"), d.join("link.txt")).unwrap();
        assert!(risk_of(&d.join("link.txt"), None).unwrap().contains("link"));
        assert_eq!(risk_of(d, None), None, "a folder opens in the file manager");
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
