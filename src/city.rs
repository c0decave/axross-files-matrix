//! The world: the islands (each a laid-out tree) placed on the dark sea -
//! the machine's own in the middle, the local mounts in a ring round it,
//! axross's remote backends in a ring further out - their buildings as GPU
//! instances, the level of detail that picks what is drawn each frame, and
//! picking, the camera's collisions and the roofs under a flight path.

use crate::layout::{CityLayout, Placed};
use crate::math::{Aabb, Frustum, Ray, Vec3, vec3};
use crate::pick::Bvh;
use crate::theme::Theme;
use crate::tree::{Listing, NodeId, Tree};
use std::sync::Arc;

/// One box to draw (32 bytes, as the shader reads it).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    /// The corner (x, base y, z); a turned box turns round it.
    pub pos: [f32; 3],
    /// Bits 0-3 the shape ([`Shape`]), 4-7 flags ([`flags`]), 8-15 the
    /// glow, 16-31 the turn about y (a 16-bit angle).
    pub info: u32,
    /// Its extent along x, y (the height) and z.
    pub size: [f32; 3],
    /// RGBA8 (the type's colour).
    pub color: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Shape {
    Building = 0,
    /// A far district drawn as one block.
    Block = 1,
    /// A district's floor (its edges are the streets' lights).
    Plate = 2,
    /// An island's ground.
    Island = 3,
    /// The light between a disk and its mountpoint's island.
    Beam = 4,
    /// A search hit's pillar of light.
    Pillar = 5,
    /// The ring on the ground round a search hit.
    Ring = 6,
}

pub mod flags {
    /// A folder not read yet (a placeholder).
    pub const UNREAD: u32 = 1 << 4;
    /// A folder that could not be read.
    pub const DENIED: u32 = 1 << 5;
    /// The machine's island (its own look).
    pub const HARDWARE: u32 = 1 << 6;
    /// A remote backend's island.
    pub const REMOTE: u32 = 1 << 7;
}

pub fn pack_color(c: egui::Color32) -> u32 {
    u32::from_le_bytes([c.r(), c.g(), c.b(), 255])
}

impl Instance {
    pub fn new(shape: Shape, min: Vec3, size: Vec3, color: u32, glow: f32, flags: u32) -> Instance {
        Instance {
            pos: min.to_array(),
            info: shape as u32 | flags | (((glow.clamp(0.0, 1.0) * 255.0) as u32) << 8),
            size: size.to_array(),
            color,
        }
    }
    pub fn turned(mut self, angle: f32) -> Instance {
        let a = angle.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
        self.info = (self.info & 0xffff) | (((a * 65536.0) as u32 & 0xffff) << 16);
        self
    }
    pub fn shape(&self) -> u32 {
        self.info & 0xf
    }
    pub fn set_glow(&mut self, glow: f32) {
        self.info = (self.info & !0xff00) | (((glow.clamp(0.0, 1.0) * 255.0) as u32) << 8);
    }
    pub fn aabb(&self) -> Aabb {
        let min = vec3(self.pos[0], self.pos[1], self.pos[2]);
        Aabb {
            min,
            max: min + vec3(self.size[0], self.size[1], self.size[2]),
        }
    }
}

/// An island's kind (its look and where it lies).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ring {
    Hardware,
    Local,
    Remote,
}

/// An island's boxes in world coordinates, made off the window's thread
/// (the layout thread) for one layout at one place: per building, per
/// district (as a block from afar, as its floor), per tile, and the
/// island's ground.
#[derive(Clone, Debug, Default)]
pub struct IslandInstances {
    pub offset: [f32; 2],
    pub buildings: Vec<Instance>,
    pub blocks: Vec<Instance>,
    pub plates: Vec<Instance>,
    pub tiles: Vec<Instance>,
    /// The far districts' and tiles' towers (`CityLayout::tall`'s
    /// buildings, in its order: read in a row, not looked up one by one).
    pub towers: Vec<Instance>,
    pub ground: Instance,
}

fn ring_flags(ring: Ring) -> u32 {
    match ring {
        Ring::Hardware => flags::HARDWARE,
        Ring::Remote => flags::REMOTE,
        Ring::Local => 0,
    }
}

/// The instances of a layout placed at `offset`.
pub fn make_instances(
    l: &CityLayout,
    ring: Ring,
    offset: [f32; 2],
    theme: &Theme,
) -> IslandInstances {
    let [ox, oz] = offset;
    let ring_flags = ring_flags(ring);
    let listing_flags = |listing: Listing| {
        let unread = matches!(listing, Listing::Unlisted | Listing::Pending);
        let denied = matches!(listing, Listing::Denied | Listing::Failed);
        ring_flags | if unread { flags::UNREAD } else { 0 } | if denied { flags::DENIED } else { 0 }
    };
    let buildings: Vec<Instance> = l
        .buildings
        .iter()
        .map(|b| {
            Instance::new(
                Shape::Building,
                vec3(ox + b.rect.x0, b.base, oz + b.rect.z0),
                vec3(b.rect.w().max(0.0), b.height, b.rect.h().max(0.0)),
                pack_color(theme.type_color(b.ftype)),
                b.glow,
                ring_flags,
            )
        })
        .collect();
    let blocks = l
        .districts
        .iter()
        .map(|d| {
            let r = d.rect.shrunk(0.06);
            Instance::new(
                Shape::Block,
                vec3(ox + r.x0, d.floor, oz + r.z0),
                vec3(r.w().max(0.0), d.block_height, r.h().max(0.0)),
                pack_color(theme.type_color(d.ftype)),
                d.glow,
                listing_flags(d.listing),
            )
        })
        .collect();
    let plates = l
        .districts
        .iter()
        .map(|d| {
            let r = d.rect;
            Instance::new(
                Shape::Plate,
                vec3(ox + r.x0, d.floor - 0.05, oz + r.z0),
                vec3(r.w().max(0.0), 0.05, r.h().max(0.0)),
                pack_color(theme.type_color(crate::tree::FileType::Folder)),
                (d.depth as f32 / 12.0).min(1.0),
                listing_flags(d.listing),
            )
        })
        .collect();
    let tiles = l
        .tiles
        .iter()
        .map(|t| {
            Instance::new(
                Shape::Block,
                vec3(ox + t.rect.x0, t.base, oz + t.rect.z0),
                vec3(t.rect.w().max(0.0), t.height, t.rect.h().max(0.0)),
                pack_color(theme.type_color(t.ftype)),
                t.glow,
                ring_flags,
            )
        })
        .collect();
    let s = l.side.max(1.0);
    let ground = Instance::new(
        Shape::Island,
        vec3(ox - 4.0, -2.0, oz - 4.0),
        vec3(s + 8.0, 2.0, s + 8.0),
        pack_color(theme.type_color(crate::tree::FileType::Folder)),
        0.5,
        ring_flags,
    );
    let towers = l
        .tall
        .iter()
        .filter_map(|&b| buildings.get(b as usize).copied())
        .collect();
    IslandInstances {
        offset,
        buildings,
        blocks,
        plates,
        tiles,
        towers,
        ground,
    }
}

