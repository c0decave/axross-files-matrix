//! Started from a terminal, the program goes to the background: it starts
//! itself again as a process of its own session (no SIGHUP when the
//! terminal closes; stdin from /dev/null, stdout and stderr into a log
//! file) and the terminal's prompt comes back. `--foreground` (`-F`) keeps
//! it attached, as before. Not from a terminal (a desktop entry, a file
//! manager - the AppImage too, from one) it simply runs; the AppImage
//! started from a terminal goes to the background as the binary does.
//!
//! The start is not lost silently: the parent waits up to two seconds for
//! the child's word that its window is up (over a pipe); a child that ends
//! before that - no display, say - has its log's last lines printed by the
//! parent, which exits 1; one still starting after two seconds goes on, the
//! parent says so and exits 0 (whether it came up is in the log).
//!
//! Windows and macOS: on Windows nothing is done here (a GUI program,
//! attached to the terminal it was started in for what it says); macOS as
//! Linux, its log in ~/Library/Caches.

use std::path::PathBuf;

/// What the process is to do.
pub enum Start {
    /// Run the program here.
    Run,
    /// End with this exit code (the program runs in the background, or its
    /// start failed and was said).
    Exit(i32),
}

/// The child's mark (set by the parent; never by hand).
pub const CHILD: &str = "AXROSS_FILES_MATRIX_DETACHED";
/// The pipe's end the child writes "ok" to once its window is up.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) const READY_FD: &str = "AXROSS_FILES_MATRIX_READY_FD";

/// Where a detached program writes what it says: `~/.cache/axross-files-matrix/
/// axross-files-matrix.log` (macOS: `~/Library/Caches/axross-files-matrix/axross-files-matrix.log`; or
/// the temporary directory's).
pub fn log_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("axross-files-matrix")
        .join("axross-files-matrix.log")
}

/// Whether this process is the detached child.
pub fn is_child() -> bool {
    std::env::var_os(CHILD).is_some()
}

/// Whether a start with `args` goes to the background: on Unix, from a
/// terminal (stdin, stdout or stderr one; AXROSS_FILES_MATRIX_ASSUME_TTY in the
/// tests), not with --foreground / -F, not as the child itself.
pub fn wanted(args: &[String]) -> bool {
    use std::io::IsTerminal;
    if !cfg!(unix) || is_child() {
        return false;
    }
    if args.iter().any(|a| a == "--foreground" || a == "-F") {
        return false;
    }
    std::env::var_os("AXROSS_FILES_MATRIX_ASSUME_TTY").is_some()
        || std::io::stdin().is_terminal()
        || std::io::stdout().is_terminal()
        || std::io::stderr().is_terminal()
}

/// The child: its window is up - the parent may go.
pub fn signal_ready() {
    #[cfg(unix)]
    if let Some(fd) = std::env::var(READY_FD)
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
    {
        use std::io::Write;
        use std::os::fd::FromRawFd;
        // (once: the variable taken away, the pipe's end closed after)
        // SAFETY: the parent made this descriptor for this process alone
        // (the pipe's write end, kept open across exec) and nothing else in
        // it uses that number; it is read once and closed here.
        unsafe { std::env::remove_var(READY_FD) };
        let mut pipe = unsafe { std::fs::File::from_raw_fd(fd) };
        let _ = pipe.write_all(b"ok\n");
    }
}

