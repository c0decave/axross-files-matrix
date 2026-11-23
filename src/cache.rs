//! The last walk of each island, kept under the cache folder
//! (`$XDG_CACHE_HOME/axross-files-matrix/walks/`, macOS `~/Library/Caches`,
//! Windows `%LOCALAPPDATA%`) so that the city stands at once on the next
//! start while the new walk runs.
//!
//! A file: `AFMWALK1`, a JSON header's length and the header (what was
//! walked, when, how many nodes), the nodes (parent, kind, listing, size,
//! time, name - variable-length integers), and the SHA-256 of all that.
//! Read back, every field is checked (a parent before its child and a
//! folder, sizes in bounds, the checksum): a file that is cut, corrupt or
//! from another version is ignored - the walk simply starts from nothing.

use crate::tree::{Entry, Kind, Listing, Tree, os_from_bytes, os_to_bytes};
use ring::digest::{SHA256, digest};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"AFMWALK1";
/// No cache file is read beyond this many nodes.
pub const MAX_NODES: usize = 8_000_000;
const MAX_NAME: usize = 4096;
const MAX_HEADER: usize = 64 * 1024;

/// What a cache file says about itself.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Header {
    /// What was walked (`local:/home`, `mcp:sftp-1:/`, ...).
    pub source: String,
    /// When (seconds since 1970).
    pub saved: i64,
    pub nodes: usize,
}

/// The cache folder.
pub fn dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("axross-files-matrix")
        .join("walks")
}

/// A stable key for a source's description (its file's name).
pub fn key(source: &str) -> String {
    let d = digest(&SHA256, source.as_bytes());
    d.as_ref()[..10]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn path_for(dir: &Path, source: &str) -> PathBuf {
    dir.join(format!("{}.afmwalk", key(source)))
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Option<u8> {
        let v = *self.b.get(self.at)?;
        self.at += 1;
        Some(v)
    }
    fn varint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.byte()?;
            v |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        let end = self.at.checked_add(n)?;
        let s = self.b.get(self.at..end)?;
        self.at = end;
        Some(s)
    }
}

/// The tree as a cache file's bytes.
pub fn encode(tree: &Tree, source: &str, saved: i64) -> Vec<u8> {
    let header = serde_json::to_vec(&Header {
        source: source.to_string(),
        saved,
        nodes: tree.len(),
    })
    .unwrap_or_default();
    let mut out = Vec::with_capacity(tree.len() * 24 + header.len() + 64);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(&header);
    for n in &tree.nodes {
        put_varint(&mut out, (n.parent as u64).wrapping_add(1) & 0xffff_ffff);
        out.push(n.kind as u8);
        // (a folder being read is not read in the cache)
        let listing = if n.listing == Listing::Pending {
            Listing::Unlisted
        } else {
            n.listing
        };
        out.push(listing as u8);
        put_varint(&mut out, n.size);
        put_varint(&mut out, zigzag(n.mtime));
        let name = os_to_bytes(&n.name);
        let name = &name[..name.len().min(MAX_NAME)];
        put_varint(&mut out, name.len() as u64);
        out.extend_from_slice(name);
    }
    let sum = digest(&SHA256, &out);
    out.extend_from_slice(sum.as_ref());
    out
}

/// A tree from a cache file's bytes (None: not one, or not whole).
pub fn decode(bytes: &[u8]) -> Option<(Header, Tree)> {
    if bytes.len() < MAGIC.len() + 4 + 32 || &bytes[..8] != MAGIC {
        return None;
    }
    let (body, sum) = bytes.split_at(bytes.len() - 32);
    if digest(&SHA256, body).as_ref() != sum {
        return None;
    }
    let mut r = Reader { b: body, at: 8 };
    let hlen = u32::from_le_bytes(r.bytes(4)?.try_into().ok()?) as usize;
    if hlen > MAX_HEADER {
        return None;
    }
    let header: Header = serde_json::from_slice(r.bytes(hlen)?).ok()?;
    if header.nodes == 0 || header.nodes > MAX_NODES {
        return None;
    }
    let mut tree: Option<Tree> = None;
    let mut listings: Vec<(u32, Listing)> = Vec::new();
    for i in 0..header.nodes {
        let parent = r.varint()?;
        let kind = Kind::from_u8(r.byte()?)?;
        let listing = Listing::from_u8(r.byte()?)?;
        let size = r.varint()?;
        let mtime = unzigzag(r.varint()?);
        let len = r.varint()? as usize;
        if len > MAX_NAME {
            return None;
        }
        let name = os_from_bytes(r.bytes(len)?);
        if i == 0 {
            if parent != 0 || kind != Kind::Dir {
                return None;
            }
            tree = Some(Tree::new(&name, mtime));
        } else {
            let t = tree.as_mut()?;
            let parent = parent.checked_sub(1)? as u32;
            if parent as usize >= i {
                return None;
            }
            t.push(Entry {
                parent,
                name: name.into_boxed_os_str(),
                kind,
                size,
                mtime,
            })?;
        }
        if kind == Kind::Dir {
            listings.push((i as u32, listing));
        }
    }
    if r.at != body.len() {
        return None;
    }
    let mut tree = tree?;
    for (id, l) in listings {
        tree.nodes[id as usize].listing = l;
    }
    Some((header, tree))
}