/// An island's world data: its layout, instances and picking.
pub struct IslandCity {
    pub ring: Ring,
    pub layout: Arc<CityLayout>,
    pub bvh: Arc<Bvh>,
    /// The buildings' boxes (island coordinates; the BVH's items).
    pub boxes: Arc<Vec<Aabb>>,
    /// Where the instances stand (picking and culling follow them).
    pub offset: [f32; 2],
    pub inst: Arc<IslandInstances>,
    /// This frame's level of detail: the collapsed districts (sorted by
    /// index; their building ranges are hidden) and tiles.
    pub collapsed: Vec<u32>,
    pub collapsed_tiles: Vec<u32>,
    /// The towers drawn over this frame's blocks (building indices):
    /// picked as they are drawn.
    pub towers_drawn: Vec<u32>,
    tile_hidden: Vec<bool>,
    /// Where the island is to stand (instances for it asked of the layout
    /// thread; None: where it stands).
    pub target: Option<[f32; 2]>,
    /// Drawn: a new island only once it stands where it belongs.
    pub shown: bool,
    /// Its ground's state ([`flags::UNREAD`]: connecting - its outline
    /// pulses; [`flags::DENIED`]: failed - amber hatching).
    pub state: u32,
}

/// The building boxes of a layout (island coordinates).
pub fn boxes_of(l: &CityLayout) -> Vec<Aabb> {
    l.buildings
        .iter()
        .map(|b| Aabb {
            min: vec3(b.rect.x0, b.base, b.rect.z0),
            max: vec3(b.rect.x1, b.base + b.height, b.rect.z1),
        })
        .collect()
}

impl IslandCity {
    pub fn new(
        ring: Ring,
        layout: Arc<CityLayout>,
        bvh: Arc<Bvh>,
        boxes: Arc<Vec<Aabb>>,
    ) -> IslandCity {
        let tiles = layout.tiles.len();
        IslandCity {
            ring,
            layout,
            bvh,
            boxes,
            offset: [0.0, 0.0],
            inst: Arc::new(IslandInstances::default()),
            collapsed: Vec::new(),
            collapsed_tiles: Vec::new(),
            towers_drawn: Vec::new(),
            tile_hidden: vec![false; tiles],
            target: None,
            shown: true,
            state: 0,
        }
    }

    pub fn side(&self) -> f32 {
        self.layout.side.max(1.0)
    }

    pub fn aabb(&self) -> Aabb {
        let [ox, oz] = self.offset;
        Aabb {
            min: vec3(ox - 4.0, -2.0, oz - 4.0),
            max: vec3(
                ox + self.side() + 4.0,
                self.layout.top.max(1.0),
                oz + self.side() + 4.0,
            ),
        }
    }

    pub fn center(&self) -> Vec3 {
        vec3(
            self.offset[0] + self.side() * 0.5,
            0.0,
            self.offset[1] + self.side() * 0.5,
        )
    }

    /// Makes the instances here, now (tests, the machine's small island;
    /// the window has the layout thread make them).
    pub fn rebuild(&mut self, theme: &Theme) {
        let inst = make_instances(&self.layout, self.ring, self.offset, theme);
        self.set_instances(Arc::new(inst));
    }

    /// Takes instances made for this layout; the island stands where they
    /// were made for.
    pub fn set_instances(&mut self, inst: Arc<IslandInstances>) {
        self.offset = inst.offset;
        if self.target.is_some_and(|t| t == inst.offset) {
            self.shown = true;
        }
        self.inst = inst;
    }

    /// Whether building `b` is hidden in a collapsed district or tile this
    /// frame.
    pub fn hidden(&self, b: u32) -> bool {
        if let Some(&t) = self.layout.tile_of.get(b as usize)
            && t != u32::MAX
            && self.tile_hidden.get(t as usize).copied().unwrap_or(false)
        {
            return true;
        }
        // the collapsed districts' building ranges are disjoint and in
        // order of their index - which is depth-first, so in order of range
        let ds = &self.layout.districts;
        let i = self
            .collapsed
            .partition_point(|&d| ds[d as usize].first <= b);
        i > 0 && {
            let d = &ds[self.collapsed[i - 1] as usize];
            b >= d.first && b < d.sub_end
        }
    }
}

