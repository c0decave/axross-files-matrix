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
    /// The bytes on the disk (files).
    pub alloc: u64,
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
            alloc: n.alloc,
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
    /// Its own buildings' tiles (`tiles[a..b]`; empty: drawn one by one).
    pub tiles: (u32, u32),
    /// Its subtree's tallest buildings (`tall[a..b]`), still drawn as
    /// towers when the district is one block: a skyline survives.
    pub tall: (u32, u32),
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
    /// A big district's own buildings in tiles (each drawn as one block
    /// from afar): the tiles, and their buildings' indices.
    pub tiles: Vec<Tile>,
    pub tile_items: Vec<u32>,
    /// Per building: its tile (u32::MAX: none).
    pub tile_of: Vec<u32>,
    /// The towers of the districts and tiles ([`District::tall`]).
    pub tall: Vec<u32>,
}

impl CityLayout {
    /// The same city `k` times as big (every length, the heights too).
    pub fn scaled(&self, k: f32) -> CityLayout {
        let r = |r: Rect| Rect::new(r.x0 * k, r.z0 * k, r.x1 * k, r.z1 * k);
        let mut l = self.clone();
        l.side *= k;
        l.top *= k;
        for d in &mut l.districts {
            d.rect = r(d.rect);
            d.floor *= k;
            d.top *= k;
            d.block_height *= k;
        }
        for b in &mut l.buildings {
            b.rect = r(b.rect);
            b.base *= k;
            b.height *= k;
        }
        for t in &mut l.tiles {
            t.rect = r(t.rect);
            t.base *= k;
            t.height *= k;
            t.top *= k;
        }
        l
    }
}

/// A piece of a big district's own buildings: one block from afar.
#[derive(Clone, Debug)]
pub struct Tile {
    pub district: u32,
    pub rect: Rect,
    /// Its buildings: `tile_items[first..end]`.
    pub first: u32,
    pub end: u32,
    pub base: f32,
    pub height: f32,
    pub top: f32,
    pub ftype: FileType,
    pub glow: f32,
    /// Its tallest buildings (`CityLayout::tall[a..b]`).
    pub tall: (u32, u32),
}

/// A district with more own buildings than this is cut into tiles.
pub const TILE_MIN: usize = 192;
/// About this many buildings a tile.
const TILE_SIZE: usize = 96;

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
    /// Weigh files by the bytes they take on the disk (a sparse file by
    /// what it holds), not by their apparent size.
    pub by_alloc: bool,
    /// Seconds since 1970 now (the glow's reference).
    pub now: i64,
}

