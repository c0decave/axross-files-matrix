//! Fonts for the scripts egui's own two do not draw - Chinese, Japanese,
//! Korean, Arabic, Hebrew, Devanagari and other Indic scripts, Thai,
//! symbols and emoji (in one colour) - found among the system's fonts on a
//! thread of its own, within a time budget, and added as fallbacks: the
//! window never waits for the scan (a slow or hung font folder only means
//! the boxes stay a while longer). No font is bundled: a CJK font alone is
//! larger than the whole program.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A script and the file names that usually cover it (lower case, parts).
const WANTED: &[(&str, &[&str])] = &[
    (
        "cjk",
        &[
            "notosanscjk",
            "notosansjp",
            "notosanssc",
            "notosanstc",
            "notosanskr",
            "notoserifcjk",
            "sourcehansans",
            "wqy-microhei",
            "wqy-zenhei",
            "droidsansfallback",
            "msyh",
            "msgothic",
            "meiryo",
            "yugoth",
            "malgun",
            "simsun",
            "pingfang",
            "hiragino",
            "applesdgothicneo",
        ],
    ),
    (
        "korean",
        &[
            "notosanskr",
            "notosanscjk",
            "notoserifkr",
            "nanumgothic",
            "malgun",
            "applesdgothicneo",
            "unbatang",
        ],
    ),
    (
        "arabic",
        &[
            "notosansarabic",
            "notonaskharabic",
            "notokufiarabic",
            "segoeui.ttf",
            "geezapro",
            "dejavusans.ttf",
        ],
    ),
    (
        "hebrew",
        &["notosanshebrew", "segoeui.ttf", "arialhb", "dejavusans.ttf"],
    ),
    (
        "indic",
        &[
            "notosansdevanagari",
            "lohit-devanagari",
            "mangal",
            "nirmala",
            "kohinoordevanagari",
            "devanagari",
        ],
    ),
    (
        "thai",
        &["notosansthai", "leelawad", "thonburi", "tlwgtypo", "garuda"],
    ),
    (
        "symbols",
        &[
            "notoemoji-regular",
            "notoemoji",
            "notocoloremoji",
            "twemoji",
            "emojione",
            "joypixels",
            "openmoji",
            "applecoloremoji",
            "notosanssymbols2",
            "symbola",
            "seguisym",
            "seguiemj",
            "applesymbols",
            "dejavusans.ttf",
        ],
    ),
];

/// The folders fonts are looked for in (`AFM_FONT_DIRS`, `:`-separated,
/// first, then the system's).
pub fn font_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("AFM_FONT_DIRS")
        .map(|v| std::env::split_paths(&v).collect())
        .unwrap_or_default();
    #[cfg(target_os = "linux")]
    {
        dirs.push("/usr/share/fonts".into());
        dirs.push("/usr/local/share/fonts".into());
        if let Some(h) = dirs::home_dir() {
            dirs.push(h.join(".local/share/fonts"));
            dirs.push(h.join(".fonts"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push("/System/Library/Fonts".into());
        dirs.push("/Library/Fonts".into());
        if let Some(h) = dirs::home_dir() {
            dirs.push(h.join("Library/Fonts"));
        }
    }
    #[cfg(windows)]
    {
        let windir = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| "C:\\Windows".into());
        dirs.push(windir.join("Fonts"));
    }
    dirs
}

/// A font found: its file and what it is for.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub script: &'static str,
    pub path: PathBuf,
}