/// A label the level of detail found worth showing.
#[derive(Clone, Copy, Debug)]
pub struct LabelCand {
    pub island: usize,
    pub node: NodeId,
    pub at: Vec3,
    /// How large it is on the screen (radians): the bigger, the first.
    pub weight: f32,
    pub kind: LabelKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelKind {
    Island,
    District,
    Building,
}

/// What the level of detail chose for a frame.
#[derive(Default)]
pub struct FrameSet {
    pub opaque: Vec<Instance>,
    pub glow: Vec<Instance>,
    pub labels: Vec<LabelCand>,
    /// Unread folders close enough to be read now: (island, node).
    pub want: Vec<(usize, NodeId)>,
    pub districts_drawn: usize,
    pub collapsed: usize,
}

/// The level of detail's knobs.
#[derive(Clone, Copy, Debug)]
pub struct Lod {
    /// A district smaller than this on the screen (radians) is one block.
    pub collapse: f32,
    /// An unread folder larger than this is read.
    pub open: f32,
    /// Labels: districts and buildings larger than these.
    pub label_district: f32,
    pub label_building: f32,
    /// How the shader raises far roofs and widens far towers
    /// ([`crate::render::Raise`]; NONE: not at all): what is culled is
    /// that high, the labels stand on the roofs drawn.
    pub raise: crate::render::Raise,
}

impl Default for Lod {
    fn default() -> Lod {
        Lod {
            // (a little sooner than without the far towers: as many boxes
            // as before them, the skyline kept)
            collapse: 0.052,
            open: 0.12,
            label_district: 0.2,
            label_building: 0.08,
            raise: crate::render::Raise::NONE,
        }
    }
}

fn distance_to(b: &Aabb, p: Vec3) -> f32 {
    let dx = (b.min.x - p.x).max(p.x - b.max.x).max(0.0);
    let dy = (b.min.y - p.y).max(p.y - b.max.y).max(0.0);
    let dz = (b.min.z - p.z).max(p.z - b.max.z).max(0.0);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Chooses what to draw: per island the districts in view, near ones
/// opened into their buildings, far ones as one block.
pub fn select(
    islands: &mut [&mut IslandCity],
    eye: Vec3,
    frustum: &Frustum,
    lod: &Lod,
    out: &mut FrameSet,
) {
    out.opaque.clear();
    out.glow.clear();
    out.labels.clear();
    out.want.clear();
    out.districts_drawn = 0;
    out.collapsed = 0;
    for (k, island) in islands.iter_mut().enumerate() {
        island.collapsed.clear();
        island.towers_drawn.clear();
        for &t in &island.collapsed_tiles {
            if let Some(h) = island.tile_hidden.get_mut(t as usize) {
                *h = false;
            }
        }
        island.collapsed_tiles.clear();
        if island.tile_hidden.len() != island.layout.tiles.len() {
            island.tile_hidden = vec![false; island.layout.tiles.len()];
        }
        // (culled as high as the shader may raise it; its distance and
        // size on the screen as it is - the level of detail unchanged)
        let ab = island.aabb();
        let mut raised = ab;
        raised.max.y *= crate::render::stretch_at(lod.raise.look, f32::MAX);
        if !island.shown || !frustum.intersects(&raised) {
            continue;
        }
        let inst = island.inst.clone();
        // (instances of another layout: not drawn until theirs come)
        if inst.blocks.len() != island.layout.districts.len()
            || inst.buildings.len() != island.layout.buildings.len()
            || inst.towers.len() != island.layout.tall.len()
        {
            continue;
        }
        let mut ground = inst.ground;
        ground.info |= island.state & (flags::UNREAD | flags::DENIED);
        out.opaque.push(ground);
        let size = island.side();
        let d = distance_to(&ab, eye).max(1.0);
        let [ox, oz] = island.offset;
        // the title at the island's edge nearest the eye (never floating
        // over the middle of the city one flies in)
        let margin = size * 0.08;
        out.labels.push(LabelCand {
            island: k,
            node: 0,
            at: vec3(
                eye.x.clamp(ox + margin, ox + size - margin),
                2.0,
                eye.z.clamp(oz + margin, oz + size - margin),
            ),
            weight: 10.0 + size / d,
            kind: LabelKind::Island,
        });
        let layout = island.layout.clone();
        let ds = &layout.districts;
        let mut i = 0usize;
        // (the end of the outermost district wholly in view: those below it
        // need no test)
        let mut inside_until = 0usize;
        while i < ds.len() {
            let dist = &ds[i];
            let r = dist.rect;
            let b = Aabb {
                min: vec3(ox + r.x0, 0.0, oz + r.z0),
                max: vec3(ox + r.x1, dist.top.max(dist.floor + 0.2), oz + r.z1),
            };
            let near = distance_to(&b, eye).max(0.5);
            // (in view as it is - most are - or as high as the shader may
            // draw its farthest corner: the raise's cost only for the rest;
            // inside a district wholly in view, no test at all)
            let inside = i < inside_until;
            if !inside && i > 0 && frustum.contains(&b) {
                inside_until = dist.end as usize;
            }
            if !inside && !frustum.intersects(&b) {
                let mut raised = b;
                let reach = near + r.w() + r.h();
                raised.max.y *= crate::render::stretch_at(lod.raise.look, reach);
                if !frustum.intersects(&raised) {
                    i = dist.end as usize;
                    continue;
                }
            }
            let extent = r.w().max(r.h());
            let angle = extent / near;
            let has_content = dist.sub_end > dist.first || dist.end as usize > i + 1;
            let unread = matches!(dist.listing, Listing::Unlisted | Listing::Pending);
            if unread && angle > lod.open && dist.listing == Listing::Unlisted {
                out.want.push((k, dist.node));
            }
            // (the root too: a whole island far off is one block)
            if angle < lod.collapse || (i > 0 && !has_content) {
                out.opaque.push(inst.blocks[i]);
                // its tallest buildings still stand over the block
                towers(
                    &layout,
                    &inst,
                    dist.tall,
                    near,
                    angle < lod.collapse * 0.5,
                    &mut out.opaque,
                    &mut island.towers_drawn,
                );
                island.collapsed.push(i as u32);
                out.collapsed += 1;
                i = dist.end as usize;
                continue;
            }
            out.districts_drawn += 1;
            if i > 0 {
                out.opaque.push(inst.plates[i]);
                if angle > lod.label_district {
                    let (cx, cz) = r.center();
                    out.labels.push(LabelCand {
                        island: k,
                        node: dist.node,
                        at: vec3(ox + cx, dist.floor + 0.5, oz + cz),
                        weight: angle,
                        kind: LabelKind::District,
                    });
                }
            }
            let own = dist.first as usize..dist.own_end as usize;
            if dist.tiles.1 > dist.tiles.0 {
                // a big district: tile by tile, far ones as one block
                for t in dist.tiles.0..dist.tiles.1 {
                    let tile = &layout.tiles[t as usize];
                    let tr = tile.rect;
                    let tb = Aabb {
                        min: vec3(ox + tr.x0, tile.base, oz + tr.z0),
                        max: vec3(ox + tr.x1, tile.top.max(tile.base + 0.2), oz + tr.z1),
                    };
                    let tnear = distance_to(&tb, eye).max(0.5);
                    if !frustum.intersects(&tb) {
                        let mut raised = tb;
                        raised.max.y *=
                            crate::render::stretch_at(lod.raise.look, tnear + tr.w() + tr.h());
                        if !frustum.intersects(&raised) {
                            continue;
                        }
                    }
                    let ta = tr.w().max(tr.h()) / tnear;
                    if ta < lod.collapse {
                        out.opaque.push(inst.tiles[t as usize]);
                        towers(
                            &layout,
                            &inst,
                            tile.tall,
                            tnear,
                            ta < lod.collapse * 0.5,
                            &mut out.opaque,
                            &mut island.towers_drawn,
                        );
                        island.collapsed_tiles.push(t);
                        island.tile_hidden[t as usize] = true;
                        out.collapsed += 1;
                    } else {
                        for &bi in &layout.tile_items[tile.first as usize..tile.end as usize] {
                            out.opaque.push(inst.buildings[bi as usize]);
                        }
                    }
                }
            } else {
                out.opaque.extend_from_slice(&inst.buildings[own.clone()]);
            }
            // labels for the buildings that are big on the screen (checked
            // for the larger ones only: they come largest first)
            if angle > lod.label_building * 2.0 {
                for (bi, b) in layout.buildings[own.clone()].iter().enumerate().take(12) {
                    if island.hidden((own.start + bi) as u32) {
                        continue;
                    }
                    let (cx, cz) = b.rect.center();
                    // (on the roof as drawn: raised far away)
                    let (rmin, rsize) = lod.raise.apply(
                        0,
                        island.ring == Ring::Hardware,
                        vec3(ox + b.rect.x0, b.base, oz + b.rect.z0),
                        vec3(b.rect.w().max(0.0), b.height, b.rect.h().max(0.0)),
                    );
                    let top = vec3(ox + cx, rmin.y + rsize.y, oz + cz);
                    let s = b.rect.w().max(b.rect.h()).max(b.height * 0.5);
                    let a = s / (top - eye).length().max(0.5);
                    if a > lod.label_building {
                        out.labels.push(LabelCand {
                            island: k,
                            node: b.node,
                            at: top + vec3(0.0, 0.3, 0.0),
                            weight: a,
                            kind: LabelKind::Building,
                        });
                    }
                }
            }
            i += 1;
        }
    }
}

/// A tower this small on the screen (radians, its height) is not drawn
/// over its block: about three pixels of a 800-pixel-high view.
const TOWER_SEEN: f32 = 0.0045;

/// The towers `tall` of a far district or tile `near` away (tallest
/// first: the first too small to see ends them); a district less than half
/// the size that collapses (`small`) keeps only its tallest - the skyline
/// at the price of a few boxes.
fn towers(
    layout: &CityLayout,
    inst: &IslandInstances,
    tall: (u32, u32),
    near: f32,
    small: bool,
    out: &mut Vec<Instance>,
    drawn: &mut Vec<u32>,
) {
    let seen = TOWER_SEEN * near;
    let range = tall.0 as usize..tall.1 as usize;
    let all = inst.towers.get(range.clone()).unwrap_or(&[]);
    let which = layout.tall.get(range).unwrap_or(&[]);
    for (i, b) in all
        .iter()
        .zip(which)
        .take(if small { 1 } else { all.len() })
    {
        if i.size[1] <= seen {
            break;
        }
        out.push(*i);
        drawn.push(*b);
    }
}

/// The most boxes the window draws a frame (about 5 MB of instances).
pub const FRAME_BUDGET: usize = 160_000;

/// [`select`] within a budget of boxes: where it chose more, again with
/// far things collapsing sooner (at most three times). The knob it ended
/// with.
pub fn select_within(
    islands: &mut [&mut IslandCity],
    eye: Vec3,
    frustum: &Frustum,
    lod: &Lod,
    budget: usize,
    out: &mut FrameSet,
) -> Lod {
    let mut l = *lod;
    select(islands, eye, frustum, &l, out);
    for _ in 0..4 {
        if out.opaque.len() <= budget {
            return l;
        }
        let over = out.opaque.len() as f32 / budget.max(1) as f32;
        // (from at least a small knob: no cliff from all opened to one block)
        let from = l.collapse.max(0.01);
        l.collapse = (from * over.sqrt() * 1.3).max(from * 1.3);
        select(islands, eye, frustum, &l, out);
    }
    if out.opaque.len() > budget {
        // (the hard bound: every district a block, every island its root)
        l.collapse = f32::MAX;
        select(islands, eye, frustum, &l, out);
    }
    l
}

/// What a ray hits first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub island: usize,
    pub node: NodeId,
    pub t: f32,
    /// The box it hit (world coordinates: the highlight).
    pub aabb: Aabb,
}

/// The first building, collapsed block or district floor along `ray` -
/// as drawn (`raise`: far roofs raised, far towers widened, as the shader
/// does; the hit's box is the thing's own, for the highlight).
pub fn pick(islands: &[&IslandCity], ray: &Ray, raise: &crate::render::Raise) -> Option<Hit> {
    let mut best: Option<Hit> = None;
    let most = raise.look[0].max(1.0);
    for (k, island) in islands.iter().enumerate() {
        let hw = island.ring == Ring::Hardware;
        let mut ab = island.aabb();
        ab.max.y *= if hw { 1.0 } else { most };
        if !island.shown || ray.hit_aabb(&ab, f32::MAX).is_none() {
            continue;
        }
        let [ox, oz] = island.offset;
        let off = vec3(ox, 0.0, oz);
        let local = Ray {
            origin: ray.origin - off,
            dir: ray.dir,
        };
        let here = crate::render::Raise {
            eye: raise.eye - off,
            ..*raise
        };
        let limit = best.map_or(f32::MAX, |h| h.t);
        let keep = |b: u32| !island.hidden(b);
        let item = |b: u32| here.aabb(0, hw, island.boxes[b as usize]);
        // (a node grown as its items may be: higher, and wider by a pixel
        // at its far side)
        let grow = |n: &Aabb| {
            if hw {
                return *n;
            }
            let c = n.center();
            let far = ((c.x - here.eye.x).powi(2) + (c.z - here.eye.z).powi(2)).sqrt()
                + (n.max.x - n.min.x).max(n.max.z - n.min.z);
            let px = far * here.lens / here.height.max(1.0);
            Aabb {
                min: vec3(n.min.x - px, n.min.y, n.min.z - px),
                max: vec3(n.max.x + px, n.max.y.max(0.0) * most + 0.01, n.max.z + px),
            }
        };
        let found = |b: u32, t: f32, best: &mut Option<Hit>| {
            let bld = &island.layout.buildings[b as usize];
            let mut aabb = island.boxes[b as usize];
            aabb.min += off;
            aabb.max += off;
            *best = Some(Hit {
                island: k,
                node: bld.node,
                t,
                aabb,
            });
        };
        if let Some((b, t)) = island.bvh.raycast_drawn(&local, limit, &grow, &item, &keep) {
            found(b, t, &mut best);
        }
        // the towers standing over far blocks, as drawn
        for &b in &island.towers_drawn {
            let Some(bx) = island.boxes.get(b as usize) else {
                continue;
            };
            if let Some(t) = local.hit_aabb(&here.aabb(0, hw, *bx), best.map_or(f32::MAX, |h| h.t))
            {
                found(b, t, &mut best);
            }
        }
        for &t in &island.collapsed_tiles {
            let Some(inst) = island.inst.tiles.get(t as usize) else {
                continue;
            };
            let aabb = inst.aabb();
            if let Some(tt) = ray.hit_aabb(&raise.aabb(1, hw, aabb), best.map_or(f32::MAX, |h| h.t))
            {
                let tile = &island.layout.tiles[t as usize];
                best = Some(Hit {
                    island: k,
                    node: island.layout.districts[tile.district as usize].node,
                    t: tt,
                    aabb,
                });
            }
        }
        for &d in &island.collapsed {
            let Some(inst) = island.inst.blocks.get(d as usize) else {
                continue;
            };
            let aabb = inst.aabb();
            if let Some(t) = ray.hit_aabb(&raise.aabb(1, hw, aabb), best.map_or(f32::MAX, |h| h.t))
            {
                best = Some(Hit {
                    island: k,
                    node: island.layout.districts[d as usize].node,
                    t,
                    aabb,
                });
            }
        }
    }
    if best.is_none() {
        best = pick_ground(islands, ray);
    }
    best
}

/// The deepest district drawn under the point where the ray meets the
/// ground.
fn pick_ground(islands: &[&IslandCity], ray: &Ray) -> Option<Hit> {
    if ray.dir.y >= -1e-4 {
        return None;
    }
    let t = -ray.origin.y / ray.dir.y;
    let p = ray.at(t);
    for (k, island) in islands.iter().enumerate() {
        let [ox, oz] = island.offset;
        let (x, z) = (p.x - ox, p.z - oz);
        let ds = &island.layout.districts;
        if ds.is_empty()
            || !(0.0..=island.side()).contains(&x)
            || !(0.0..=island.side()).contains(&z)
        {
            continue;
        }
        // down the tree while a child holds the point
        let mut at = 0usize;
        'down: loop {
            if island.collapsed.binary_search(&(at as u32)).is_ok() {
                break;
            }
            let mut c = at + 1;
            while c < ds[at].end as usize {
                let r = ds[c].rect;
                if x >= r.x0 && x <= r.x1 && z >= r.z0 && z <= r.z1 {
                    at = c;
                    continue 'down;
                }
                c = ds[c].end as usize;
            }
            break;
        }
        let r = ds[at].rect;
        // (the district's plate, as drawn: its highlight)
        let aabb = Aabb {
            min: vec3(ox + r.x0, ds[at].floor - 0.05, oz + r.z0),
            max: vec3(ox + r.x1, ds[at].floor, oz + r.z1),
        };
        return Some(Hit {
            island: k,
            node: ds[at].node,
            t,
            aabb,
        });
    }
    None
}

