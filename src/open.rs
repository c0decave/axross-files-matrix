//! Opening what the city shows: in axross (when it is installed: `axross
//! <path>`, or the remote profile's URL), or with the system's default
//! program (`xdg-open`, `open`, Explorer). The program and its one
//! argument are passed as they are - never through a shell - and the path
//! is always absolute (it can never be read as an option).

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

/// axross, if it is on the PATH.
pub fn axross() -> Option<PathBuf> {
    crate::mcp::on_path("axross")
}

/// The system's way to open a path with its default program.
pub fn system(path: &Path) -> Option<Launch> {
    if !path.is_absolute() {
        return None;
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

/// axross opening a local path.
pub fn in_axross(path: &Path) -> Option<Launch> {
    if !path.is_absolute() {
        return None;
    }
    axross().map(|p| Launch {
        program: p,
        args: vec![path.as_os_str().to_os_string()],
        label: "axross".into(),
    })
}

/// A remote file's address for axross: `<protocol>://<profile><path>`
/// (the profile's name, its path percent-encoded where needed).
pub fn profile_url(protocol: &str, profile: &str, path: &str) -> String {
    let enc = |s: &str, keep_slash: bool| {
        let mut out = String::new();
        for b in s.bytes() {
            let plain =
                b.is_ascii_alphanumeric() || b"-._~".contains(&b) || (keep_slash && b == b'/');
            if plain {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
        out
    };
    let p = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    format!(
        "{}://{}{}",
        enc(protocol, false),
        enc(profile, false),
        enc(&p, true)
    )
}

/// axross opening a remote profile's path.
pub fn remote_in_axross(protocol: &str, profile: &str, path: &str) -> Option<Launch> {
    axross().map(|p| Launch {
        program: p,
        args: vec![OsString::from(profile_url(protocol, profile, path))],
        label: "axross".into(),
    })
}

/// Starts it; Err with what went wrong. A program that ends at once with a
/// failure (axross without a path argument, say) is said too: it is
/// watched for a moment on a thread of its own, `done` hears the outcome.
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
    fn urls_are_encoded() {
        assert_eq!(
            profile_url("sftp", "my box", "/srv/a b/ü.txt"),
            "sftp://my%20box/srv/a%20b/%C3%BC.txt"
        );
        assert_eq!(profile_url("s3", "bucket", "key"), "s3://bucket/key");
    }

    #[test]
    fn relative_paths_are_never_passed() {
        assert!(system(Path::new("-rf")).is_none());
        assert!(in_axross(Path::new("relative/x")).is_none());
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
