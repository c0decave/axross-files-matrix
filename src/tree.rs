//! What a walk found: one tree per island, its nodes in an arena (a node's
//! id is its index; children only ever after their parent). The walkers
//! allocate the ids - each batch says where it starts - and the window
//! applies the batches in order, so both agree without a lock.
//!
//! Every directory knows the bytes, files and newest change below it (kept
//! up to date as entries arrive: the city's treemap and the glow read them).
//! Names are kept as the system gives them (`OsStr`: bytes that are no
//! UTF-8, newlines and the like are kept, and shown escaped).

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

pub type NodeId = u32;
/// No node (the root's parent).
pub const NONE: NodeId = u32::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kind {
    Dir = 0,
    File = 1,
    /// A symbolic link (never followed: no loops).
    Link = 2,
    /// A device, a socket, a pipe.
    Other = 3,
}

impl Kind {
    pub fn from_u8(v: u8) -> Option<Kind> {
        Some(match v {
            0 => Kind::Dir,
            1 => Kind::File,
            2 => Kind::Link,
            3 => Kind::Other,
            _ => return None,
        })
    }
}

/// How far a directory has been read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Listing {
    /// Not read yet (lazily, when the camera comes near, or past a cap).
    Unlisted = 0,
    /// Asked for; the answer is on its way.
    Pending = 1,
    /// Read in part (a huge folder past the per-folder cap).
    Partial = 2,
    Listed = 3,
    /// Permission denied.
    Denied = 4,
    /// Another error (gone meanwhile, I/O, a remote error).
    Failed = 5,
    /// Not entered on purpose: another file system's mountpoint (its own
    /// island), or another device.
    Skipped = 6,
}

impl Listing {
    pub fn from_u8(v: u8) -> Option<Listing> {
        Some(match v {
            0 => Listing::Unlisted,
            1 => Listing::Pending,
            2 => Listing::Partial,
            3 => Listing::Listed,
            4 => Listing::Denied,
            5 => Listing::Failed,
            6 => Listing::Skipped,
            _ => return None,
        })
    }
    pub fn label(self) -> &'static str {
        match self {
            Listing::Unlisted => "not read yet",
            Listing::Pending => "reading",
            Listing::Partial => "read in part",
            Listing::Listed => "read",
            Listing::Denied => "permission denied",
            Listing::Failed => "could not be read",
            Listing::Skipped => "another file system",
        }
    }
}

/// What kind of file, by its name: the building's colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FileType {
    Folder = 0,
    Code = 1,
    Document = 2,
    Image = 3,
    Video = 4,
    Audio = 5,
    Archive = 6,
    Binary = 7,
    Data = 8,
    Config = 9,
    Other = 10,
}

pub const FILE_TYPES: [FileType; 11] = [
    FileType::Folder,
    FileType::Code,
    FileType::Document,
    FileType::Image,
    FileType::Video,
    FileType::Audio,
    FileType::Archive,
    FileType::Binary,
    FileType::Data,
    FileType::Config,
    FileType::Other,
];