/// Whether a building stands between `eye` and `at` (a label there is
/// hidden behind it).
pub fn occluded(islands: &[&IslandCity], eye: Vec3, at: Vec3) -> bool {
    let d = at - eye;
    let dist = d.length();
    if dist < 1e-3 {
        return false;
    }
    let ray = Ray {
        origin: eye,
        dir: d * (1.0 / dist),
    };
    // (a little short of the point: its own building is not in the way)
    let limit = dist * 0.97 - 0.5;
    islands.iter().any(|island| {
        if ray.hit_aabb(&island.aabb(), limit).is_none() {
            return false;
        }
        let [ox, oz] = island.offset;
        let local = Ray {
            origin: eye - vec3(ox, 0.0, oz),
            dir: ray.dir,
        };
        island
            .bvh
            .raycast(&local, &island.boxes, limit, &|b| !island.hidden(b))
            .is_some()
    })
}

/// Whether the camera at `p` would be inside a building (or a block).
pub fn solid(islands: &[&IslandCity], p: Vec3) -> bool {
    islands.iter().any(|island| {
        if !island.aabb().grown(1.0).contains(p) {
            return false;
        }
        let [ox, oz] = island.offset;
        let local = p - vec3(ox, 0.0, oz);
        let grown = |b: u32| !island.hidden(b);
        // (a little room round the walls)
        let near = [
            vec3(0.0, 0.0, 0.0),
            vec3(0.4, 0.0, 0.0),
            vec3(-0.4, 0.0, 0.0),
            vec3(0.0, 0.0, 0.4),
            vec3(0.0, 0.0, -0.4),
        ];
        near.iter()
            .any(|d| island.bvh.contains(local + *d, &island.boxes, &grown))
            || island.collapsed.iter().any(|&d| {
                island
                    .inst
                    .blocks
                    .get(d as usize)
                    .is_some_and(|b| b.aabb().grown(0.4).contains(p))
            })
    })
}

