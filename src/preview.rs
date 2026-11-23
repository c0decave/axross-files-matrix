//! What the inspect panel shows of a file: its details (path, size, times,
//! owner and mode, type) and a preview - text (the first 64 KB, any bytes
//! safe: control characters escaped, bidirectional overrides shown), an
//! image (decoded on a thread of its own, its size capped) or a hex dump.
//! Remote files are read through MCP `read_file` with a byte limit. Nothing
//! here runs on the window's thread.

use crate::mcp::{Client, PathArg};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The most bytes read for a text or hex preview.
pub const TEXT_BYTES: usize = 64 * 1024;
/// The largest image file decoded (local), and read remotely.
pub const IMAGE_FILE_BYTES: u64 = 32 << 20;
pub const REMOTE_IMAGE_BYTES: u32 = 4 << 20;
/// The largest picture shown (pixels on the long side).
pub const IMAGE_SIDE: u32 = 1024;
/// The most memory a picture may take while it is decoded.
pub const IMAGE_ALLOC: u64 = 128 << 20;
/// The most lines of a text preview (each line costs every frame).
pub const TEXT_LINES: usize = 2000;

#[derive(Clone, Debug)]
pub enum Content {
    Text(String),
    Hex(String),
    Image(Arc<egui::ColorImage>, [u32; 2]),
    Empty,
    Error(String),
}