impl FileType {
    pub fn from_u8(v: u8) -> FileType {
        FILE_TYPES
            .get(v as usize)
            .copied()
            .unwrap_or(FileType::Other)
    }
    pub fn label(self) -> &'static str {
        match self {
            FileType::Folder => "folder",
            FileType::Code => "source code",
            FileType::Document => "document",
            FileType::Image => "image",
            FileType::Video => "video",
            FileType::Audio => "audio",
            FileType::Archive => "archive",
            FileType::Binary => "program / library",
            FileType::Data => "data",
            FileType::Config => "configuration",
            FileType::Other => "other",
        }
    }

    /// By the name's extension (any case).
    pub fn of_name(name: &OsStr) -> FileType {
        let bytes = name.as_encoded_bytes();
        let Some(dot) = bytes.iter().rposition(|&b| b == b'.') else {
            return FileType::Other;
        };
        if dot == 0 || bytes.len() - dot > 12 {
            // a dot file without extension (".bashrc") is configuration
            return if dot == 0 {
                FileType::Config
            } else {
                FileType::Other
            };
        }
        let mut ext = [0u8; 12];
        let raw = &bytes[dot + 1..];
        for (o, b) in ext.iter_mut().zip(raw) {
            *o = b.to_ascii_lowercase();
        }
        let ext = &ext[..raw.len()];
        match ext {
            b"rs" | b"c" | b"h" | b"cc" | b"cpp" | b"hpp" | b"cxx" | b"py" | b"js" | b"ts"
            | b"tsx" | b"jsx" | b"go" | b"java" | b"kt" | b"swift" | b"rb" | b"php" | b"sh"
            | b"bash" | b"zsh" | b"pl" | b"lua" | b"cs" | b"scala" | b"hs" | b"ml" | b"wgsl"
            | b"glsl" | b"sql" | b"html" | b"css" | b"scss" | b"vue" | b"m" | b"mm" | b"s"
            | b"asm" | b"zig" | b"nim" | b"dart" | b"ex" | b"exs" | b"erl" | b"clj" | b"r"
            | b"jl" | b"f90" | b"pyi" | b"mk" | b"cmake" => FileType::Code,
            b"txt" | b"md" | b"rst" | b"pdf" | b"doc" | b"docx" | b"odt" | b"rtf" | b"tex"
            | b"epub" | b"xls" | b"xlsx" | b"ods" | b"ppt" | b"pptx" | b"odp" | b"org"
            | b"adoc" | b"man" | b"1" | b"info" => FileType::Document,
            b"png" | b"jpg" | b"jpeg" | b"gif" | b"bmp" | b"webp" | b"tif" | b"tiff" | b"svg"
            | b"ico" | b"heic" | b"avif" | b"raw" | b"cr2" | b"nef" | b"psd" | b"xcf" => {
                FileType::Image
            }
            b"mp4" | b"mkv" | b"avi" | b"mov" | b"webm" | b"wmv" | b"flv" | b"m4v" | b"mpg"
            | b"mpeg" | b"ts2" | b"3gp" => FileType::Video,
            b"mp3" | b"flac" | b"ogg" | b"opus" | b"wav" | b"aac" | b"m4a" | b"wma" | b"aiff"
            | b"aif" | b"alac" | b"mid" | b"midi" => FileType::Audio,
            b"zip" | b"tar" | b"gz" | b"tgz" | b"bz2" | b"xz" | b"zst" | b"7z" | b"rar" | b"lz"
            | b"lz4" | b"lzma" | b"cab" | b"deb" | b"rpm" | b"jar" | b"whl" | b"crate" | b"iso"
            | b"img" | b"dmg" | b"apk" | b"appimage" | b"snap" | b"pack" | b"qcow2" | b"vmdk"
            | b"vdi" => FileType::Archive,
            b"so" | b"dll" | b"exe" | b"o" | b"a" | b"lib" | b"dylib" | b"bin" | b"elf"
            | b"wasm" | b"pyc" | b"class" | b"rlib" | b"rmeta" | b"ko" | b"efi" | b"sys"
            | b"msi" => FileType::Binary,
            b"json" | b"csv" | b"tsv" | b"xml" | b"db" | b"sqlite" | b"sqlite3" | b"parquet"
            | b"arrow" | b"h5" | b"hdf5" | b"npy" | b"npz" | b"pkl" | b"log" | b"dat"
            | b"jsonl" | b"ndjson" | b"avro" | b"idx" | b"mdb" => FileType::Data,
            b"toml" | b"yaml" | b"yml" | b"ini" | b"conf" | b"cfg" | b"lock" | b"env"
            | b"properties" | b"plist" | b"desktop" | b"service" | b"rules" | b"config"
            | b"gitignore" | b"editorconfig" => FileType::Config,
            _ => FileType::Other,
        }
    }
}

/// One file, folder or link.
#[derive(Clone, Debug)]
pub struct Node {
    pub name: Box<OsStr>,
    pub parent: NodeId,
    pub kind: Kind,
    pub listing: Listing,
    pub ftype: FileType,
    /// The entry's own size in bytes (a folder's own entry size is not
    /// counted in its total).
    pub size: u64,
    /// The bytes it takes on the disk (files).
    pub alloc: u64,
    /// Bytes below (folders), the size (files).
    pub total: u64,
    /// Files below (folders), 1 (files and links).
    pub count: u32,
    /// Seconds since 1970 (0: unknown).
    pub mtime: i64,
    /// The newest change below (folders), the mtime (files).
    pub newest: i64,
    /// A hash of the name (the building's variety).
    pub hash: u32,
    pub children: Vec<NodeId>,
}