/// The highest roof under the line from `a` to `b` (on the ground plane):
/// districts down to the fourth level whose ground the line's box touches.
pub fn ceiling(islands: &[&IslandCity], a: Vec3, b: Vec3) -> f32 {
    let (x0, x1) = (a.x.min(b.x) - 2.0, a.x.max(b.x) + 2.0);
    let (z0, z1) = (a.z.min(b.z) - 2.0, a.z.max(b.z) + 2.0);
    let mut top = 0.0f32;
    for island in islands {
        let ab = island.aabb();
        if ab.max.x < x0 || ab.min.x > x1 || ab.max.z < z0 || ab.min.z > z1 {
            continue;
        }
        let [ox, oz] = island.offset;
        let ds = &island.layout.districts;
        let mut i = 0usize;
        while i < ds.len() {
            let d = &ds[i];
            let r = d.rect;
            let miss = ox + r.x1 < x0 || ox + r.x0 > x1 || oz + r.z1 < z0 || oz + r.z0 > z1;
            if miss || d.top <= top {
                i = d.end as usize;
                continue;
            }
            if d.depth >= 4 || d.end as usize == i + 1 {
                top = top.max(d.top);
                i = d.end as usize;
                continue;
            }
            // its own buildings, then its children
            for bld in &island.layout.buildings[d.first as usize..d.own_end as usize] {
                let r = bld.rect;
                if !(ox + r.x1 < x0 || ox + r.x0 > x1 || oz + r.z1 < z0 || oz + r.z0 > z1) {
                    top = top.max(bld.base + bld.height);
                }
            }
            i += 1;
        }
    }
    top
}

