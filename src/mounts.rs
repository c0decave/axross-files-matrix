//! The mountpoints the system shows, each an island: Linux's
//! `/proc/self/mountinfo`, macOS's `/` and `/Volumes`, Windows's logical
//! drives. Pseudo file systems (proc, sysfs, cgroup, tmpfs but `/tmp`,
//! overlay but `/`, ...) are hidden unless the setting shows them; network
//! file systems are marked, and walked lazily.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountKind {
    /// A disk, a partition, a local file system.
    Local,
    /// NFS, SMB, sshfs and the like: walked lazily (each folder on demand
    /// past the first levels).
    Network,
    /// A removable medium (macOS volumes, Windows removable drives and
    /// optical discs, Linux /media and /run/media).
    Removable,
    /// proc, sysfs, cgroup, tmpfs, overlay, ...: hidden by default.
    Pseudo,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub mount_point: PathBuf,
    pub fs_type: String,
    /// The device or the remote (`/dev/nvme0n1p2`, `server:/export`).
    pub source: String,
    /// The device numbers (Linux; 0:0 elsewhere): ties the island to its
    /// disk on the machine's island.
    pub dev: (u32, u32),
    pub read_only: bool,
    pub kind: MountKind,
}

impl Mount {
    /// A short label for the island.
    pub fn label(&self) -> String {
        let mp = self.mount_point.to_string_lossy();
        format!("{mp}  ({})", self.fs_type)
    }
}

/// File systems that are no data a person keeps.
const PSEUDO: &[&str] = &[
    "proc",
    "sysfs",
    "cgroup",
    "cgroup2",
    "devtmpfs",
    "devpts",
    "mqueue",
    "debugfs",
    "tracefs",
    "securityfs",
    "pstore",
    "bpf",
    "configfs",
    "fusectl",
    "hugetlbfs",
    "autofs",
    "binfmt_misc",
    "efivarfs",
    "rpc_pipefs",
    "nsfs",
    "ramfs",
    "selinuxfs",
    "squashfs",
    "tmpfs",
    "overlay",
    "aufs",
    "shm",
    "devfs",
    "nullfs",
    "fuse.portal",
    "fuse.gvfsd-fuse",
    "fuse.lxcfs",
    "fuse.snapfuse",
    "nfsd",
    "none",
];

const NETWORK: &[&str] = &[
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "smbfs",
    "afs",
    "ceph",
    "glusterfs",
    "9p",
    "davfs",
    "fuse.sshfs",
    "fuse.rclone",
    "fuse.s3fs",
    "fuse.davfs2",
    "fuse.gcsfuse",
    "webdav",
];

/// How a file system of `fs_type` mounted at `mount_point` is taken.
pub fn classify(fs_type: &str, mount_point: &Path) -> MountKind {
    let mp = mount_point.to_string_lossy();
    if NETWORK.contains(&fs_type) {
        return MountKind::Network;
    }
    // the root is always shown (a container's root is an overlay); /tmp is
    // data people keep for a while
    if mp == "/" {
        return MountKind::Local;
    }
    if fs_type == "tmpfs" && mp == "/tmp" {
        return MountKind::Local;
    }
    if PSEUDO.contains(&fs_type) {
        return MountKind::Pseudo;
    }
    // the system's own trees, whatever their type
    let system = ["/proc", "/sys", "/dev", "/run"];
    let removable_run = mp.starts_with("/run/media/");
    if !removable_run
        && system
            .iter()
            .any(|s| mp == *s || mp.starts_with(&format!("{s}/")))
    {
        return MountKind::Pseudo;
    }
    if mp.starts_with("/media/") || removable_run {
        return MountKind::Removable;
    }
    MountKind::Local
}