/// An entry as a walker found it.
#[derive(Clone, Debug)]
pub struct Entry {
    pub parent: NodeId,
    pub name: Box<OsStr>,
    pub kind: Kind,
    pub size: u64,
    /// The bytes it takes on the disk (a sparse file takes fewer than its
    /// size; where it is not known, its size).
    pub alloc: u64,
    pub mtime: i64,
}

/// A walker's word on how far a folder has been read.
#[derive(Clone, Copy, Debug)]
pub struct ListingUpdate {
    pub dir: NodeId,
    pub listing: Listing,
}

/// A batch of new entries (ids from `first_id` on, in order) and listing
/// states.
#[derive(Clone, Debug, Default)]
pub struct Batch {
    pub first_id: NodeId,
    pub entries: Vec<Entry>,
    pub listings: Vec<ListingUpdate>,
}

#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub nodes: Vec<Node>,
}

impl Tree {
    /// A tree with a root folder named `name`.
    pub fn new(name: &OsStr, mtime: i64) -> Tree {
        Tree {
            nodes: vec![Node {
                name: name.into(),
                parent: NONE,
                kind: Kind::Dir,
                listing: Listing::Unlisted,
                ftype: FileType::Folder,
                size: 0,
                alloc: 0,
                total: 0,
                count: 0,
                mtime,
                newest: mtime,
                hash: crate::math::hash32(name.as_encoded_bytes()),
                children: Vec::new(),
            }],
        }
    }

    /// A tree with room for `n` nodes made at once (untouched memory: used
    /// only as they come - no copy of a big tree while it grows).
    pub fn with_room(name: &OsStr, n: usize) -> Tree {
        let mut t = Tree::new(name, 0);
        t.nodes.reserve(n);
        t
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    pub fn root(&self) -> &Node {
        &self.nodes[0]
    }
    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id as usize)
    }

    /// Adds one entry; its id, or None when its parent is no folder of this
    /// tree (a batch that does not belong here).
    pub fn push(&mut self, e: Entry) -> Option<NodeId> {
        let id = self.nodes.len() as NodeId;
        if id == NONE {
            return None;
        }
        let parent = self.nodes.get(e.parent as usize)?;
        if parent.kind != Kind::Dir {
            return None;
        }
        let ftype = match e.kind {
            Kind::Dir => FileType::Folder,
            _ => FileType::of_name(&e.name),
        };
        let counted = e.kind != Kind::Dir;
        let (total, count) = if counted { (e.size, 1) } else { (0, 0) };
        let hash = crate::math::hash32(e.name.as_encoded_bytes());
        self.nodes.push(Node {
            name: e.name,
            parent: e.parent,
            kind: e.kind,
            listing: if e.kind == Kind::Dir {
                Listing::Unlisted
            } else {
                Listing::Listed
            },
            ftype,
            size: e.size,
            alloc: e.alloc,
            total,
            count,
            mtime: e.mtime,
            newest: e.mtime,
            hash,
            children: Vec::new(),
        });
        self.nodes[e.parent as usize].children.push(id);
        // the folders above: bytes, files and the newest change
        let mut up = e.parent;
        while up != NONE {
            let n = &mut self.nodes[up as usize];
            n.total = n.total.saturating_add(total);
            n.count = n.count.saturating_add(count);
            n.newest = n.newest.max(e.mtime);
            up = n.parent;
        }
        Some(id)
    }

    /// Applies a batch; false (and nothing applied) when it does not start
    /// where this tree ends.
    pub fn apply(&mut self, batch: Batch) -> bool {
        if batch.first_id as usize != self.nodes.len() {
            return false;
        }
        for e in batch.entries {
            if self.push(e).is_none() {
                return false;
            }
        }
        for l in batch.listings {
            if let Some(n) = self.nodes.get_mut(l.dir as usize)
                && n.kind == Kind::Dir
            {
                n.listing = l.listing;
            }
        }
        true
    }

    /// The names from the root down to `id` (the root's own name first).
    pub fn chain(&self, id: NodeId) -> Vec<&OsStr> {
        let mut out = Vec::new();
        let mut at = id;
        while let Some(n) = self.nodes.get(at as usize) {
            out.push(&*n.name);
            at = n.parent;
        }
        out.reverse();
        out
    }

    /// The path of `id` below `root` (a local folder).
    pub fn local_path(&self, root: &std::path::Path, id: NodeId) -> PathBuf {
        let mut p = root.to_path_buf();
        for name in self.chain(id).into_iter().skip(1) {
            p.push(name);
        }
        p
    }

    /// The path of `id` below `root` with '/' between the names (a remote
    /// backend's paths).
    pub fn remote_path(&self, root: &str, id: NodeId) -> String {
        let mut p = root.trim_end_matches('/').to_string();
        let chain = self.chain(id);
        if chain.len() <= 1 {
            return if p.is_empty() {
                "/".into()
            } else {
                root.into()
            };
        }
        for name in chain.into_iter().skip(1) {
            p.push('/');
            p.push_str(&name.to_string_lossy());
        }
        p
    }

    /// The path of `id` below `root` as raw bytes (names that are no UTF-8
    /// kept as the server gave them: reads and opens find the very file).
    pub fn remote_path_bytes(&self, root: &str, id: NodeId) -> Vec<u8> {
        let mut p = root.trim_end_matches('/').as_bytes().to_vec();
        let chain = self.chain(id);
        if chain.len() <= 1 {
            return if p.is_empty() {
                b"/".to_vec()
            } else {
                root.as_bytes().to_vec()
            };
        }
        for name in chain.into_iter().skip(1) {
            p.push(b'/');
            p.extend_from_slice(&os_to_bytes(name));
        }
        p
    }

    /// How deep `id` lies (the root: 0).
    pub fn depth(&self, id: NodeId) -> usize {
        let mut d = 0;
        let mut at = self.nodes.get(id as usize).map_or(NONE, |n| n.parent);
        while at != NONE {
            d += 1;
            at = self.nodes[at as usize].parent;
        }
        d
    }

    /// Whether `a` is `b` or lies below it.
    pub fn is_within(&self, a: NodeId, b: NodeId) -> bool {
        let mut at = a;
        while at != NONE {
            if at == b {
                return true;
            }
            at = self.nodes.get(at as usize).map_or(NONE, |n| n.parent);
        }
        false
    }

    /// Recomputes every folder's totals from the files (after a cache was
    /// read: what it says is not trusted).
    pub fn recount(&mut self) {
        for n in &mut self.nodes {
            if n.kind == Kind::Dir {
                n.total = 0;
                n.count = 0;
                n.newest = n.mtime;
            } else {
                n.total = n.size;
                n.count = 1;
                n.newest = n.mtime;
            }
        }
        // children always come after their parent: backwards is bottom-up
        for i in (1..self.nodes.len()).rev() {
            let (total, count, newest, parent) = {
                let n = &self.nodes[i];
                (n.total, n.count, n.newest, n.parent)
            };
            if let Some(p) = self.nodes.get_mut(parent as usize) {
                p.total = p.total.saturating_add(total);
                p.count = p.count.saturating_add(count);
                p.newest = p.newest.max(newest);
            }
        }
    }
}