/// The font files in `dirs` (at most four levels down, `budget` of time,
/// 50 000 entries): the best one per script, by the order of [`WANTED`].
pub fn find(dirs: &[PathBuf], budget: Duration) -> Vec<Found> {
    let start = Instant::now();
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let mut seen = 0usize;
    let mut stack: Vec<(PathBuf, u32)> = dirs.iter().map(|d| (d.clone(), 0)).collect();
    while let Some((dir, depth)) = stack.pop() {
        if start.elapsed() > budget || seen > 50_000 {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            seen += 1;
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() && depth < 4 {
                stack.push((p, depth + 1));
            } else if ft.is_file() {
                let name = e.file_name().to_string_lossy().to_lowercase();
                if name.ends_with(".ttf") || name.ends_with(".otf") || name.ends_with(".ttc") {
                    files.push((name, p));
                }
            }
        }
    }
    files.sort();
    let mut out: Vec<Found> = Vec::new();
    for (script, wanted) in WANTED {
        let hit = wanted.iter().find_map(|w| {
            files
                .iter()
                // (regular weights first: no bold, no italic)
                .filter(|(n, _)| n.contains(w))
                .min_by_key(|(n, _)| {
                    (n.contains("bold") || n.contains("italic") || n.contains("light")) as u8
                })
        });
        if let Some((_, p)) = hit
            && !out.iter().any(|f| &f.path == p)
        {
            out.push(Found {
                script,
                path: p.clone(),
            });
        }
    }
    out
}

/// The found fonts' bytes (each at most 48 MB; a file that cannot be read
/// is left out), read and checked - every glyph drawn once - within
/// thirty seconds (on the fonts' own thread: the window never waits).
pub fn load(found: &[Found]) -> Vec<(String, Vec<u8>)> {
    load_within(found, Duration::from_secs(30))
}

/// [`load`] within `budget`: past it, the rest is left out (and said so) -
/// a slow disk or a font folder on a dead mount ends the checks, never the
/// fonts' thread.
pub fn load_within(found: &[Found], budget: Duration) -> Vec<(String, Vec<u8>)> {
    let start = Instant::now();
    found
        .iter()
        .filter_map(|f| {
            if start.elapsed() >= budget {
                eprintln!(
                    "axross-files-matrix: no time left to check the font {}; left out",
                    f.path.display()
                );
                return None;
            }
            let m = std::fs::metadata(&f.path).ok()?;
            if m.len() > 48 << 20 {
                return None;
            }
            let bytes = std::fs::read(&f.path).ok()?;
            // (a font file by its first bytes: TrueType, OpenType, a collection)
            let magic = bytes.get(..4)?;
            if !matches!(magic, [0, 1, 0, 0] | b"OTTO" | b"ttcf" | b"true") {
                return None;
            }
            // read whole by egui's own parser here, on the fonts' thread: a
            // file it cannot take is left out, never handed to the window
            // (where it would stop the program at every start)
            if !parses(&bytes) || !glyphs_draw(&bytes, budget.saturating_sub(start.elapsed())) {
                eprintln!(
                    "axross-files-matrix: the font {} cannot be read; left out",
                    f.path.display()
                );
                return None;
            }
            Some((format!("afm-{}-{}", f.script, file_stem(&f.path)), bytes))
        })
        .collect()
}

/// A pen that keeps nothing: the outlines are only drawn to see that they
/// can be.
struct NoPen;

impl skrifa::outline::OutlinePen for NoPen {
    fn move_to(&mut self, _: f32, _: f32) {}
    fn line_to(&mut self, _: f32, _: f32) {}
    fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {}
    fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {}
    fn close(&mut self) {}
}