/// One field of mountinfo with its octal escapes (`\040` a space, `\012`
/// a newline, `\134` a backslash) undone - as bytes: a mountpoint's name
/// need not be UTF-8.
pub fn unescape(field: &str) -> Vec<u8> {
    let b = field.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            let v = (b[i + 1] - b'0') as u32 * 64
                + (b[i + 2] - b'0') as u32 * 8
                + (b[i + 3] - b'0') as u32;
            if v <= 255 {
                out.push(v as u8);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// The mounts in a `/proc/self/mountinfo` text (lines that do not parse are
/// skipped; a mountpoint mounted over keeps the last mount).
pub fn parse_mountinfo(text: &str) -> Vec<Mount> {
    let mut out: Vec<Mount> = Vec::new();
    for line in text.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let l: Vec<&str> = left.split(' ').collect();
        let r: Vec<&str> = right.split(' ').collect();
        if l.len() < 6 || r.len() < 3 {
            continue;
        }
        let Some((maj, min)) = l[2].split_once(':') else {
            continue;
        };
        let (Ok(maj), Ok(min)) = (maj.parse::<u32>(), min.parse::<u32>()) else {
            continue;
        };
        let mount_point = PathBuf::from(crate::tree::os_from_bytes(&unescape(l[4])));
        if !mount_point.has_root() {
            continue;
        }
        let fs_type = String::from_utf8_lossy(&unescape(r[0])).into_owned();
        let source = String::from_utf8_lossy(&unescape(r[1])).into_owned();
        let read_only = l[5].split(',').any(|o| o == "ro");
        let kind = classify(&fs_type, &mount_point);
        out.retain(|m| m.mount_point != mount_point);
        out.push(Mount {
            mount_point,
            fs_type,
            source,
            dev: (maj, min),
            read_only,
            kind,
        });
    }
    out
}

/// The mounts of this system.
pub fn system_mounts() -> Vec<Mount> {
    #[cfg(target_os = "linux")]
    {
        match std::fs::read_to_string("/proc/self/mountinfo") {
            Ok(text) => parse_mountinfo(&text),
            Err(_) => vec![root_only()],
        }
    }
    #[cfg(target_os = "macos")]
    {
        macos_volumes(Path::new("/Volumes"))
    }
    #[cfg(windows)]
    {
        windows_drives()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        vec![root_only()]
    }
}

#[allow(dead_code)]
fn root_only() -> Mount {
    Mount {
        mount_point: PathBuf::from("/"),
        fs_type: "root".into(),
        source: String::new(),
        dev: (0, 0),
        read_only: false,
        kind: MountKind::Local,
    }
}

/// macOS: the root and every volume in `/Volumes` (the link to `/` that
/// names the boot volume left out: it is the root).
pub fn macos_volumes(volumes: &Path) -> Vec<Mount> {
    let mut out = vec![Mount {
        mount_point: PathBuf::from("/"),
        fs_type: "apfs".into(),
        source: String::new(),
        dev: (0, 0),
        read_only: false,
        kind: MountKind::Local,
    }];
    let Ok(dir) = std::fs::read_dir(volumes) else {
        return out;
    };
    let mut names: Vec<_> = dir.flatten().collect();
    names.sort_by_key(|e| e.file_name());
    for e in names {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() || !ft.is_dir() {
            continue;
        }
        out.push(Mount {
            mount_point: e.path(),
            fs_type: "volume".into(),
            source: String::new(),
            dev: (0, 0),
            read_only: false,
            kind: MountKind::Removable,
        });
    }
    out
}

#[cfg(windows)]
fn windows_drives() -> Vec<Mount> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    // SAFETY: no arguments; returns a bit mask of the drive letters
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: a NUL-terminated wide string that lives through the call
        let t = unsafe { GetDriveTypeW(wide.as_ptr()) };
        // 2 removable, 3 fixed, 4 remote, 5 cd-rom, 6 ram disk
        let (kind, fs_type) = match t {
            2 => (MountKind::Removable, "removable"),
            3 => (MountKind::Local, "fixed"),
            4 => (MountKind::Network, "network"),
            5 => (MountKind::Removable, "optical"),
            6 => (MountKind::Local, "ramdisk"),
            _ => continue,
        };
        out.push(Mount {
            mount_point: PathBuf::from(root),
            fs_type: fs_type.into(),
            source: String::new(),
            dev: (0, 0),
            read_only: false,
            kind,
        });
    }
    out
}

/// The mounts to show: pseudo ones only when asked.
pub fn visible(mounts: &[Mount], show_pseudo: bool) -> Vec<Mount> {
    mounts
        .iter()
        .filter(|m| show_pseudo || m.kind != MountKind::Pseudo)
        .cloned()
        .collect()
}