/// A name as it is shown: lossy UTF-8, control characters (a newline in a
/// file name) as visible escapes, never more than `max` characters.
pub fn display_name(name: &OsStr, max: usize) -> String {
    // (bytes that are no UTF-8 shown as \xNN - exact, and drawn with any
    // font: U+FFFD has no glyph in egui's)
    let raw = os_to_bytes(name);
    let mut out = String::with_capacity(raw.len().min(max.saturating_mul(4)));
    let mut n = 0usize;
    let mut push = |out: &mut String, s: &str| -> bool {
        if n >= max {
            out.push('…');
            return false;
        }
        n += 1;
        out.push_str(s);
        true
    };
    for chunk in raw.utf8_chunks() {
        for c in chunk.valid().chars() {
            let piece = match c {
                '\n' => "\\n".to_string(),
                '\r' => "\\r".to_string(),
                '\t' => "\\t".to_string(),
                c if c.is_control() => format!("\\u{{{:x}}}", c as u32),
                // (bidirectional overrides and invisible formatting: shown)
                '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
                | '\u{feff}' => {
                    format!("\\u{{{:x}}}", c as u32)
                }
                c => c.to_string(),
            };
            if !push(&mut out, &piece) {
                return out;
            }
        }
        for b in chunk.invalid() {
            if !push(&mut out, &format!("\\x{b:02x}")) {
                return out;
            }
        }
    }
    out
}

/// Text made safe to show in a label (a path, a link's target): control
/// characters, invisible formatting and bytes that are no UTF-8 escaped.
pub fn display_text(text: &OsStr) -> String {
    display_name(text, usize::MAX)
}