/// A file's details.
#[derive(Clone, Debug, Default)]
pub struct Details {
    pub rows: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct Preview {
    pub content: Content,
    /// Read only in part.
    pub truncated: bool,
    pub details: Details,
}

/// Where a file is.
#[derive(Clone, Debug)]
pub enum Where {
    Local(PathBuf),
    Remote {
        client: Arc<Client>,
        backend: String,
        path: Vec<u8>,
    },
}

fn is_image_name(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.ends_with(".png") || l.ends_with(".jpg") || l.ends_with(".jpeg")
}

/// Text or not: no NUL, and few control characters other than white space.
pub fn looks_like_text(b: &[u8]) -> bool {
    if b.contains(&0) {
        return false;
    }
    let text = String::from_utf8_lossy(b);
    let total = text.chars().count().max(1);
    let odd = text
        .chars()
        .filter(|c| {
            (c.is_control() && !matches!(c, '\n' | '\r' | '\t' | '\u{c}')) || *c == '\u{fffd}'
        })
        .count();
    odd * 20 <= total
}

/// Text made safe to show: control characters and bidirectional or
/// invisible formatting as visible escapes (newlines and tabs kept).
pub fn safe_text(b: &[u8]) -> String {
    safe_text_lines(b, usize::MAX).0
}

/// [`safe_text`], at most `max_lines` lines; whether it was cut.
pub fn safe_text_lines(b: &[u8], max_lines: usize) -> (String, bool) {
    let text = String::from_utf8_lossy(b);
    let mut out = String::with_capacity(text.len());
    let mut lines = 0usize;
    for c in text.chars() {
        match c {
            '\n' => {
                lines += 1;
                if lines >= max_lines {
                    return (out, true);
                }
                out.push(c);
            }
            '\t' => out.push(c),
            '\r' => {}
            c if c.is_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
            '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}' => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    (out, false)
}

/// A hex dump: offset, sixteen bytes, their printable characters.
pub fn hex_dump(b: &[u8], max: usize) -> String {
    let mut out = String::new();
    for (row, chunk) in b[..b.len().min(max)].chunks(16).enumerate() {
        out.push_str(&format!("{:08x}  ", row * 16));
        for i in 0..16 {
            match chunk.get(i) {
                Some(v) => out.push_str(&format!("{v:02x} ")),
                None => out.push_str("   "),
            }
            if i == 7 {
                out.push(' ');
            }
        }
        out.push_str(" |");
        for &v in chunk {
            out.push(if (0x20..0x7f).contains(&v) {
                v as char
            } else {
                '.'
            });
        }
        out.push_str("|\n");
    }
    out
}

/// A picture from a file's bytes, at most [`IMAGE_SIDE`] on its long side;
/// one that would take more than [`IMAGE_ALLOC`] to decode is refused
/// before anything is allocated.
pub fn decode_image(bytes: &[u8]) -> Result<(egui::ColorImage, [u32; 2]), image::ImageError> {
    let reader = || {
        image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(image::ImageError::IoError)
    };
    // (its size from the header first: the decoded picture, as RGBA, and
    // its copy must fit)
    let (w, h) = reader()?.into_dimensions()?;
    let need = u64::from(w) * u64::from(h) * 4 * 2;
    if need > IMAGE_ALLOC {
        return Err(image::ImageError::Limits(
            image::error::LimitError::from_kind(image::error::LimitErrorKind::InsufficientMemory),
        ));
    }
    let mut r = reader()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(20_000);
    limits.max_image_height = Some(20_000);
    limits.max_alloc = Some(IMAGE_ALLOC);
    r.limits(limits);
    let img = r.decode()?;
    let size = [img.width(), img.height()];
    let thumb = if img.width().max(img.height()) > IMAGE_SIDE {
        img.thumbnail(IMAGE_SIDE, IMAGE_SIDE)
    } else {
        img
    };
    let rgba = thumb.to_rgba8();
    let ci = egui::ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    );
    Ok((ci, size))
}

fn classify(bytes: &[u8], name: &str, total: u64) -> (Content, bool) {
    if bytes.is_empty() {
        return (Content::Empty, false);
    }
    if is_image_name(name) {
        match decode_image(bytes) {
            Ok((img, size)) => return (Content::Image(Arc::new(img), size), false),
            Err(image::ImageError::Limits(_)) => {
                return (
                    Content::Error("the picture is too big to preview here".into()),
                    false,
                );
            }
            // (not a picture after all: its bytes)
            Err(_) => {}
        }
    }
    let shown = &bytes[..bytes.len().min(TEXT_BYTES)];
    let truncated = total > shown.len() as u64 || bytes.len() > shown.len();
    if looks_like_text(shown) {
        let (text, cut) = safe_text_lines(shown, TEXT_LINES);
        (Content::Text(text), truncated || cut)
    } else {
        (Content::Hex(hex_dump(shown, 4096)), truncated)
    }
}

/// `rwxr-x---` and the like.
pub fn mode_string(mode: u32, dir: bool, link: bool) -> String {
    let mut s = String::with_capacity(10);
    s.push(if link {
        'l'
    } else if dir {
        'd'
    } else {
        '-'
    });
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        s.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    s
}

/// A user's name from a passwd text (`/etc/passwd`), by its id.
pub fn user_name(passwd: &str, uid: u32) -> Option<String> {
    passwd.lines().find_map(|l| {
        let mut f = l.split(':');
        let name = f.next()?;
        let _ = f.next();
        let id: u32 = f.next()?.parse().ok()?;
        (id == uid).then(|| name.to_string())
    })
}

/// A time for people (UTC; seconds since 1970).
pub fn time_string(secs: i64) -> String {
    if secs <= 0 {
        return "unknown".into();
    }
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // civil from days (H. Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        rem / 3600,
        (rem / 60) % 60
    )
}

/// The local time's offset from UTC at `secs` (seconds; None where it
/// cannot be known).
pub fn local_offset(secs: i64) -> Option<i64> {
    #[cfg(unix)]
    {
        let t = secs as libc::time_t;
        // SAFETY: localtime_r writes only into the tm given
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
        ok.then_some(tm.tm_gmtoff)
    }
    #[cfg(not(unix))]
    {
        let _ = secs;
        None
    }
}

/// A time for people: local (with its offset) where the offset is known,
/// else UTC.
pub fn time_local(secs: i64) -> String {
    match local_offset(secs) {
        Some(off) if secs > 0 && off != 0 => {
            let s = time_string(secs + off);
            let sign = if off < 0 { '-' } else { '+' };
            let a = off.abs();
            format!(
                "{} (UTC{sign}{:02}:{:02})",
                s.trim_end_matches(" UTC"),
                a / 3600,
                (a / 60) % 60
            )
        }
        _ => time_string(secs),
    }
}

fn system_time(t: std::io::Result<std::time::SystemTime>) -> i64 {
    t.ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

/// A local file's details (lstat: a link is described as the link).
pub fn local_details(path: &Path) -> Details {
    let mut rows = vec![(
        "path".to_string(),
        crate::tree::display_text(path.as_os_str()),
    )];
    match std::fs::symlink_metadata(path) {
        Err(e) => rows.push(("error".into(), e.to_string())),
        Ok(m) => {
            let ft = m.file_type();
            rows.push((
                "kind".into(),
                if ft.is_symlink() {
                    "symbolic link".into()
                } else if ft.is_dir() {
                    "folder".into()
                } else if ft.is_file() {
                    "file".into()
                } else {
                    "special file".into()
                },
            ));
            if ft.is_symlink()
                && let Ok(target) = std::fs::read_link(path)
            {
                rows.push((
                    "points to".into(),
                    crate::tree::display_text(target.as_os_str()),
                ));
            }
            if !ft.is_dir() {
                rows.push((
                    "size".into(),
                    format!("{} ({} bytes)", crate::hw::human_bytes(m.len()), m.len()),
                ));
            }
            rows.push(("changed".into(), time_local(system_time(m.modified()))));
            rows.push(("accessed".into(), time_local(system_time(m.accessed()))));
            if let Ok(t) = m.created() {
                rows.push(("created".into(), time_local(system_time(Ok(t)))));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                rows.push((
                    "mode".into(),
                    format!(
                        "{} ({:04o})",
                        mode_string(m.mode(), ft.is_dir(), ft.is_symlink()),
                        m.mode() & 0o7777
                    ),
                ));
                let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
                let owner = user_name(&passwd, m.uid()).unwrap_or_else(|| m.uid().to_string());
                rows.push((
                    "owner".into(),
                    format!("{owner} (uid {}, gid {})", m.uid(), m.gid()),
                ));
            }
            #[cfg(not(unix))]
            rows.push(("read-only".into(), m.permissions().readonly().to_string()));
        }
    }
    Details { rows }
}

/// Up to `cap` bytes of a regular file - nothing of a pipe, a device, a
/// socket or a link. Opened without following a link and without waiting
/// (a FIFO opens at once), then checked on the handle itself: what is read
/// is what was checked.
pub fn read_regular(path: &Path, cap: u64) -> std::io::Result<Vec<u8>> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_NOCTTY);
    }
    let f = match o.open(path) {
        Ok(f) => f,
        // (a link: O_NOFOLLOW refuses it - nothing to show)
        #[cfg(unix)]
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    if !f.metadata()?.is_file() {
        return Ok(Vec::new());
    }
    let mut b = Vec::new();
    f.take(cap).read_to_end(&mut b)?;
    Ok(b)
}

