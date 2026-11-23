//! A tree laid out as a city: folders are districts (a squarified treemap
//! by bytes, streets between them), files are buildings (the footprint by
//! size, the height by size and a little variety, the colour by type, the
//! glow by how recently they changed). Runs on the layout thread from a
//! compact copy of the tree ([`LayoutNode`]), so the window never waits.
//!
//! The districts come in depth-first order: a district's whole subtree is
//! the range `i..end`, and its buildings a range too, its own files first.
//! The level of detail walks this order and skips whole subtrees - a far
//! district is drawn as one block ([`District::block_height`]).

use crate::tree::{FileType, Kind, Listing, NONE, NodeId, Tree};
use crate::treemap::{Rect, squarify};

/// What the layout needs of a node (a compact copy of [`crate::tree::Node`]).
#[derive(Clone, Copy, Debug)]
pub struct LayoutNode {
    pub parent: NodeId,
    pub kind: Kind,
    pub ftype: FileType,
    pub listing: Listing,
    pub hash: u32,
    pub total: u64,
    pub count: u32,
    pub newest: i64,
}

/// The compact copy of a tree (made on the window's thread: a copy, no
/// names - quick).
pub fn snapshot(tree: &Tree) -> Vec<LayoutNode> {
    tree.nodes
        .iter()
        .map(|n| LayoutNode {
            parent: n.parent,
            kind: n.kind,
            ftype: n.ftype,
            listing: n.listing,
            hash: n.hash,
            total: n.total,
            count: n.count,
            newest: n.newest,
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct District {
    pub node: NodeId,
    /// The district's ground (island coordinates).
    pub rect: Rect,
    /// Its plate's top (nested districts stand a little higher).
    pub floor: f32,
    pub depth: u16,
    /// The districts of its subtree: `index..end`.
    pub end: u32,
    /// Its buildings: its own `first..own_end`, its subtree's `first..sub_end`.
    pub first: u32,
    pub own_end: u32,
    pub sub_end: u32,
    /// The tallest roof in its subtree (its box for culling and picking).
    pub top: f32,
    /// Drawn as one block from afar: this high, in this type's colour.
    pub block_height: f32,
    pub ftype: FileType,
    pub glow: f32,
    pub listing: Listing,
}

#[derive(Clone, Copy, Debug)]
pub struct Building {
    pub node: NodeId,
    pub rect: Rect,
    pub base: f32,
    pub height: f32,
    pub ftype: FileType,
    pub glow: f32,
    pub hash: u32,
}

/// One island's city.
#[derive(Clone, Debug, Default)]
pub struct CityLayout {
    /// The island's side (the island is the square 0..side).
    pub side: f32,
    /// How many nodes of the tree this covers (later ones are not shown
    /// yet).
    pub nodes: usize,
    pub districts: Vec<District>,
    pub buildings: Vec<Building>,
    /// Node id -> building index or district index ([`Placed`]).
    pub placed: Vec<Placed>,
    pub top: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placed {
    Nowhere,
    District(u32),
    Building(u32),
}

/// How the city is laid out.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// The island's side for n nodes: `scale * sqrt(n + 40)`.
    pub scale: f32,
    /// A file's least weight, as a share of the island's average file.
    pub floor_share: f64,
    /// The share of a lot a building leaves free on each side.
    pub gap: f32,
    /// Seconds since 1970 now (the glow's reference).
    pub now: i64,
}

impl Default for Params {
    fn default() -> Params {
        Params {
            scale: 5.0,
            floor_share: 0.04,
            gap: 0.12,
            now: now_secs(),
        }
    }
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// How brightly a change `mtime` glows at `now`: 1 within a day, fading
/// to 0 over about two years (logarithmically); 0 when unknown.
pub fn glow(mtime: i64, now: i64) -> f32 {
    if mtime <= 0 {
        return 0.0;
    }
    let age = (now - mtime).max(0) as f32;
    let day = 86_400.0;
    if age <= day {
        return 1.0;
    }
    let years2 = 2.0 * 365.0 * day;
    (1.0 - (age / day).ln() / (years2 / day).ln()).clamp(0.0, 1.0)
}

/// A building's height on a lot of side `lot`: by size, with some variety
/// from the name's hash and the file's kind.
pub fn building_height(lot: f32, ftype: FileType, hash: u32, cap: f32) -> f32 {
    let v = (hash >> 8) as f32 / (1u32 << 24) as f32;
    let kind = match ftype {
        FileType::Video | FileType::Archive => 1.5,
        FileType::Binary => 1.3,
        FileType::Image => 1.1,
        FileType::Config => 0.7,
        _ => 1.0,
    };
    (lot * (0.7 + 2.6 * v * v) * kind).clamp(0.12, cap.max(0.12))
}

/// The street around a district of rect `r` at `depth`.
fn street(r: &Rect, depth: usize) -> f32 {
    let side = r.w().min(r.h());
    let k = if depth <= 1 { 0.035 } else { 0.025 };
    (side * k).clamp(0.04, 4.0)
}

/// The weight an unread folder gets (it shows as a placeholder block).
const UNREAD_FILES: f64 = 12.0;

/// Lays out `nodes` (a tree's compact copy).
pub fn layout(nodes: &[LayoutNode], p: &Params) -> CityLayout {
    let n = nodes.len();
    let mut out = CityLayout {
        nodes: n,
        placed: vec![Placed::Nowhere; n],
        ..Default::default()
    };
    if n == 0 {
        return out;
    }
    // children by parent (counting sort: the ids are in no tree order)
    let mut start = vec![0u32; n + 1];
    for node in &nodes[1..] {
        if (node.parent as usize) < n {
            start[node.parent as usize + 1] += 1;
        }
    }
    for i in 0..n {
        start[i + 1] += start[i];
    }
    let mut fill = start.clone();
    let mut kids = vec![0u32; n.saturating_sub(1)];
    for (id, node) in nodes.iter().enumerate().skip(1) {
        let p = node.parent as usize;
        if p < n {
            kids[fill[p] as usize] = id as u32;
            fill[p] += 1;
        }
    }
    let children = |id: usize| &kids[start[id] as usize..start[id + 1] as usize];
    // weights, bottom-up (children come after their parents)
    let root = &nodes[0];
    let avg = root.total as f64 / root.count.max(1) as f64;
    let floor = (avg * p.floor_share).max(1.0);
    let mut weight = vec![0f64; n];
    for id in (0..n).rev() {
        let node = &nodes[id];
        if node.kind == Kind::Dir {
            let sum: f64 = children(id).iter().map(|&c| weight[c as usize]).sum();
            let unread = matches!(node.listing, Listing::Unlisted | Listing::Pending);
            weight[id] = if sum > 0.0 {
                sum
            } else if unread {
                floor * UNREAD_FILES
            } else {
                floor * 0.5
            };
        } else {
            weight[id] = node.total as f64 + floor;
        }
    }
    let side = p.scale * ((root.count as f32 + n as f32 * 0.25) + 40.0).sqrt();
    out.side = side;
    let cap = side * 0.3;
    // depth-first; a stack of (node, rect, depth, parent district)
    struct Job {
        node: u32,
        rect: Rect,
        depth: u16,
        floor: f32,
    }
    let mut stack = vec![Job {
        node: 0,
        rect: Rect::new(0.0, 0.0, side, side),
        depth: 0,
        floor: 0.0,
    }];
    // a district's index, to close its ranges once its subtree is done
    let mut open: Vec<u32> = Vec::new();
    let mut order: Vec<usize> = Vec::new();
    while let Some(job) = stack.pop() {
        // close the districts whose subtree ended (not ancestors of this)
        while let Some(&d) = open.last() {
            let dn = out.districts[d as usize].node;
            if is_ancestor(nodes, dn, job.node) {
                break;
            }
            close(&mut out, d);
            open.pop();
        }
        let id = job.node as usize;
        let node = &nodes[id];
        let index = out.districts.len() as u32;
        let first = out.buildings.len() as u32;
        out.placed[id] = Placed::District(index);
        let inner = job.rect.inset(street(&job.rect, job.depth as usize));
        let plate = job.floor + if job.depth == 0 { 0.0 } else { 0.05 };
        out.districts.push(District {
            node: job.node,
            rect: job.rect,
            floor: plate,
            depth: job.depth,
            end: index + 1,
            first,
            own_end: first,
            sub_end: first,
            top: plate,
            block_height: 0.0,
            ftype: FileType::Folder,
            glow: glow(node.newest, p.now),
            listing: node.listing,
        });
        open.push(index);
        // the children, largest first
        order.clear();
        order.extend(children(id).iter().map(|&c| c as usize));
        order.sort_by(|&a, &b| weight[b].total_cmp(&weight[a]).then(a.cmp(&b)));
        let w: Vec<f64> = order.iter().map(|&c| weight[c]).collect();
        let rects = squarify(&w, inner);
        // the files here (buildings now), the folders after (pushed so
        // that the largest is laid out first)
        let mut dirs = Vec::new();
        for (&c, r) in order.iter().zip(&rects) {
            let child = &nodes[c];
            if child.kind == Kind::Dir {
                dirs.push((c, *r));
                continue;
            }
            let lot = r.area().sqrt();
            let height = building_height(lot, child.ftype, child.hash, cap);
            out.placed[c] = Placed::Building(out.buildings.len() as u32);
            out.buildings.push(Building {
                node: c as u32,
                rect: r.shrunk(p.gap),
                base: plate,
                height,
                ftype: child.ftype,
                glow: glow(child.newest, p.now),
                hash: child.hash,
            });
        }
        let own_end = out.buildings.len() as u32;
        out.districts[index as usize].own_end = own_end;
        for (c, r) in dirs.into_iter().rev() {
            stack.push(Job {
                node: c as u32,
                rect: r,
                depth: job.depth.saturating_add(1),
                floor: plate,
            });
        }
    }
    while let Some(d) = open.pop() {
        close(&mut out, d);
    }
    finish(&mut out);
    out
}

/// Whether `a` is an ancestor of (or is) `b`.
fn is_ancestor(nodes: &[LayoutNode], a: u32, b: u32) -> bool {
    let mut at = b;
    while at != NONE {
        if at == a {
            return true;
        }
        at = nodes.get(at as usize).map_or(NONE, |n| n.parent);
    }
    false
}

/// Closes district `d`'s ranges: its subtree is complete.
fn close(out: &mut CityLayout, d: u32) {
    let end = out.districts.len() as u32;
    let sub_end = out.buildings.len() as u32;
    let dist = &mut out.districts[d as usize];
    dist.end = end;
    dist.sub_end = sub_end;
}

/// The heights and colours of the blocks a far district becomes, and the
/// tops (bottom-up: the depth-first order backwards).
fn finish(out: &mut CityLayout) {
    let count = out.districts.len();
    // per district: sum of height * area, area, per type area, top
    let mut hsum = vec![0f64; count];
    let mut asum = vec![0f64; count];
    let mut types = vec![[0f32; 11]; count];
    let mut top = vec![0f32; count];
    for i in (0..count).rev() {
        let d = &out.districts[i];
        let mut t = d.floor;
        for b in &out.buildings[d.first as usize..d.own_end as usize] {
            let a = b.rect.area() as f64;
            hsum[i] += b.height as f64 * a;
            asum[i] += a;
            types[i][b.ftype as usize] += a as f32;
            t = t.max(b.base + b.height);
        }
        // the child districts (direct ones: their sums already hold theirs)
        let mut c = i + 1;
        while c < d.end as usize {
            hsum[i] += hsum[c];
            asum[i] += asum[c];
            for k in 0..11 {
                types[i][k] += types[c][k];
            }
            t = t.max(top[c]);
            c = out.districts[c].end as usize;
        }
        top[i] = t;
    }
    for (i, d) in out.districts.iter_mut().enumerate() {
        d.top = top[i];
        let area = d
            .rect
            .inset(street(&d.rect, d.depth as usize))
            .area()
            .max(1e-6) as f64;
        // the block: as high as its buildings on average over its ground
        let avg = if asum[i] > 0.0 { hsum[i] / area } else { 0.0 };
        let unread = matches!(d.listing, Listing::Unlisted | Listing::Pending);
        let lot = d.rect.area().sqrt();
        d.block_height = if asum[i] > 0.0 {
            (avg as f32).max(0.15)
        } else if unread {
            (lot * 0.25).clamp(0.2, 30.0)
        } else {
            0.1
        };
        d.top = d.top.max(d.floor + d.block_height);
        let best = types[i]
            .iter()
            .enumerate()
            .skip(1)
            .max_by(|a, b| a.1.total_cmp(b.1))
            .filter(|(_, a)| **a > 0.0)
            .map(|(k, _)| k as u8);
        d.ftype = best.map_or(FileType::Folder, FileType::from_u8);
    }
    out.top = out.districts.first().map_or(0.0, |d| d.top);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{Entry, Tree};
    use std::ffi::OsStr;

    fn tree() -> Tree {
        let mut t = Tree::new(OsStr::new("/"), 0);
        let push = |t: &mut Tree, parent, name: &str, kind, size| {
            t.push(Entry {
                parent,
                name: OsStr::new(name).into(),
                kind,
                size,
                mtime: 1_000,
            })
            .unwrap()
        };
        let a = push(&mut t, 0, "a", Kind::Dir, 0);
        let b = push(&mut t, 0, "b", Kind::Dir, 0);
        let c = push(&mut t, a, "c", Kind::Dir, 0);
        for i in 0..20 {
            push(&mut t, a, &format!("f{i}.rs"), Kind::File, 100 + i * 10);
            push(&mut t, c, &format!("g{i}.png"), Kind::File, 5000);
        }
        push(&mut t, b, "big.mkv", Kind::File, 1_000_000);
        push(&mut t, 0, "empty", Kind::File, 0);
        push(&mut t, 0, "unread", Kind::Dir, 0);
        for n in &mut t.nodes {
            if n.kind == Kind::Dir && &*n.name != OsStr::new("unread") {
                n.listing = Listing::Listed;
            }
        }
        t
    }

    #[test]
    fn every_node_is_placed_once_and_inside_its_district() {
        let t = tree();
        let l = layout(&snapshot(&t), &Params::default());
        assert_eq!(l.placed.len(), t.len());
        assert!(l.placed.iter().all(|p| *p != Placed::Nowhere));
        assert_eq!(l.buildings.len() + l.districts.len(), t.len());
        for b in &l.buildings {
            let parent = t.nodes[b.node as usize].parent;
            let Placed::District(d) = l.placed[parent as usize] else {
                panic!("parent not a district")
            };
            let r = l.districts[d as usize].rect;
            assert!(
                b.rect.x0 >= r.x0 && b.rect.x1 <= r.x1 && b.rect.z0 >= r.z0 && b.rect.z1 <= r.z1
            );
            assert!(b.height > 0.0 && b.height.is_finite());
            assert!(b.base >= l.districts[d as usize].floor - 1e-6);
        }
    }

    #[test]
    fn subtree_ranges_are_depth_first() {
        let t = tree();
        let l = layout(&snapshot(&t), &Params::default());
        let root = &l.districts[0];
        assert_eq!(root.end as usize, l.districts.len());
        assert_eq!(root.sub_end as usize, l.buildings.len());
        for (i, d) in l.districts.iter().enumerate() {
            assert!(d.end as usize > i);
            assert!(d.first <= d.own_end && d.own_end <= d.sub_end);
            // every building of the subtree range lies within the node
            for b in &l.buildings[d.first as usize..d.sub_end as usize] {
                assert!(t.is_within(b.node, d.node));
            }
            // and every district of the subtree range
            for c in &l.districts[i..d.end as usize] {
                assert!(t.is_within(c.node, d.node));
            }
            assert!(d.top >= d.floor);
        }
    }

    #[test]
    fn big_files_get_big_lots_and_unread_folders_a_block() {
        let t = tree();
        let l = layout(&snapshot(&t), &Params::default());
        let area = |name: &str| {
            let id = t
                .nodes
                .iter()
                .position(|n| &*n.name == OsStr::new(name))
                .unwrap();
            match l.placed[id] {
                Placed::Building(b) => l.buildings[b as usize].rect.area(),
                Placed::District(d) => l.districts[d as usize].rect.area(),
                Placed::Nowhere => 0.0,
            }
        };
        assert!(area("big.mkv") > area("f0.rs") * 50.0);
        assert!(area("empty") > 0.0, "an empty file still has a lot");
        let unread = t
            .nodes
            .iter()
            .position(|n| &*n.name == OsStr::new("unread"))
            .unwrap();
        let Placed::District(d) = l.placed[unread] else {
            panic!()
        };
        assert!(l.districts[d as usize].block_height >= 0.2);
        assert_eq!(l.districts[d as usize].listing, Listing::Unlisted);
    }

    #[test]
    fn glow_fades_with_age() {
        let now = 100_000_000;
        assert_eq!(glow(now - 10, now), 1.0);
        assert!(glow(now - 86_400 * 30, now) > glow(now - 86_400 * 300, now));
        assert_eq!(glow(now - 86_400 * 365 * 5, now), 0.0);
        assert_eq!(glow(0, now), 0.0);
    }

    #[test]
    fn a_huge_flat_folder_lays_out() {
        let mut t = Tree::new(OsStr::new("/"), 0);
        for i in 0..50_000u64 {
            t.push(Entry {
                parent: 0,
                name: OsStr::new(&format!("f{i}")).into(),
                kind: Kind::File,
                size: i % 1000,
                mtime: 0,
            });
        }
        let l = layout(&snapshot(&t), &Params::default());
        assert_eq!(l.buildings.len(), 50_000);
        assert!(
            l.buildings
                .iter()
                .all(|b| b.rect.x0.is_finite() && b.rect.area() >= 0.0)
        );
    }
}
