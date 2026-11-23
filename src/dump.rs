//! `--dump-city PATH`: the city of a folder without a window - walked,
//! laid out, and written as text (an indented tree with each district's and
//! building's ground, height and bytes) or as JSON (`--json`). For tests,
//! scripts and seeing what the layout makes of a tree.

use crate::layout::{CityLayout, Params, Placed, layout, snapshot};
use crate::tree::{Kind, NodeId, Tree, display_name};
use crate::walk::{Limits, Source, Status, walk_blocking};
use serde_json::{Value, json};
use std::path::Path;

#[derive(Clone, Copy, Debug)]
pub struct DumpOptions {
    pub json: bool,
    /// Down to this depth (0: the island only).
    pub depth: usize,
    /// At most this many nodes listed.
    pub max_nodes: usize,
}

impl Default for DumpOptions {
    fn default() -> DumpOptions {
        DumpOptions {
            json: false,
            depth: 3,
            max_nodes: 5000,
        }
    }
}

/// Walks `root` (or makes a synthetic tree of `synthetic` files) and lays
/// it out.
pub fn city_of(root: &Path, synthetic: Option<usize>) -> (Tree, CityLayout, Status, f64, f64) {
    let t0 = std::time::Instant::now();
    let source = match synthetic {
        Some(files) => Source::Synthetic { files, seed: 7 },
        None => Source::Local {
            root: root.to_path_buf(),
            skip: Vec::new(),
            auto_depth: None,
        },
    };
    let name = if synthetic.is_some() {
        std::ffi::OsString::from("synthetic")
    } else {
        root.as_os_str().to_os_string()
    };
    let (tree, status) = walk_blocking(source, Limits::default(), &name);
    let walked = t0.elapsed().as_secs_f64();
    let t1 = std::time::Instant::now();
    let l = layout(&snapshot(&tree), &Params::default());
    (tree, l, status, walked, t1.elapsed().as_secs_f64())
}

fn kind(k: Kind) -> &'static str {
    match k {
        Kind::Dir => "dir",
        Kind::File => "file",
        Kind::Link => "link",
        Kind::Other => "other",
    }
}

fn r2(v: f32) -> f64 {
    (v as f64 * 100.0).round() / 100.0
}

