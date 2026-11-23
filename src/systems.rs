//! The systems of axross's connection manager - its saved profiles - as
//! teleport targets: their families (a sign and a colour for the common
//! kinds; any other protocol, a new one included, gets the generic look and
//! still works), their state, and the filter of the Teleport menu.
//!
//! The protocols themselves come from axross's registry at run time (MCP
//! `list_remote_profiles`); nothing here lists them. A family is only how
//! an island looks and whether its folders are read as they are approached
//! (a mail store or a database lists slowly).

use std::time::Instant;

/// A kind of system: how its island looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    /// A shell login (SSH, SFTP, SCP, rsync, telnet, WinRM).
    Shell,
    /// A file share (NFS, SMB, DFS, iSCSI, FTP, WebDAV).
    Share,
    /// A cloud store or bucket (S3, Azure, Google Drive, Dropbox, OneDrive).
    Cloud,
    /// A phone or a device on a cable (ADB, MTP).
    Phone,
    /// A mailbox or newsgroups (IMAP, POP3, Exchange, NNTP).
    Mail,
    /// A database or a directory (SQLite, PostgreSQL, MongoDB, Redis, LDAP).
    Database,
    /// Version control (git, svn).
    Vcs,
    /// An old or odd network service (gopher, TFTP, printers, SLP).
    Legacy,
    /// Memory of axross's own (RAM disk).
    Memory,
    /// Anything else: a protocol this program does not know (yet).
    Generic,
}

impl Family {
    /// The family of a protocol id as axross's registry names it.
    pub fn of(protocol: &str) -> Family {
        match protocol.to_ascii_lowercase().as_str() {
            "sftp" | "ssh" | "scp" | "rsync" | "rsh" | "telnet" | "cisco-telnet" | "winrm"
            | "wmi" => Family::Shell,
            "nfs" | "smb" | "cifs" | "dfsn" | "iscsi" | "ftp" | "ftps" | "webdav" | "afp" => {
                Family::Share
            }
            "s3" | "azure_blob" | "azure_files" | "gdrive" | "dropbox" | "onedrive"
            | "sharepoint" | "gcs" | "b2" | "swift" => Family::Cloud,
            "adb" | "mtp" | "ios" => Family::Phone,
            "imap" | "pop3" | "exchange" | "nntp" | "jmap" => Family::Mail,
            "sqlite" | "postgres" | "mysql" | "mongodb" | "redis" | "ldap" => Family::Database,
            "git" | "svn" | "hg" => Family::Vcs,
            "gopher" | "tftp" | "pjl" | "slp" => Family::Legacy,
            "ramfs" | "ram" | "memory" => Family::Memory,
            _ => Family::Generic,
        }
    }

    /// A short sign (drawn with any font).
    pub fn sign(self) -> &'static str {
        match self {
            Family::Shell => ">_",
            Family::Share => "[share]",
            Family::Cloud => "(cloud)",
            Family::Phone => "[phone]",
            Family::Mail => "@",
            Family::Database => "[db]",
            Family::Vcs => "<vcs>",
            Family::Legacy => "[old]",
            Family::Memory => "[ram]",
            Family::Generic => "[net]",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Family::Shell => "shell login",
            Family::Share => "file share",
            Family::Cloud => "cloud store",
            Family::Phone => "phone or device",
            Family::Mail => "mail",
            Family::Database => "database",
            Family::Vcs => "version control",
            Family::Legacy => "old network service",
            Family::Memory => "memory",
            Family::Generic => "remote system",
        }
    }

    /// Its colour (sRGB): calm, none of them a saturated red.
    pub fn color(self) -> [u8; 3] {
        match self {
            Family::Shell => [95, 215, 255],
            Family::Share => [135, 215, 135],
            Family::Cloud => [135, 175, 255],
            Family::Phone => [215, 135, 255],
            Family::Mail => [255, 215, 95],
            Family::Database => [255, 175, 95],
            Family::Vcs => [175, 135, 255],
            Family::Legacy => [175, 175, 135],
            Family::Memory => [135, 255, 215],
            Family::Generic => [150, 170, 160],
        }
    }

    /// Whether its folders are read only as they are approached (it lists
    /// slowly, or cannot walk a tree in reasonable time).
    pub fn lazy(self) -> bool {
        matches!(self, Family::Mail | Family::Database | Family::Legacy)
    }
}