/// Whether every glyph of the font's first face (the one egui takes) can
/// be drawn as egui draws them - unhinted and hinted, its metrics too -
/// within `budget` (a panic inside the parser is caught: false; past the
/// budget: false, the font left out unchecked).
pub fn glyphs_draw(bytes: &[u8], budget: Duration) -> bool {
    use skrifa::MetadataProvider as _;
    use skrifa::raw::TableProvider as _;
    let start = Instant::now();
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Ok(font) = skrifa::FontRef::from_index(bytes, 0) else {
            return false;
        };
        let loc = skrifa::instance::LocationRef::default();
        let outlines = font.outline_glyphs();
        let charmap = font.charmap();
        let m = font.metrics(skrifa::instance::Size::unscaled(), loc);
        // (a scale of nothing: egui would divide by it)
        if m.units_per_em == 0 || !(m.ascent.is_finite() && m.descent.is_finite()) {
            return false;
        }
        let gm = font.glyph_metrics(skrifa::instance::Size::new(14.0), loc);
        let hinting = skrifa::outline::HintingInstance::new(
            &outlines,
            skrifa::instance::Size::new(14.0),
            loc,
            skrifa::outline::Target::default(),
        )
        .ok();
        let count = font.maxp().map(|m| m.num_glyphs()).unwrap_or(0) as u32;
        // (the characters' glyphs as the charmap names them, and every
        // glyph by its number: a hole anywhere is found)
        for (_, gid) in charmap.mappings() {
            let _ = gm.advance_width(gid);
            if start.elapsed() > budget {
                return false;
            }
        }
        for g in 0..count {
            let gid = skrifa::GlyphId::new(g);
            let _ = gm.advance_width(gid);
            if let Some(o) = outlines.get(gid) {
                let _ = o.draw(
                    skrifa::outline::DrawSettings::unhinted(skrifa::instance::Size::new(14.0), loc),
                    &mut NoPen,
                );
                if let Some(h) = &hinting {
                    let _ = o.draw(skrifa::outline::DrawSettings::hinted(h, false), &mut NoPen);
                }
            }
            if g % 256 == 0 && start.elapsed() > budget {
                return false;
            }
        }
        true
    }))
    .unwrap_or(false)
}

/// Whether egui's font system takes `bytes` as a font and can look glyphs
/// up in it (a panic inside it is caught: false).
pub fn parses(bytes: &[u8]) -> bool {
    let bytes = bytes.to_vec();
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let mut defs = egui::FontDefinitions::empty();
        defs.font_data.insert(
            "probe".into(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        defs.families
            .insert(egui::FontFamily::Proportional, vec!["probe".into()]);
        defs.families
            .insert(egui::FontFamily::Monospace, vec!["probe".into()]);
        let mut fonts =
            egui::epaint::text::Fonts::new(egui::epaint::text::TextOptions::default(), defs);
        let id = egui::FontId::proportional(14.0);
        let _ = fonts.has_glyph(&id, 'a');
        let _ = fonts.has_glyph(&id, '\u{65e5}');
        // (drawn as the window draws: its scale and metrics used too)
        let mut view = fonts.with_pixels_per_point(1.0);
        let _ = view.layout_no_wrap("aA0 日本".into(), id, egui::Color32::WHITE);
    }))
    .is_ok()
}

