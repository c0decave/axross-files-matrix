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
/// is left out).
pub fn load(found: &[Found]) -> Vec<(String, Vec<u8>)> {
    found
        .iter()
        .filter_map(|f| {
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
            Some((format!("afm-{}-{}", f.script, file_stem(&f.path)), bytes))
        })
        .collect()
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
            std::fs::write(&p, b"OTTOfont").unwrap();
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
        let defs = definitions(load(&found));
        assert!(
            defs.families[&egui::FontFamily::Proportional]
                .iter()
                .any(|n| n.starts_with("afm-cjk"))
        );
    }
}