/// The parent: start the program again in the background with `args` (as
/// they came: bytes that are no UTF-8 too) and wait for its word (see the
/// module's text).
#[cfg(unix)]
pub fn spawn(args: &[std::ffi::OsString]) -> Start {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    let log = log_path();
    if let Some(dir) = log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log);
    let Ok(out) = out else {
        eprintln!(
            "axross-files-matrix: cannot write its log {} - started in the foreground",
            log.display()
        );
        return Start::Run;
    };
    let Ok(err) = out.try_clone() else {
        return Start::Run;
    };
    let Ok((mut reader, writer)) = std::io::pipe() else {
        return Start::Run;
    };
    let Ok(exe) = std::env::current_exe() else {
        return Start::Run;
    };
    let fd = writer.as_raw_fd();
    let mut command = std::process::Command::new(exe);
    command
        .args(args)
        .env(CHILD, "1")
        .env(READY_FD, fd.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(err);
    // SAFETY: between fork and exec only async-signal-safe calls: setsid
    // (a session of its own: no controlling terminal, no SIGHUP from it)
    // and fcntl (the pipe's write end kept open across exec).
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn();
    // (the parent's own copy of the write end closed: the reader sees the
    // end when the child closes its own, or ends)
    drop(writer);
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "axross-files-matrix: could not start in the background ({e}) - started here"
            );
            return Start::Run;
        }
    };
    // its word, within two seconds
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut said = String::new();
        let _ = reader.read_to_string(&mut said);
        let _ = tx.send(said);
    });
    match rx.recv_timeout(std::time::Duration::from_secs(2)) {
        Ok(said) if said.starts_with("ok") => Start::Exit(0),
        Ok(_) => {
            // (it ended without its window: why, from its log)
            let _ = child.wait();
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            let tail: Vec<&str> = text.lines().rev().take(12).collect();
            eprintln!("axross-files-matrix could not start:");
            for line in tail.into_iter().rev() {
                eprintln!("  {line}");
            }
            eprintln!("(the log: {}; --foreground runs it here)", log.display());
            Start::Exit(1)
        }
        // (still starting: it goes on; where it writes is said)
        Err(_) => {
            eprintln!(
                "axross-files-matrix is starting in the background (its log: {})",
                log.display()
            );
            Start::Exit(0)
        }
    }
}

#[cfg(not(unix))]
pub fn spawn(_args: &[std::ffi::OsString]) -> Start {
    Start::Run
}

/// What a program started with a [`ReadyPipe`] said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    /// Its window is up.
    Up,
    /// It ended (the pipe closed) without a word.
    Ended,
    /// Still starting when the wait was over.
    Starting,
}

/// The pipe a program started by this one says "ok" on once its window is
/// up ([`signal_ready`]): the updater's restart watches the new program's
/// start with it.
#[cfg(unix)]
pub struct ReadyPipe {
    reader: Option<std::io::PipeReader>,
    writer: Option<std::io::PipeWriter>,
}

/// A [`ReadyPipe`] for `command` (its write end kept open across exec,
/// named to it by [`READY_FD`]); None where there is none (not Unix).
#[cfg(unix)]
pub fn ready_pipe(command: &mut std::process::Command) -> Option<ReadyPipe> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    let (reader, writer) = std::io::pipe().ok()?;
    let fd = writer.as_raw_fd();
    // (marked as a detached child: it never goes to the background itself)
    command.env(READY_FD, fd.to_string()).env(CHILD, "1");
    // SAFETY: between fork and exec only fcntl, async-signal-safe (the
    // pipe's write end kept open across exec).
    unsafe {
        command.pre_exec(move || {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Some(ReadyPipe {
        reader: Some(reader),
        writer: Some(writer),
    })
}

#[cfg(unix)]
impl ReadyPipe {
    /// The program is started: this one's copy of the write end goes (the
    /// reader sees the end when the program closes its own, or ends).
    pub fn spawned(&mut self) {
        self.writer = None;
    }

    /// Its word, waited for at most `wait`.
    pub fn wait(mut self, wait: std::time::Duration) -> Readiness {
        use std::io::BufRead;
        let Some(reader) = self.reader.take() else {
            return Readiness::Ended;
        };
        self.writer = None;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // (its first line: the pipe may stay open after it)
            let mut said = String::new();
            let _ = std::io::BufReader::new(reader).read_line(&mut said);
            let _ = tx.send(said);
        });
        match rx.recv_timeout(wait) {
            Ok(said) if said.starts_with("ok") => Readiness::Up,
            Ok(_) => Readiness::Ended,
            Err(_) => Readiness::Starting,
        }
    }
}

#[cfg(not(unix))]
pub struct ReadyPipe;

#[cfg(not(unix))]
pub fn ready_pipe(_command: &mut std::process::Command) -> Option<ReadyPipe> {
    None
}

#[cfg(not(unix))]
impl ReadyPipe {
    pub fn spawned(&mut self) {}

    pub fn wait(self, _wait: std::time::Duration) -> Readiness {
        Readiness::Starting
    }
}