/// Places the islands: the machine in the middle, the local mounts in a
/// ring round it, the remote backends in a ring further out; each ring
/// wide enough for its islands side by side. Returns the offsets.
pub fn place(sides: &[(Ring, f32)]) -> Vec<[f32; 2]> {
    let mut out = vec![[0.0, 0.0]; sides.len()];
    let gap = 60.0;
    let hw: Vec<usize> = (0..sides.len())
        .filter(|&i| sides[i].0 == Ring::Hardware)
        .collect();
    let mut inner = 0.0f32;
    for &i in &hw {
        let s = sides[i].1;
        out[i] = [-s * 0.5, -s * 0.5];
        inner = inner.max(s * 0.5);
    }
    for ring in [Ring::Local, Ring::Remote] {
        let members: Vec<usize> = (0..sides.len()).filter(|&i| sides[i].0 == ring).collect();
        if members.is_empty() {
            continue;
        }
        let widest = members.iter().map(|&i| sides[i].1).fold(0.0, f32::max);
        // (the half-diagonals side by side round the circle)
        let around: f32 = members
            .iter()
            .map(|&i| sides[i].1 * std::f32::consts::SQRT_2 + gap)
            .sum();
        let radius = (inner + gap + widest * 0.75).max(around / std::f32::consts::TAU);
        let mut angle = -std::f32::consts::FRAC_PI_2;
        for &i in &members {
            let s = sides[i].1;
            let share = (s * std::f32::consts::SQRT_2 + gap) / (radius * std::f32::consts::TAU);
            let a = angle + share * std::f32::consts::PI;
            let (cx, cz) = (radius * a.cos(), radius * a.sin());
            out[i] = [cx - s * 0.5, cz - s * 0.5];
            angle += share * std::f32::consts::TAU;
        }
        inner = radius + widest * 0.75;
    }
    out
}