/// Where a system stands.
#[derive(Clone, Debug, PartialEq)]
pub enum SysState {
    NotOpen,
    Connecting,
    Walking,
    Open,
    Failed(String),
    /// A password or key passphrase axross has not stored.
    NeedsSecret,
    /// axross cannot open it over MCP (an interactive sign-in, a blocked
    /// protocol): why.
    Unavailable(String),
}

impl SysState {
    pub fn label(&self) -> String {
        match self {
            SysState::NotOpen => "not open".into(),
            SysState::Connecting => "connecting…".into(),
            SysState::Walking => "walking".into(),
            SysState::Open => "open".into(),
            SysState::Failed(why) => format!("failed: {why}"),
            SysState::NeedsSecret => "needs unlocking in axross".into(),
            SysState::Unavailable(why) => format!("cannot be opened here: {why}"),
        }
    }
}

/// What the window keeps of a system it is opening or opened.
#[derive(Clone, Debug)]
pub struct Live {
    pub state: SysState,
    /// Its island (by id), once raised.
    pub island: Option<u64>,
    /// The connect under way: when it started, its number (a late answer
    /// of a cancelled one is dropped).
    pub since: Option<Instant>,
    pub attempt: u64,
    /// When it was last teleported to (the least recently visited is
    /// offered for closing when the server's slots are full).
    pub visited: Instant,
}

impl Default for Live {
    fn default() -> Live {
        Live {
            state: SysState::NotOpen,
            island: None,
            since: None,
            attempt: 0,
            visited: Instant::now(),
        }
    }
}

/// Whether `query` (any case, any part; words all of them) matches any of
/// `fields`. An empty query matches everything.
pub fn matches(query: &str, fields: &[&str]) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    let hay: String = fields.join(" ").to_lowercase();
    q.split_whitespace().all(|w| hay.contains(w))
}

/// How long a connect may take before it is given up.
pub const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// How long a connect is waited for in the background after the window
/// gave up on it (a late success is closed on the server).
pub const LATE_ANSWER: std::time::Duration = std::time::Duration::from_secs(600);
/// How long a jump to a spot inside a system waits for its district.
pub const SPOT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_and_the_generic_look() {
        assert_eq!(Family::of("sftp"), Family::Shell);
        assert_eq!(Family::of("NFS"), Family::Share);
        assert_eq!(Family::of("s3"), Family::Cloud);
        assert_eq!(Family::of("adb"), Family::Phone);
        assert_eq!(Family::of("imap"), Family::Mail);
        assert_eq!(Family::of("sqlite"), Family::Database);
        assert_eq!(Family::of("git"), Family::Vcs);
        // a protocol of tomorrow: the generic look, still a system
        assert_eq!(Family::of("quantum-ftp"), Family::Generic);
        assert!(Family::of("imap").lazy() && Family::of("sqlite").lazy());
        assert!(!Family::of("sftp").lazy() && !Family::of("x").lazy());
        for f in [Family::Shell, Family::Mail, Family::Generic] {
            let [r, g, b] = f.color();
            assert!(!(r > 200 && g < 80 && b < 80), "no saturated red");
        }
    }

    #[test]
    fn the_filter_takes_every_word_in_any_field() {
        assert!(matches("", &["x"]));
        assert!(matches("NAS sftp", &["nas", "sftp", "203.0.113.5"]));
        assert!(!matches("nas s3", &["nas", "sftp"]));
        assert!(matches("203.0", &["box", "sftp", "203.0.113.5"]));
    }
}