/// An `OsString` from raw bytes as a remote backend or the cache gives them
/// (on Windows: UTF-8, lossily).
pub fn os_from_bytes(bytes: &[u8]) -> OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        OsStr::from_bytes(bytes).to_os_string()
    }
    #[cfg(not(unix))]
    {
        OsString::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// Bytes `os_from_bytes` turns back into the same name (Unix: the raw
/// bytes; elsewhere UTF-8, lossily).
pub fn os_to_bytes(name: &OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        name.as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        name.to_string_lossy().into_owned().into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(parent: NodeId, name: &str, kind: Kind, size: u64, mtime: i64) -> Entry {
        Entry {
            parent,
            name: OsStr::new(name).into(),
            kind,
            size,
            alloc: size,
            mtime,
        }
    }

    #[test]
    fn totals_follow_the_entries() {
        let mut t = Tree::new(OsStr::new("/"), 10);
        let a = t.push(entry(0, "a", Kind::Dir, 4096, 5)).unwrap();
        t.push(entry(a, "x.rs", Kind::File, 100, 50)).unwrap();
        t.push(entry(0, "y.png", Kind::File, 7, 20)).unwrap();
        assert_eq!(t.root().total, 107);
        assert_eq!(t.root().count, 2);
        assert_eq!(t.root().newest, 50);
        assert_eq!(t.nodes[a as usize].total, 100);
        assert_eq!(t.nodes[2].ftype, FileType::Code);
        assert_eq!(t.nodes[3].ftype, FileType::Image);
        let before: Vec<_> = t
            .nodes
            .iter()
            .map(|n| (n.total, n.count, n.newest))
            .collect();
        t.recount();
        let after: Vec<_> = t
            .nodes
            .iter()
            .map(|n| (n.total, n.count, n.newest))
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn a_batch_must_start_where_the_tree_ends() {
        let mut t = Tree::new(OsStr::new("r"), 0);
        let bad = Batch {
            first_id: 5,
            entries: vec![entry(0, "a", Kind::File, 1, 0)],
            listings: vec![],
        };
        assert!(!t.apply(bad));
        assert_eq!(t.len(), 1);
        let good = Batch {
            first_id: 1,
            entries: vec![entry(0, "a", Kind::File, 1, 0)],
            listings: vec![ListingUpdate {
                dir: 0,
                listing: Listing::Listed,
            }],
        };
        assert!(t.apply(good));
        assert_eq!(t.root().listing, Listing::Listed);
        // a file is no parent
        assert!(t.push(entry(1, "b", Kind::File, 1, 0)).is_none());
    }

    #[test]
    fn odd_names_are_shown_escaped() {
        assert_eq!(display_name(OsStr::new("a\nb"), 80), "a\\nb");
        assert_eq!(display_name(OsStr::new("x\u{202e}y"), 80), "x\\u{202e}y");
        assert_eq!(display_name(OsStr::new("abcdef"), 3), "abc…");
        #[cfg(unix)]
        {
            let raw = os_from_bytes(b"caf\xe9");
            assert_eq!(
                display_name(&raw, 80),
                "caf\\xe9",
                "exact, and drawable with any font"
            );
            assert_eq!(os_to_bytes(&raw), b"caf\xe9");
        }
    }

    #[test]
    fn file_types_by_extension() {
        assert_eq!(FileType::of_name(OsStr::new("A.TAR.GZ")), FileType::Archive);
        assert_eq!(FileType::of_name(OsStr::new(".bashrc")), FileType::Config);
        assert_eq!(FileType::of_name(OsStr::new("Makefile")), FileType::Other);
        assert_eq!(
            FileType::of_name(OsStr::new("x.verylongextension")),
            FileType::Other
        );
    }

    #[test]
    fn paths_from_the_chain() {
        let mut t = Tree::new(OsStr::new("root"), 0);
        let a = t.push(entry(0, "a", Kind::Dir, 0, 0)).unwrap();
        let f = t.push(entry(a, "f", Kind::File, 0, 0)).unwrap();
        assert_eq!(t.remote_path("/data", f), "/data/a/f");
        assert_eq!(t.remote_path("/", f), "/a/f");
        assert_eq!(t.remote_path("/", 0), "/");
        assert_eq!(
            t.local_path(std::path::Path::new("/x"), f),
            PathBuf::from("/x/a/f")
        );
        assert_eq!(t.depth(f), 2);
        assert!(t.is_within(f, a));
        assert!(!t.is_within(a, f));
    }
}