impl Default for Params {
    fn default() -> Params {
        Params {
            scale: 5.0,
            // (every file at least a sixth of the average's footprint: one
            // huge file leaves the others room - no strips 0.4 m deep)
            floor_share: 0.16,
            gap: 0.12,
            by_alloc: true,
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
    let bytes = |node: &LayoutNode| if p.by_alloc { node.alloc } else { node.total };
    let file_bytes: f64 = nodes
        .iter()
        .filter(|x| x.kind != Kind::Dir)
        .map(|x| bytes(x) as f64)
        .sum();
    let avg = file_bytes / root.count.max(1) as f64;
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
            weight[id] = bytes(node) as f64 + floor;
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
            tiles: (0, 0),
            tall: (0, 0),
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
    make_tiles(&mut out);
    out
}

/// Cuts the own buildings of each big district into tiles of about
/// [`TILE_SIZE`], by where they stand (a grid over the district): from afar
/// a tile is one block, so a flat folder of 200 000 files is a few
/// thousand boxes, not 200 000.
fn make_tiles(out: &mut CityLayout) {
    out.tile_of = vec![u32::MAX; out.buildings.len()];
    for d in 0..out.districts.len() {
        let (first, own_end) = (
            out.districts[d].first as usize,
            out.districts[d].own_end as usize,
        );
        let own = own_end - first;
        if own < TILE_MIN {
            continue;
        }
        let r = out.districts[d].rect;
        let g = ((own as f32 / TILE_SIZE as f32).sqrt().ceil() as usize).max(2);
        let mut cells: Vec<Vec<u32>> = vec![Vec::new(); g * g];
        for (k, bld) in out.buildings[first..own_end].iter().enumerate() {
            let b = first + k;
            let (cx, cz) = bld.rect.center();
            let gx =
                (((cx - r.x0) / r.w().max(1e-6)) * g as f32).clamp(0.0, g as f32 - 1.0) as usize;
            let gz =
                (((cz - r.z0) / r.h().max(1e-6)) * g as f32).clamp(0.0, g as f32 - 1.0) as usize;
            cells[gz * g + gx].push(b as u32);
        }
        let a = out.tiles.len() as u32;
        for items in cells.into_iter().filter(|c| !c.is_empty()) {
            let mut rect = Rect::new(f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            let (mut top, mut glow) = (0f32, 0f32);
            let mut types = [0f32; 11];
            let base = out.buildings[items[0] as usize].base;
            for &b in &items {
                let bl = &out.buildings[b as usize];
                rect = Rect::new(
                    rect.x0.min(bl.rect.x0),
                    rect.z0.min(bl.rect.z0),
                    rect.x1.max(bl.rect.x1),
                    rect.z1.max(bl.rect.z1),
                );
                let area = bl.rect.area();
                top = top.max(bl.base + bl.height);
                glow = glow.max(bl.glow);
                types[bl.ftype as usize] += area;
            }
            let ftype = types
                .iter()
                .enumerate()
                .max_by(|x, y| x.1.total_cmp(y.1))
                .map_or(FileType::Other, |(k, _)| FileType::from_u8(k as u8));
            let height = p90(out, items.iter().copied()).max(0.15);
            let mut cand = items.clone();
            let towers = tallest(out, &mut cand, TILE_TOWERS, height);
            let tall = (
                out.tall.len() as u32,
                (out.tall.len() + towers.len()) as u32,
            );
            out.tall.extend_from_slice(&towers);
            let first_item = out.tile_items.len() as u32;
            let index = out.tiles.len() as u32;
            for &b in &items {
                out.tile_of[b as usize] = index;
            }
            out.tile_items.extend_from_slice(&items);
            out.tiles.push(Tile {
                district: d as u32,
                rect,
                first: first_item,
                end: out.tile_items.len() as u32,
                base,
                height,
                top,
                ftype,
                glow,
                tall,
            });
        }
        out.districts[d].tiles = (a, out.tiles.len() as u32);
    }
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

/// How many towers a far district keeps.
pub const TOWERS: usize = 4;
/// How many a far tile keeps.
pub const TILE_TOWERS: usize = 3;

/// The 90th percentile of the heights of `items` (at most 512 of them
/// looked at, evenly spread: a big district costs no more than a small
/// one).
fn p90(out: &CityLayout, items: impl ExactSizeIterator<Item = u32> + Clone) -> f32 {
    let n = items.len();
    if n == 0 {
        return 0.0;
    }
    let step = n.div_ceil(512).max(1);
    let mut h: Vec<f32> = items
        .step_by(step)
        .map(|b| out.buildings[b as usize].height)
        .collect();
    let k = ((h.len() - 1) as f32 * 0.9).round() as usize;
    let (_, v, _) = h.select_nth_unstable_by(k, |a, b| a.total_cmp(b));
    *v
}

/// The `k` tallest of `items` (tallest first), only those standing out
/// over `block` by a third (a tower barely over its block is not seen).
fn tallest(out: &CityLayout, items: &mut Vec<u32>, k: usize, block: f32) -> Vec<u32> {
    let height = |b: &u32| out.buildings[*b as usize].height;
    if items.len() > k {
        items.select_nth_unstable_by(k - 1, |a, b| height(b).total_cmp(&height(a)));
        items.truncate(k);
    }
    items.sort_by(|a, b| height(b).total_cmp(&height(a)).then(a.cmp(b)));
    items.retain(|b| height(b) > block * 1.3);
    std::mem::take(items)
}

/// The heights and colours of the blocks a far district becomes, its
/// towers, and the tops (bottom-up: the depth-first order backwards).
fn finish(out: &mut CityLayout) {
    let count = out.districts.len();
    // per district: area, per type area, top, its subtree's tallest
    let mut asum = vec![0f64; count];
    let mut types = vec![[0f32; 11]; count];
    let mut top = vec![0f32; count];
    let mut towers: Vec<Vec<u32>> = vec![Vec::new(); count];
    let mut heights = vec![0f32; count];
    for i in (0..count).rev() {
        let d = &out.districts[i];
        let mut t = d.floor;
        let mut cand: Vec<u32> = Vec::new();
        for (k, b) in out.buildings[d.first as usize..d.own_end as usize]
            .iter()
            .enumerate()
        {
            let a = b.rect.area() as f64;
            asum[i] += a;
            types[i][b.ftype as usize] += a as f32;
            t = t.max(b.base + b.height);
            cand.push(d.first + k as u32);
        }
        if cand.len() > TOWERS {
            let height = |b: &u32| out.buildings[*b as usize].height;
            cand.select_nth_unstable_by(TOWERS - 1, |a, b| height(b).total_cmp(&height(a)));
            cand.truncate(TOWERS);
        }
        // the child districts (direct ones: their sums already hold theirs)
        let mut c = i + 1;
        while c < d.end as usize {
            asum[i] += asum[c];
            for k in 0..11 {
                types[i][k] += types[c][k];
            }
            t = t.max(top[c]);
            cand.extend_from_slice(&towers[c]);
            c = out.districts[c].end as usize;
        }
        top[i] = t;
        // (the block: its buildings' 90th percentile, never their mean -
        // a mean flattens a skyline into a slab)
        heights[i] = p90(out, d.first..d.sub_end);
        towers[i] = tallest(out, &mut cand, TOWERS, 0.0);
    }
    for i in 0..count {
        let d = &out.districts[i];
        let unread = matches!(d.listing, Listing::Unlisted | Listing::Pending);
        let lot = d.rect.area().sqrt();
        let block = if asum[i] > 0.0 {
            heights[i].max(0.15)
        } else if unread {
            (lot * 0.25).clamp(0.2, 30.0)
        } else {
            0.1
        };
        towers[i].retain(|b| out.buildings[*b as usize].height > block * 1.3);
        let a = out.tall.len() as u32;
        out.tall.extend_from_slice(&towers[i]);
        let b = out.tall.len() as u32;
        let best = types[i]
            .iter()
            .enumerate()
            .skip(1)
            .max_by(|a, b| a.1.total_cmp(b.1))
            .filter(|(_, a)| **a > 0.0)
            .map(|(k, _)| k as u8);
        let d = &mut out.districts[i];
        d.top = top[i].max(d.floor + block);
        d.block_height = block;
        d.tall = (a, b);
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
                alloc: size,
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
    fn a_far_district_keeps_its_skyline_never_a_mean_slab() {
        // one folder: many small files and a few big ones
        let mut t = Tree::new(OsStr::new("/"), 0);
        let push = |t: &mut Tree, parent, name: &str, kind, size| {
            t.push(Entry {
                parent,
                name: OsStr::new(name).into(),
                kind,
                size,
                alloc: size,
                mtime: 1_000,
            })
            .unwrap()
        };
        let a = push(&mut t, 0, "a", Kind::Dir, 0);
        let c = push(&mut t, a, "c", Kind::Dir, 0);
        for i in 0..300 {
            push(&mut t, c, &format!("s{i}.txt"), Kind::File, 2_000 + i);
        }
        for i in 0..4 {
            push(&mut t, a, &format!("big{i}.mkv"), Kind::File, 3_000_000);
        }
        for n in &mut t.nodes {
            if n.kind == Kind::Dir {
                n.listing = Listing::Listed;
            }
        }
        let l = layout(&snapshot(&t), &Params::default());
        for (i, d) in l.districts.iter().enumerate() {
            let hs: Vec<f32> = l.buildings[d.first as usize..d.sub_end as usize]
                .iter()
                .map(|b| b.height)
                .collect();
            if hs.is_empty() {
                continue;
            }
            let mut sorted = hs.clone();
            sorted.sort_by(f32::total_cmp);
            let p90 = sorted[((sorted.len() - 1) as f32 * 0.9).round() as usize];
            let inner = d.rect.inset(street(&d.rect, d.depth as usize)).area();
            let volume: f32 = l.buildings[d.first as usize..d.sub_end as usize]
                .iter()
                .map(|b| b.height * b.rect.area())
                .sum();
            let mean = volume / inner.max(1e-6);
            assert!(
                (d.block_height - p90.max(0.15)).abs() < 1e-4,
                "district {i}: block {} is not the 90th percentile {p90} (mean {mean})",
                d.block_height
            );
            // its towers: the tallest, tallest first, all over the block
            let tall = &l.tall[d.tall.0 as usize..d.tall.1 as usize];
            assert!(tall.len() <= TOWERS);
            let max = *sorted.last().unwrap();
            if max > d.block_height * 1.3 {
                assert!(!tall.is_empty(), "district {i} lost its tallest tower");
                assert_eq!(l.buildings[tall[0] as usize].height, max);
            }
            for w in tall.windows(2) {
                assert!(l.buildings[w[0] as usize].height >= l.buildings[w[1] as usize].height);
            }
            for &b in tall {
                assert!(b >= d.first && b < d.sub_end, "a tower of another district");
                assert!(l.buildings[b as usize].height > d.block_height);
            }
        }
        // the root: the four big files stand as towers over the small ones
        let root = &l.districts[0];
        let names: Vec<String> = l.tall[root.tall.0 as usize..root.tall.1 as usize]
            .iter()
            .map(|&b| {
                t.nodes[l.buildings[b as usize].node as usize]
                    .name
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        for i in 0..4 {
            assert!(names.contains(&format!("big{i}.mkv")), "{names:?}");
        }
        // and the tiles of the folder of 300 files keep theirs too
        for tile in &l.tiles {
            let items = &l.tile_items[tile.first as usize..tile.end as usize];
            let mut hs: Vec<f32> = items
                .iter()
                .map(|&b| l.buildings[b as usize].height)
                .collect();
            hs.sort_by(f32::total_cmp);
            let p90 = hs[((hs.len() - 1) as f32 * 0.9).round() as usize].max(0.15);
            assert!((tile.height - p90).abs() < 1e-4);
            assert!(tile.tall.1 - tile.tall.0 <= TILE_TOWERS as u32);
        }
        assert!(!l.tiles.is_empty());
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
    fn a_big_flat_folder_is_cut_into_tiles_that_cover_every_building_once() {
        let mut t = Tree::new(OsStr::new("/"), 0);
        for i in 0..20_000u64 {
            t.push(Entry {
                parent: 0,
                name: OsStr::new(&format!("f{i}")).into(),
                kind: Kind::File,
                size: i % 1000,
                alloc: i % 1000,
                mtime: 0,
            });
        }
        let l = layout(&snapshot(&t), &Params::default());
        let (a, b) = l.districts[0].tiles;
        assert!(b - a > 50, "{a}..{b}");
        let mut seen = vec![0u8; l.buildings.len()];
        for tile in &l.tiles[a as usize..b as usize] {
            for &i in &l.tile_items[tile.first as usize..tile.end as usize] {
                seen[i as usize] += 1;
                let r = l.buildings[i as usize].rect;
                assert!(
                    r.x0 >= tile.rect.x0
                        && r.x1 <= tile.rect.x1
                        && r.z0 >= tile.rect.z0
                        && r.z1 <= tile.rect.z1
                );
            }
            assert!(tile.height > 0.0 && tile.top >= tile.height * 0.0);
        }
        assert!(seen.iter().all(|&n| n == 1));
    }

    #[test]
    fn a_sparse_giant_leaves_the_others_room() {
        let mut t = Tree::new(OsStr::new("/"), 0);
        // 4 GB apparent, 4 KB on the disk
        t.push(Entry {
            parent: 0,
            name: OsStr::new("sparse.img").into(),
            kind: Kind::File,
            size: 4 << 30,
            alloc: 4096,
            mtime: 0,
        });
        for i in 0..200u64 {
            t.push(Entry {
                parent: 0,
                name: OsStr::new(&format!("f{i}")).into(),
                kind: Kind::File,
                size: 10_000,
                alloc: 12_288,
                mtime: 0,
            });
        }
        let l = layout(&snapshot(&t), &Params::default());
        let worst = l
            .buildings
            .iter()
            .map(|b| b.rect.aspect())
            .fold(0.0f32, f32::max);
        assert!(worst < 12.0, "no slivers: worst aspect {worst}");
        // by apparent size the giant takes most, but the others keep a
        // minimum footprint
        let p = Params {
            by_alloc: false,
            ..Params::default()
        };
        let l = layout(&snapshot(&t), &p);
        let giant = l
            .buildings
            .iter()
            .find(|b| b.node == 1)
            .unwrap()
            .rect
            .area();
        let small = l
            .buildings
            .iter()
            .filter(|b| b.node != 1)
            .map(|b| b.rect.area())
            .sum::<f32>();
        assert!(
            small > giant * 0.05,
            "the others keep room: {small} vs {giant}"
        );
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
                alloc: i % 1000,
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

    #[test]
    fn a_city_scales_as_a_whole() {
        let t = sample_for_scale();
        let l = layout(&snapshot(&t), &Params::default());
        let k = l.scaled(3.0);
        assert!((k.side - l.side * 3.0).abs() < 1e-3);
        for (a, b) in l.buildings.iter().zip(&k.buildings) {
            assert!((b.rect.x0 - a.rect.x0 * 3.0).abs() < 1e-3);
            assert!((b.height - a.height * 3.0).abs() < 1e-3);
        }
        assert_eq!(k.districts.len(), l.districts.len());
    }

    fn sample_for_scale() -> Tree {
        let mut t = Tree::new(std::ffi::OsStr::new("/"), 0);
        for i in 0..40u64 {
            t.push(crate::tree::Entry {
                parent: 0,
                name: std::ffi::OsString::from(format!("f{i}")).into_boxed_os_str(),
                kind: Kind::File,
                size: 1000 * (i + 1),
                alloc: 4096 * (i + 1),
                mtime: 0,
            });
        }
        t.recount();
        t
    }
}