/// The dump of a laid-out tree.
pub fn render(
    tree: &Tree,
    l: &CityLayout,
    status: &Status,
    opts: &DumpOptions,
    walk_secs: f64,
    layout_secs: f64,
) -> String {
    let root = tree.root();
    let summary = json!({
        "root": root.name.to_string_lossy(),
        "nodes": tree.len(),
        "files": root.count,
        "bytes": root.total,
        "side": r2(l.side),
        "top": r2(l.top),
        "districts": l.districts.len(),
        "buildings": l.buildings.len(),
        "walk_seconds": (walk_secs * 1000.0).round() / 1000.0,
        "layout_seconds": (layout_secs * 1000.0).round() / 1000.0,
        "walk_error": status.error,
    });
    // depth-first, children by layout order (largest first)
    let mut order: Vec<(NodeId, usize)> = Vec::new();
    let mut stack: Vec<(NodeId, usize)> = vec![(0, 0)];
    while let Some((id, d)) = stack.pop() {
        if order.len() >= opts.max_nodes {
            break;
        }
        order.push((id, d));
        if d < opts.depth {
            let mut kids = tree.nodes[id as usize].children.clone();
            kids.sort_by(|a, b| {
                tree.nodes[*a as usize]
                    .total
                    .cmp(&tree.nodes[*b as usize].total)
                    .then(b.cmp(a))
            });
            for k in kids {
                stack.push((k, d + 1));
            }
        }
    }
    let item = |id: NodeId| -> (Option<[f64; 4]>, f64, f64) {
        match l.placed.get(id as usize) {
            Some(Placed::Building(b)) => {
                let b = &l.buildings[*b as usize];
                (
                    Some([r2(b.rect.x0), r2(b.rect.z0), r2(b.rect.x1), r2(b.rect.z1)]),
                    r2(b.base),
                    r2(b.height),
                )
            }
            Some(Placed::District(d)) => {
                let d = &l.districts[*d as usize];
                (
                    Some([r2(d.rect.x0), r2(d.rect.z0), r2(d.rect.x1), r2(d.rect.z1)]),
                    r2(d.floor),
                    r2(d.block_height),
                )
            }
            _ => (None, 0.0, 0.0),
        }
    };
    if opts.json {
        let nodes: Vec<Value> = order
            .iter()
            .map(|&(id, depth)| {
                let n = &tree.nodes[id as usize];
                let (rect, base, height) = item(id);
                json!({
                    "path": tree.chain(id).iter().skip(1).map(|s| s.to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"),
                    "name": display_name(&n.name, 200),
                    "kind": kind(n.kind),
                    "type": n.ftype.label(),
                    "depth": depth,
                    "size": n.size,
                    "total": n.total,
                    "listing": n.listing.label(),
                    "rect": rect,
                    "base": base,
                    "height": height,
                })
            })
            .collect();
        let mut out = serde_json::to_string_pretty(&json!({"city": summary, "nodes": nodes}))
            .unwrap_or_default();
        out.push('\n');
        return out;
    }
    let mut out = format!(
        "city of {}: {} nodes, {} files, {} bytes; island {} m, {} districts, {} buildings, tallest {} m; walked in {:.3} s, laid out in {:.3} s\n",
        display_name(&root.name, 200),
        tree.len(),
        root.count,
        root.total,
        r2(l.side),
        l.districts.len(),
        l.buildings.len(),
        r2(l.top),
        walk_secs,
        layout_secs
    );
    if let Some(e) = &status.error {
        out.push_str(&format!("walk: {e}\n"));
    }
    for &(id, depth) in &order {
        let n = &tree.nodes[id as usize];
        let (rect, base, height) = item(id);
        let ground = rect.map_or("-".into(), |r| {
            format!("[{} {} .. {} {}]", r[0], r[1], r[2], r[3])
        });
        out.push_str(&format!(
            "{}{} {} {} B  {} base {} h {}{}\n",
            "  ".repeat(depth),
            kind(n.kind),
            display_name(&n.name, 120),
            n.total,
            ground,
            base,
            height,
            if n.kind == Kind::Dir {
                format!("  ({})", n.listing.label())
            } else {
                String::new()
            }
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::Scratch;

    #[test]
    fn dumps_a_folder_as_text_and_json() {
        let dir = Scratch::new("dump");
        std::fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), vec![b'x'; 300]).unwrap();
        std::fs::write(dir.path().join("src/deep/a\nb.txt"), b"12").unwrap();
        std::fs::write(dir.path().join("big.iso"), vec![0u8; 10_000]).unwrap();
        let (tree, l, status, w, s) = city_of(dir.path(), None);
        let text = render(&tree, &l, &status, &DumpOptions::default(), w, s);
        assert!(text.contains("3 files, 10302 bytes"), "{text}");
        assert!(text.contains("file big.iso 10000 B"));
        assert!(text.contains("a\\nb.txt"), "the newline escaped: {text}");
        let json: Value = serde_json::from_str(&render(
            &tree,
            &l,
            &status,
            &DumpOptions {
                json: true,
                ..Default::default()
            },
            w,
            s,
        ))
        .unwrap();
        assert_eq!(json["city"]["files"], 3);
        let nodes = json["nodes"].as_array().unwrap();
        let main = nodes.iter().find(|n| n["name"] == "main.rs").unwrap();
        assert_eq!(main["path"], "src/main.rs");
        assert_eq!(main["kind"], "file");
        assert_eq!(main["type"], "source code");
        assert!(main["height"].as_f64().unwrap() > 0.0);
        // depth 0: the island alone
        let top = render(
            &tree,
            &l,
            &status,
            &DumpOptions {
                depth: 0,
                ..Default::default()
            },
            w,
            s,
        );
        assert_eq!(top.lines().count(), 2);
    }
}
