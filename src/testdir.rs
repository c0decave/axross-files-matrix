//! A folder of a test's own under the temporary directory, emptied first
//! and removed when the test is done (also when it fails): tests leave
//! nothing behind on a RAM disk.

use std::ffi::OsStr;
use std::ops::Deref;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("afm-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<OsStr> for Scratch {
    fn as_ref(&self) -> &OsStr {
        self.0.as_os_str()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