/// The light from a disk on the machine's island to an island: a thin,
/// turned box at the ground.
pub fn beam(from: Vec3, to: Vec3, color: u32) -> Instance {
    let d = to - from;
    let len = (d.x * d.x + d.z * d.z).sqrt();
    let angle = d.z.atan2(d.x);
    // the shader turns x into the direction (cos, sin) of the angle
    Instance::new(
        Shape::Beam,
        vec3(from.x, 0.25, from.z),
        vec3(len, 0.5, 1.2),
        color,
        0.8,
        0,
    )
    .turned(-angle)
}

/// The world position of node `id`'s building, block or district.
pub fn node_box(island: &IslandCity, id: NodeId) -> Option<Aabb> {
    let [ox, oz] = island.offset;
    let off = vec3(ox, 0.0, oz);
    match island.layout.placed.get(id as usize)? {
        Placed::Building(b) => {
            let mut a = island.boxes[*b as usize];
            a.min += off;
            a.max += off;
            Some(a)
        }
        Placed::District(d) => {
            let dist = &island.layout.districts[*d as usize];
            let r = dist.rect;
            Some(Aabb {
                min: vec3(ox + r.x0, dist.floor, oz + r.z0),
                max: vec3(
                    ox + r.x1,
                    dist.top.max(dist.floor + dist.block_height),
                    oz + r.z1,
                ),
            })
        }
        Placed::Nowhere => None,
    }
}