/// Reads a preview of `at` (named `name`, `size` bytes as the walk saw it).
pub fn load(at: &Where, name: &str, size: u64, is_dir: bool) -> Preview {
    match at {
        Where::Local(path) => {
            let details = local_details(path);
            if is_dir {
                return Preview {
                    content: Content::Empty,
                    truncated: false,
                    details,
                };
            }
            let image = is_image_name(name) && size <= IMAGE_FILE_BYTES;
            let cap = if image {
                IMAGE_FILE_BYTES
            } else {
                TEXT_BYTES as u64
            };
            let read = read_regular(path, cap);
            let (content, truncated) = match read {
                Ok(b) => classify(&b, name, size),
                Err(e) => (Content::Error(e.to_string()), false),
            };
            Preview {
                content,
                truncated,
                details,
            }
        }
        Where::Remote {
            client,
            backend,
            path,
        } => {
            let shown = crate::tree::display_text(&crate::tree::os_from_bytes(path));
            let mut rows = vec![
                ("path".to_string(), shown),
                ("backend".to_string(), backend.clone()),
            ];
            let arg = PathArg::from_bytes(path);
            if let Ok(v) = client.stat(backend, &arg, Duration::from_secs(30)) {
                if let Some(s) = v.size {
                    rows.push((
                        "size".into(),
                        format!("{} ({s} bytes)", crate::hw::human_bytes(s)),
                    ));
                }
                if let Some(m) = v.modified {
                    rows.push(("changed".into(), time_local(m)));
                }
                if let Some(m) = v.mode {
                    rows.push(("mode".into(), m));
                }
            }
            let details = Details { rows };
            if is_dir {
                return Preview {
                    content: Content::Empty,
                    truncated: false,
                    details,
                };
            }
            let image = is_image_name(name) && size <= REMOTE_IMAGE_BYTES as u64;
            let cap = if image {
                REMOTE_IMAGE_BYTES
            } else {
                TEXT_BYTES as u32
            };
            let (content, truncated) =
                match client.read(backend, &arg, cap, Duration::from_secs(60)) {
                    Ok(b) => classify(&b, name, size),
                    Err(e) => (Content::Error(e), false),
                };
            Preview {
                content,
                truncated,
                details,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::Scratch;

    #[test]
    fn text_binary_and_odd_characters() {
        assert!(looks_like_text(b"hello\nworld\t!"));
        assert!(!looks_like_text(b"hel\x00lo"));
        assert!(!looks_like_text(&[0xffu8; 100]));
        let t = safe_text("a\u{1b}[31mred\u{202e}x\r\n".as_bytes());
        assert_eq!(t, "a\\x1b[31mred\\u{202e}x\n");
        let h = hex_dump(b"ABC\x00\x01", 100);
        assert!(h.starts_with("00000000  41 42 43 00 01"));
        assert!(h.trim_end().ends_with("|ABC..|"));
    }

    #[test]
    fn previews_local_files() {
        let dir = Scratch::new("preview");
        let text = dir.path().join("a.txt");
        std::fs::write(&text, "x".repeat(100_000)).unwrap();
        let p = load(&Where::Local(text.clone()), "a.txt", 100_000, false);
        match p.content {
            Content::Text(t) => assert_eq!(t.len(), TEXT_BYTES),
            other => panic!("{other:?}"),
        }
        assert!(p.truncated);
        assert!(p.details.rows.iter().any(|(k, _)| k == "size"));
        let bin = dir.path().join("b.bin");
        std::fs::write(&bin, [0u8, 1, 2, 255]).unwrap();
        assert!(matches!(
            load(&Where::Local(bin), "b.bin", 4, false).content,
            Content::Hex(_)
        ));
        // a picture, scaled down
        let png = dir.path().join("p.png");
        image::RgbaImage::from_pixel(2000, 500, image::Rgba([0, 255, 0, 255]))
            .save(&png)
            .unwrap();
        match load(&Where::Local(png), "p.png", 1, false).content {
            Content::Image(img, size) => {
                assert_eq!(size, [2000, 500]);
                assert_eq!(img.size[0], IMAGE_SIDE as usize);
            }
            other => panic!("{other:?}"),
        }
        // a broken picture falls back to hex
        let bad = dir.path().join("bad.png");
        std::fs::write(&bad, b"\x89PNG\r\n\x1a\nnot really").unwrap();
        assert!(matches!(
            load(&Where::Local(bad), "bad.png", 10, false).content,
            Content::Hex(_)
        ));
        let gone = load(
            &Where::Local(dir.path().join("missing")),
            "missing",
            0,
            false,
        );
        assert!(gone.details.rows.iter().any(|(k, _)| k == "error"));
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_is_not_read() {
        let dir = Scratch::new("preview-fifo");
        let fifo = dir.path().join("pipe");
        let c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path
        let made = unsafe { libc::mkfifo(c.as_ptr(), 0o600) } == 0;
        if made {
            let t = std::time::Instant::now();
            let p = load(&Where::Local(fifo), "pipe", 0, false);
            assert!(matches!(p.content, Content::Empty));
            assert!(t.elapsed() < Duration::from_secs(2));
        }
    }

    #[test]
    fn previews_remote_files() {
        let client = Arc::new(crate::mcp::tests::fake_server(true));
        let p = load(
            &Where::Remote {
                client,
                backend: "sftp-1".into(),
                path: b"/z.png".to_vec(),
            },
            "z.txt",
            10,
            false,
        );
        assert!(matches!(p.content, Content::Hex(_)));
    }

    #[test]
    fn modes_owners_times() {
        assert_eq!(mode_string(0o755, true, false), "drwxr-xr-x");
        assert_eq!(mode_string(0o640, false, false), "-rw-r-----");
        assert_eq!(
            user_name(
                "daemon:x:1:1::/usr/sbin:/bin/sh\nme:x:1000:1000::/h:/bin/sh\n",
                1000
            )
            .as_deref(),
            Some("me")
        );
        assert_eq!(user_name("garbage", 0), None);
        assert_eq!(time_string(0), "unknown");
        assert_eq!(time_string(951_868_800), "2000-03-01 00:00 UTC");
        assert_eq!(time_string(1_795_430_755), "2026-11-23 10:45 UTC");
    }
}