/// Writes the tree's cache file (a file beside it first, then renamed over:
/// a crash leaves the old one or the new one, never half of one).
pub fn save(dir: &Path, source: &str, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let target = path_for(dir, source);
    let tmp = target.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &target).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Reads a source's cache file; None when there is none or it is not good
/// (or is another source's: a key collision).
pub fn load(dir: &Path, source: &str) -> Option<(Header, Tree)> {
    let path = path_for(dir, source);
    let meta = std::fs::metadata(&path).ok()?;
    // (no more than the most nodes could take)
    if meta.len() > (MAX_NODES as u64) * (MAX_NAME as u64 / 8) {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    let (h, t) = decode(&bytes)?;
    (h.source == source).then_some((h, t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::Scratch;
    use std::ffi::OsStr;

    fn sample() -> Tree {
        let mut t = Tree::new(OsStr::new("/"), 77);
        let a = t
            .push(Entry {
                parent: 0,
                name: OsStr::new("a").into(),
                kind: Kind::Dir,
                size: 4096,
                mtime: -5,
            })
            .unwrap();
        t.push(Entry {
            parent: a,
            name: os_from_bytes(b"odd\nname\xff").into_boxed_os_str(),
            kind: Kind::File,
            size: u64::MAX / 3,
            mtime: 1_700_000_000,
        });
        t.push(Entry {
            parent: 0,
            name: OsStr::new("link").into(),
            kind: Kind::Link,
            size: 12,
            mtime: 0,
        });
        t.nodes[0].listing = Listing::Listed;
        t.nodes[a as usize].listing = Listing::Denied;
        t
    }

    #[test]
    fn round_trips() {
        let t = sample();
        let bytes = encode(&t, "local:/", 123);
        let (h, back) = decode(&bytes).unwrap();
        assert_eq!(h.source, "local:/");
        assert_eq!(h.saved, 123);
        assert_eq!(back.len(), t.len());
        for (x, y) in t.nodes.iter().zip(&back.nodes) {
            assert_eq!(x.name, y.name);
            assert_eq!(
                (x.parent, x.kind, x.listing, x.size, x.mtime),
                (y.parent, y.kind, y.listing, y.size, y.mtime)
            );
            assert_eq!((x.total, x.count, x.newest), (y.total, y.count, y.newest));
        }
    }

    #[test]
    fn corrupt_files_are_refused() {
        let bytes = encode(&sample(), "s", 1);
        // every cut and every flipped byte
        for cut in 0..bytes.len() {
            assert!(decode(&bytes[..cut]).is_none(), "cut at {cut}");
        }
        for i in 0..bytes.len() {
            let mut b = bytes.clone();
            b[i] ^= 0x41;
            assert!(decode(&b).is_none(), "flip at {i}");
        }
        assert!(decode(b"").is_none());
        assert!(decode(&[0u8; 100]).is_none());
    }

    #[test]
    fn a_forged_file_with_a_good_checksum_is_still_checked() {
        // a child before its parent, with a correct checksum
        let mut body = Vec::new();
        body.extend_from_slice(MAGIC);
        let header = serde_json::to_vec(&Header {
            source: "s".into(),
            saved: 0,
            nodes: 2,
        })
        .unwrap();
        body.extend_from_slice(&(header.len() as u32).to_le_bytes());
        body.extend_from_slice(&header);
        for (parent, kind) in [(0u64, 0u8), (5, 1)] {
            put_varint(&mut body, parent);
            body.push(kind);
            body.push(3);
            put_varint(&mut body, 1);
            put_varint(&mut body, 0);
            put_varint(&mut body, 1);
            body.push(b'x');
        }
        let sum = digest(&SHA256, &body);
        body.extend_from_slice(sum.as_ref());
        assert!(decode(&body).is_none());
    }

    #[test]
    fn saves_and_loads_by_source() {
        let dir = Scratch::new("cache");
        let t = sample();
        save(dir.path(), "local:/x", &encode(&t, "local:/x", 5)).unwrap();
        assert!(load(dir.path(), "local:/x").is_some());
        assert!(load(dir.path(), "local:/y").is_none());
        // a file of another source under this key's name is not taken
        std::fs::copy(
            path_for(dir.path(), "local:/x"),
            path_for(dir.path(), "local:/z"),
        )
        .unwrap();
        assert!(load(dir.path(), "local:/z").is_none());
        assert_eq!(key("a").len(), 20);
    }
}