/// The node or its nearest ancestor that the layout placed.
pub fn placed_ancestor(island: &IslandCity, tree: &Tree, id: NodeId) -> Option<NodeId> {
    let mut at = id;
    while at != crate::tree::NONE {
        if island
            .layout
            .placed
            .get(at as usize)
            .is_some_and(|p| *p != Placed::Nowhere)
        {
            return Some(at);
        }
        at = tree.get(at)?.parent;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Params, layout, snapshot};
    use crate::math::Mat4;
    use crate::tree::{Entry, Kind};
    use std::ffi::OsStr;

    fn island_of(files: usize) -> (Tree, IslandCity) {
        let mut t = Tree::new(OsStr::new("/"), 0);
        for e in crate::walk::synthetic_entries(files, 1) {
            t.push(e);
        }
        for n in &mut t.nodes {
            if n.kind == Kind::Dir {
                n.listing = Listing::Listed;
            }
        }
        let l = Arc::new(layout(&snapshot(&t), &Params::default()));
        let boxes = Arc::new(boxes_of(&l));
        let bvh = Arc::new(Bvh::build(&boxes));
        let mut c = IslandCity::new(Ring::Local, l, bvh, boxes);
        c.rebuild(&crate::theme::MATRIX);
        (t, c)
    }

    fn view(eye: Vec3, at: Vec3) -> Frustum {
        let f = (at - eye).normalized();
        let v = Mat4::look_to(eye, f, Vec3::Y);
        let p = Mat4::perspective_reversed_infinite(1.0, 1.6, 0.1);
        Frustum::from_view_proj(&p.mul(&v))
    }

    #[test]
    fn far_towers_stand_and_raising_changes_no_level_of_detail() {
        let (_, mut c) = island_of(20_000);
        let center = c.center();
        let eye = center + vec3(0.0, 900.0, 2200.0);
        let lod = Lod::default();
        let mut flat = FrameSet::default();
        select(&mut [&mut c], eye, &view(eye, center), &lod, &mut flat);
        let collapsed = flat.collapsed;
        assert!(collapsed > 0, "some districts far enough to be blocks");
        // the collapsed districts' towers are drawn over their blocks
        let blocks = flat
            .opaque
            .iter()
            .filter(|i| i.shape() == Shape::Block as u32)
            .count();
        let towers_possible: usize = c
            .collapsed
            .iter()
            .map(|&d| {
                let t = c.layout.districts[d as usize].tall;
                (t.1 - t.0) as usize
            })
            .sum();
        assert!(blocks > 0 && towers_possible > 0);
        let buildings = flat
            .opaque
            .iter()
            .filter(|i| i.shape() == Shape::Building as u32)
            .count();
        assert!(buildings > 0, "towers over the far blocks");
        // raised roofs (the shader's stretch): the same choice of what is
        // a block and what opens - only the culling is taller
        let raised = Lod {
            raise: crate::render::Raise {
                look: crate::render::far_look(c.side(), c.layout.top),
                eye,
                ..crate::render::Raise::NONE
            },
            ..lod
        };
        let mut tall = FrameSet::default();
        select(&mut [&mut c], eye, &view(eye, center), &raised, &mut tall);
        // (a few more pass the culling: their raised roofs reach the view)
        assert!(tall.collapsed >= collapsed && tall.collapsed <= collapsed + collapsed / 10 + 2);
        assert!(tall.districts_drawn >= flat.districts_drawn);
        assert!(tall.districts_drawn <= flat.districts_drawn + flat.districts_drawn / 10 + 2);
        assert!(tall.opaque.len() >= flat.opaque.len());
        assert!(tall.opaque.len() <= flat.opaque.len() + flat.opaque.len() / 10);
    }

    #[test]
    fn far_away_collapses_near_opens() {
        let (_, mut c) = island_of(20_000);
        let center = c.center();
        let lod = Lod::default();
        let mut far = FrameSet::default();
        let eye_far = center + vec3(0.0, 20_000.0, 20_000.0);
        select(
            &mut [&mut c],
            eye_far,
            &view(eye_far, center),
            &lod,
            &mut far,
        );
        let mut near = FrameSet::default();
        let eye_near = center + vec3(0.0, 40.0, 60.0);
        select(
            &mut [&mut c],
            eye_near,
            &view(eye_near, center),
            &lod,
            &mut near,
        );
        assert!(far.opaque.len() < 200, "far: {}", far.opaque.len());
        assert!(
            near.opaque.len() > far.opaque.len() * 5,
            "near {} far {}",
            near.opaque.len(),
            far.opaque.len()
        );
        // all from above: the whole island, every building of the open districts
        let mut all = FrameSet::default();
        let lod_all = Lod {
            collapse: 0.0,
            ..lod
        };
        let eye_top = center + vec3(0.0, 3000.0, 1.0);
        select(
            &mut [&mut c],
            eye_top,
            &view(eye_top, center),
            &lod_all,
            &mut all,
        );
        let buildings = all
            .opaque
            .iter()
            .filter(|i| i.shape() == Shape::Building as u32)
            .count();
        assert_eq!(buildings, c.layout.buildings.len());
        assert!(c.collapsed.iter().all(|&d| d > 0));
    }

    #[test]
    fn behind_the_camera_nothing_is_drawn() {
        let (_, mut c) = island_of(2000);
        let center = c.center();
        let eye = center + vec3(0.0, 10.0, 5000.0);
        let away = eye + vec3(0.0, 0.0, 10.0);
        let mut f = FrameSet::default();
        select(
            &mut [&mut c],
            eye,
            &view(eye, away),
            &Lod::default(),
            &mut f,
        );
        assert!(f.opaque.is_empty());
    }

    #[test]
    fn picking_finds_the_building_under_the_ray() {
        let (t, mut c) = island_of(3000);
        // the tallest building, from straight above
        let (bi, b) = c
            .layout
            .buildings
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.height.total_cmp(&b.1.height))
            .map(|(i, b)| (i, *b))
            .unwrap();
        let (cx, cz) = b.rect.center();
        let top = vec3(cx, b.base + b.height + 50.0, cz);
        let mut f = FrameSet::default();
        let lod = Lod {
            collapse: 0.0,
            ..Default::default()
        };
        select(
            &mut [&mut c],
            top,
            &view(top, top - vec3(0.0, 1.0, 0.001)),
            &lod,
            &mut f,
        );
        let ray = Ray {
            origin: top,
            dir: vec3(0.0, -1.0, 0.0),
        };
        let hit = pick(&[&c], &ray, &crate::render::Raise::NONE).unwrap();
        assert_eq!(hit.node, b.node);
        assert!((hit.t - 50.0).abs() < 1e-2);
        assert!(t.get(hit.node).is_some());
        assert!(!c.hidden(bi as u32));
        // inside it is solid; high above is not
        assert!(solid(&[&c], vec3(cx, b.base + b.height * 0.5, cz)));
        assert!(!solid(&[&c], top));
        // a flight over it clears it
        assert!(
            ceiling(&[&c], vec3(cx - 5.0, 0.0, cz), vec3(cx + 5.0, 0.0, cz)) >= b.base + b.height
        );
        // the ground under an empty spot: a district
        let miss = Ray {
            origin: vec3(-100.0, 10.0, -100.0),
            dir: vec3(0.0, -1.0, 0.0),
        };
        assert!(pick(&[&c], &miss, &crate::render::Raise::NONE).is_none());
    }

    #[test]
    fn hidden_follows_the_collapsed_ranges() {
        let (_, mut c) = island_of(3000);
        let ds = c.layout.districts.clone();
        let d = (1..ds.len())
            .find(|&i| ds[i].sub_end > ds[i].first + 2)
            .unwrap();
        c.collapsed = vec![d as u32];
        assert!(c.hidden(ds[d].first));
        assert!(c.hidden(ds[d].sub_end - 1));
        assert!(!c.hidden(ds[d].sub_end));
        if ds[d].first > 0 {
            assert!(!c.hidden(ds[d].first - 1));
        }
    }

    #[test]
    fn islands_do_not_overlap() {
        let mut sides = vec![(Ring::Hardware, 220.0)];
        for s in [2000.0, 40.0, 300.0, 300.0, 90.0, 1200.0] {
            sides.push((Ring::Local, s));
        }
        for s in [500.0, 30.0] {
            sides.push((Ring::Remote, s));
        }
        let at = place(&sides);
        for i in 0..sides.len() {
            for j in i + 1..sides.len() {
                let (a, b) = (at[i], at[j]);
                let (sa, sb) = (sides[i].1, sides[j].1);
                let apart = a[0] + sa <= b[0]
                    || b[0] + sb <= a[0]
                    || a[1] + sa <= b[1]
                    || b[1] + sb <= a[1];
                assert!(apart, "{i} and {j} overlap: {a:?} {sa} / {b:?} {sb}");
            }
        }
    }

    #[test]
    fn instances_pack_their_bits() {
        let i = Instance::new(
            Shape::Pillar,
            Vec3::ZERO,
            vec3(1.0, 2.0, 3.0),
            0xff00ff00,
            1.0,
            flags::UNREAD,
        )
        .turned(std::f32::consts::PI);
        assert_eq!(i.shape(), Shape::Pillar as u32);
        assert_eq!((i.info >> 8) & 0xff, 255);
        assert_eq!(i.info & flags::UNREAD, flags::UNREAD);
        assert_eq!(i.info >> 16, 32768);
        assert_eq!(std::mem::size_of::<Instance>(), 32);
        let _ = Entry {
            parent: 0,
            name: OsStr::new("x").into(),
            kind: Kind::File,
            size: 0,
            alloc: 0,
            mtime: 0,
        };
    }
}