fn file_stem(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// egui's fonts with `extra` added as fallbacks of every family (after its
/// own, in the order given).
pub fn definitions(extra: Vec<(String, Vec<u8>)>) -> egui::FontDefinitions {
    let mut defs = egui::FontDefinitions::default();
    for (name, bytes) in extra {
        defs.font_data.insert(
            name.clone(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            defs.families.entry(family).or_default().push(name.clone());
        }
    }
    defs
}

/// Scans and loads on a thread of its own; `done` gets the fonts (never
/// called when none is found).
pub fn find_in_background(done: impl FnOnce(Vec<(String, Vec<u8>)>) + Send + 'static) {
    let _ = std::thread::Builder::new()
        .name("afm-fonts".into())
        .spawn(move || {
            let found = find(&font_dirs(), Duration::from_secs(3));
            let fonts = load(&found);
            if !fonts.is_empty() {
                done(fonts);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_best_font_per_script_is_found_in_time() {
        let dir = crate::testdir::Scratch::new("fonts");
        let d = dir.path();
        for f in [
            "noto/NotoSansCJK-Bold.ttc",
            "noto/NotoSansCJK-Regular.ttc",
            "arabic/NotoSansArabic-Regular.ttf",
            "x/Readme.txt",
            "dejavu/DejaVuSans.ttf",
        ] {
            let p = d.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            // (a real font's bytes where the system has one: load reads it)
            let body = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf")
                .unwrap_or_else(|_| b"OTTOfont".to_vec());
            std::fs::write(&p, body).unwrap();
        }
        let found = find(&[d.to_path_buf()], Duration::from_secs(2));
        let by: Vec<(&str, String)> = found
            .iter()
            .map(|f| {
                (
                    f.script,
                    f.path.file_name().unwrap().to_string_lossy().into_owned(),
                )
            })
            .collect();
        assert!(
            by.contains(&("cjk", "NotoSansCJK-Regular.ttc".into())),
            "{by:?}"
        );
        assert!(
            by.contains(&("arabic", "NotoSansArabic-Regular.ttf".into())),
            "{by:?}"
        );
        assert!(by.contains(&("hebrew", "DejaVuSans.ttf".into())), "{by:?}");
        // (no time at all: nothing, and no wait)
        assert!(find(&[d.to_path_buf()], Duration::ZERO).len() <= found.len());
        if std::path::Path::new("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf").exists() {
            let defs = definitions(load(&found));
            assert!(
                defs.families[&egui::FontFamily::Proportional]
                    .iter()
                    .any(|n| n.starts_with("afm-cjk"))
            );
        }
    }

    /// Draws every character the font maps with egui itself (a panic is
    /// caught: false) - what the window would do with it.
    fn egui_draws_all(bytes: &[u8]) -> bool {
        use skrifa::MetadataProvider as _;
        let chars: String = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            skrifa::FontRef::from_index(bytes, 0)
                .map(|f| {
                    f.charmap()
                        .mappings()
                        .filter_map(|(c, _)| char::from_u32(c))
                        .filter(|c| !c.is_control())
                        .collect()
                })
                .unwrap_or_default()
        }))
        .unwrap_or_default();
        let bytes = bytes.to_vec();
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let mut defs = egui::FontDefinitions::empty();
            defs.font_data.insert(
                "probe".into(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            defs.families
                .insert(egui::FontFamily::Proportional, vec!["probe".into()]);
            defs.families
                .insert(egui::FontFamily::Monospace, vec!["probe".into()]);
            let mut fonts =
                egui::epaint::text::Fonts::new(egui::epaint::text::TextOptions::default(), defs);
            let mut view = fonts.with_pixels_per_point(1.0);
            for chunk in chars.chars().collect::<Vec<_>>().chunks(64) {
                let text: String = chunk.iter().collect();
                let _ = view.layout_no_wrap(
                    text,
                    egui::FontId::proportional(14.0),
                    egui::Color32::WHITE,
                );
            }
        }))
        .is_ok()
    }

    /// A real font of this machine (DejaVu Sans, Liberation Sans), if any.
    fn a_real_font() -> Option<Vec<u8>> {
        [
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
        ]
        .iter()
        .find_map(|p| std::fs::read(p).ok())
    }

    /// A table's place in a font file (offset, length).
    fn table(bytes: &[u8], tag: &[u8; 4]) -> Option<(usize, usize)> {
        let n = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
        (0..n).find_map(|i| {
            let r = &bytes[12 + i * 16..12 + i * 16 + 16];
            (&r[..4] == tag).then(|| {
                (
                    u32::from_be_bytes([r[8], r[9], r[10], r[11]]) as usize,
                    u32::from_be_bytes([r[12], r[13], r[14], r[15]]) as usize,
                )
            })
        })
    }

    #[test]
    fn a_font_with_a_hole_is_left_out_or_draws_everything() {
        let Some(good) = a_real_font() else {
            eprintln!("SKIPPED: no DejaVu or Liberation font here");
            return;
        };
        assert!(parses(&good) && glyphs_draw(&good, Duration::from_secs(30)));
        assert!(egui_draws_all(&good));
        // a zeroed 4 KiB block (a bad block, a hole) at many places: what
        // the checks take, egui draws whole - never a panic in the window
        let mut taken = 0;
        let step = (good.len() / 120).max(4096);
        for at in (0..good.len()).step_by(step) {
            let mut bad = good.clone();
            let end = (at + 4096).min(bad.len());
            bad[at..end].iter_mut().for_each(|b| *b = 0);
            if parses(&bad) && glyphs_draw(&bad, Duration::from_secs(30)) {
                taken += 1;
                assert!(
                    egui_draws_all(&bad),
                    "a hole at {at} passed the checks and panics egui"
                );
            }
        }
        eprintln!("holes taken harmlessly: {taken}");
        // the glyph table zeroed: left out (or harmless)
        let (off, len) = table(&good, b"glyf").expect("a glyf table");
        let mut zeroed = good.clone();
        zeroed[off..off + len].iter_mut().for_each(|b| *b = 0);
        if parses(&zeroed) && glyphs_draw(&zeroed, Duration::from_secs(30)) {
            assert!(egui_draws_all(&zeroed));
        }
    }

    #[test]
    fn checking_fonts_keeps_to_its_time() {
        let dir = crate::testdir::Scratch::new("fonts-time");
        let p = dir.path().join("NotoSansCJK-Regular.ttc");
        std::fs::write(&p, b"ttcf\0\x01\0\0").unwrap();
        let found = vec![Found {
            script: "cjk",
            path: p,
        }];
        // no time at all: nothing is read, nothing handed on
        assert!(load_within(&found, Duration::ZERO).is_empty());
    }

    #[test]
    fn damaged_fonts_are_left_out_never_handed_on() {
        let dir = crate::testdir::Scratch::new("bad-fonts");
        let d = dir.path();
        let cases: Vec<(&str, Vec<u8>)> = vec![
            // a collection whose offsets point nowhere
            ("NotoSansCJK-Regular.ttc", {
                let mut b = b"ttcf\x00\x01\x00\x00\x00\x00\x00\x01\xff\xff\xff\xff".to_vec();
                b.extend_from_slice(&[0; 4]);
                b
            }),
            // cut off in its header
            (
                "NotoSansArabic-Regular.ttf",
                vec![0, 1, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            ),
            // zero-filled
            ("NotoSansThai-Regular.ttf", vec![0; 4096]),
            (
                "NotoSansHebrew-Regular.ttf",
                b"\x00\x01\x00\x00garbage after the magic".to_vec(),
            ),
            ("NotoSansDevanagari-Regular.otf", b"OTTO".to_vec()),
            // the wrong magic
            ("NotoColorEmoji.ttf", b"wrong magic entirely".to_vec()),
        ];
        for (n, b) in cases {
            std::fs::write(d.join(n), b).unwrap();
        }
        // a huge one: past the limit, not even read
        let huge = d.join("NotoSansKR-Regular.ttf");
        std::fs::write(&huge, b"\x00\x01\x00\x00").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&huge)
            .unwrap()
            .set_len(60 << 20)
            .unwrap();
        let found = find(&[d.to_path_buf()], Duration::from_secs(2));
        assert!(found.len() >= 6, "{found:?}");
        let loaded = load(&found);
        let names: Vec<&str> = loaded.iter().map(|l| l.0.as_str()).collect();
        // what egui cannot read is left out (the collection with bad
        // offsets: the window stopped at every start), and so is what is
        // no font at all ...
        for bad in ["cjk", "thai", "indic", "symbols", "korean"] {
            assert!(
                !names.iter().any(|n| n.contains(bad)),
                "{bad} left out: {names:?}"
            );
        }
        // ... and what egui does take is safe to use in the window
        let defs = definitions(loaded);
        let used = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut fonts =
                egui::epaint::text::Fonts::new(egui::epaint::text::TextOptions::default(), defs);
            let id = egui::FontId::proportional(14.0);
            fonts.has_glyphs(&id, "abc \u{0645}\u{05d0}")
        }));
        assert!(used.is_ok(), "the fonts handed on never stop the window");
        if let Ok(good) = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf") {
            assert!(parses(&good), "a good font passes");
        }
    }

    #[test]
    fn colour_emoji_fonts_are_found() {
        let dir = crate::testdir::Scratch::new("emoji-fonts");
        std::fs::write(dir.path().join("NotoColorEmoji.ttf"), b"OTTO").unwrap();
        let found = find(&[dir.path().to_path_buf()], Duration::from_secs(2));
        assert!(found.iter().any(|f| f.script == "symbols"), "{found:?}");
    }
}
