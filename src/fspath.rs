//! Small helpers for paths and renames.

use std::ffi::OsStr;
use std::path::Path;

/// An argument or a name as text (lossily: for messages and options; the
/// paths themselves are kept as the system gives them).
pub fn os_to_string(s: &OsStr) -> String {
    s.to_string_lossy().into_owned()
}

/// `from` renamed over `to`. On Windows a file another program holds open
/// for a moment without sharing its deletion (a virus scanner reading the
/// new file, say) refuses the rename with "access denied": it is tried
/// again for up to half a second.
pub fn rename_over(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Err(e)
                if cfg!(windows)
                    && e.kind() == std::io::ErrorKind::PermissionDenied
                    && tries < 25 =>
            {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            done => return done,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_over_an_existing_file() {
        let dir = crate::testdir::Scratch::new("rename");
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::write(&a, b"new").unwrap();
        std::fs::write(&b, b"old").unwrap();
        rename_over(&a, &b).unwrap();
        assert_eq!(std::fs::read(&b).unwrap(), b"new");
        assert!(!a.exists());
    }
}