/// The mountpoints strictly below `root` (a walk of `root` stops there:
/// they are islands of their own).
pub fn nested_below(mounts: &[Mount], root: &Path) -> Vec<PathBuf> {
    mounts
        .iter()
        .filter(|m| m.mount_point != root && m.mount_point.starts_with(root))
        .map(|m| m.mount_point.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
22 1 0:21 / /sys rw,nosuid,nodev,noexec,relatime shared:7 - sysfs sysfs rw
23 1 0:22 / /proc rw,nosuid,nodev,noexec,relatime shared:13 - proc proc rw
25 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw,errors=remount-ro
26 25 0:24 / /dev/shm rw,nosuid,nodev shared:3 - tmpfs tmpfs rw,inode64
30 25 259:1 / /boot/efi rw,relatime shared:31 - vfat /dev/nvme0n1p1 rw,fmask=0077
31 25 0:27 / /tmp rw,nosuid,nodev shared:32 - tmpfs tmpfs rw,size=8G
32 25 0:28 / /run/user/1000 rw,nosuid,nodev,relatime shared:33 - tmpfs tmpfs rw
40 25 0:50 / /mnt/my\\040disk rw,relatime shared:40 - ext4 /dev/sdb1 rw
41 25 0:51 / /srv/nas ro,relatime shared:41 - nfs4 nas:/export rw,vers=4.2
42 25 8:33 / /run/media/me/USB rw,relatime shared:42 - vfat /dev/sdc1 rw
43 25 0:60 / /snap/core/1 ro,nodev,relatime shared:44 - squashfs /dev/loop3 ro
44 25 0:61 / /srv/sshfs/new\\012line rw,relatime - fuse.sshfs host:/ rw
45 25 0:62 / /var/lib/docker/overlay2/x/merged rw - overlay overlay rw
garbage line without a separator
46 25 0:63 / /dup rw - ext4 /dev/sdd1 rw
47 25 0:64 / /dup rw - xfs /dev/sde1 rw
";

    #[test]
    fn parses_mountinfo() {
        let all = parse_mountinfo(SAMPLE);
        let find = |p: &str| all.iter().find(|m| m.mount_point == Path::new(p)).unwrap();
        assert_eq!(find("/").kind, MountKind::Local);
        assert_eq!(find("/").dev, (259, 2));
        assert_eq!(find("/").source, "/dev/nvme0n1p2");
        assert_eq!(find("/sys").kind, MountKind::Pseudo);
        assert_eq!(find("/proc").kind, MountKind::Pseudo);
        assert_eq!(find("/dev/shm").kind, MountKind::Pseudo);
        assert_eq!(find("/tmp").kind, MountKind::Local);
        assert_eq!(find("/run/user/1000").kind, MountKind::Pseudo);
        assert_eq!(find("/mnt/my disk").kind, MountKind::Local);
        assert_eq!(find("/srv/nas").kind, MountKind::Network);
        assert!(find("/srv/nas").read_only);
        assert_eq!(find("/run/media/me/USB").kind, MountKind::Removable);
        assert_eq!(find("/snap/core/1").kind, MountKind::Pseudo);
        assert_eq!(find("/srv/sshfs/new\nline").kind, MountKind::Network);
        assert_eq!(
            find("/var/lib/docker/overlay2/x/merged").kind,
            MountKind::Pseudo
        );
        // mounted over: the last one
        assert_eq!(find("/dup").fs_type, "xfs");
        assert_eq!(
            all.iter()
                .filter(|m| m.mount_point == Path::new("/dup"))
                .count(),
            1
        );
        let shown = visible(&all, false);
        let names: Vec<String> = shown
            .iter()
            .map(|m| m.mount_point.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "/",
                "/boot/efi",
                "/tmp",
                "/mnt/my disk",
                "/srv/nas",
                "/run/media/me/USB",
                "/srv/sshfs/new\nline",
                "/dup"
            ]
        );
        assert_eq!(visible(&all, true).len(), all.len());
    }

    #[test]
    fn a_container_root_overlay_is_shown() {
        let all = parse_mountinfo("1 0 0:131 / / ro,relatime - overlay overlay rw,lowerdir=x\n");
        assert_eq!(all[0].kind, MountKind::Local);
        assert!(all[0].read_only);
    }

    #[test]
    fn unescapes_octal_and_keeps_the_rest() {
        assert_eq!(unescape("a\\040b"), b"a b");
        assert_eq!(unescape("a\\134b"), b"a\\b");
        assert_eq!(unescape("x\\0"), b"x\\0");
        assert_eq!(unescape("\\377"), [255u8]);
        assert_eq!(unescape("\\999"), b"\\999");
    }

    #[test]
    fn nested_mounts_are_cut_out_of_a_walk() {
        let all = parse_mountinfo(SAMPLE);
        let below = nested_below(&all, Path::new("/"));
        assert!(below.contains(&PathBuf::from("/tmp")));
        assert!(!below.contains(&PathBuf::from("/")));
        let below_boot = nested_below(&all, Path::new("/boot"));
        assert_eq!(below_boot, vec![PathBuf::from("/boot/efi")]);
    }

    #[test]
    fn macos_volumes_skip_the_link_to_the_root() {
        let dir = crate::testdir::Scratch::new("volumes");
        std::fs::create_dir(dir.path().join("Backup")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/", dir.path().join("Macintosh HD")).unwrap();
        let v = macos_volumes(dir.path());
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].mount_point, dir.path().join("Backup"));
        assert_eq!(v[1].kind, MountKind::Removable);
    }
}
