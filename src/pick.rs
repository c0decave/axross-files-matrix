//! A bounding volume hierarchy over boxes (median splits on the longest
//! axis, a few boxes per leaf): what the mouse points at - a ray against
//! every building of an island in microseconds - and whether the camera
//! would fly into a building.

use crate::math::{Aabb, Ray, Vec3};

#[derive(Clone, Copy, Debug)]
struct Node {
    aabb: Aabb,
    /// A leaf: the first item and how many; inside: the left child (the
    /// right one follows its subtree) and 0.
    first: u32,
    count: u32,
    /// Inside: the right child's index.
    right: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Bvh {
    nodes: Vec<Node>,
    items: Vec<u32>,
}

const LEAF: usize = 6;

impl Bvh {
    pub fn build(boxes: &[Aabb]) -> Bvh {
        let mut bvh = Bvh {
            nodes: Vec::with_capacity(boxes.len() / 2 + 1),
            items: (0..boxes.len() as u32).collect(),
        };
        // (each box's middle once: the splits compare them many times)
        let mids: Vec<[f32; 3]> = boxes
            .iter()
            .map(|b| {
                let c = b.center();
                [c.x, c.y, c.z]
            })
            .collect();
        if !boxes.is_empty() {
            let mut stack = vec![(0usize, boxes.len(), usize::MAX)];
            // (iterative: a deep tree never overflows the stack)
            while let Some((start, end, parent_right_slot)) = stack.pop() {
                let index = bvh.nodes.len();
                if parent_right_slot != usize::MAX {
                    bvh.nodes[parent_right_slot].right = index as u32;
                }
                let mut aabb = Aabb::EMPTY;
                let mut centers = Aabb::EMPTY;
                for &i in &bvh.items[start..end] {
                    aabb = aabb.union(boxes[i as usize]);
                    let [x, y, z] = mids[i as usize];
                    let c = crate::math::vec3(x, y, z);
                    centers = centers.union(Aabb { min: c, max: c });
                }
                if end - start <= LEAF {
                    bvh.nodes.push(Node {
                        aabb,
                        first: start as u32,
                        count: (end - start) as u32,
                        right: 0,
                    });
                    continue;
                }
                let ext = centers.max - centers.min;
                let axis = if ext.x >= ext.y && ext.x >= ext.z {
                    0
                } else if ext.y >= ext.z {
                    1
                } else {
                    2
                };
                let key = |i: &u32| mids[*i as usize][axis];
                let mid = (start + end) / 2;
                bvh.items[start..end]
                    .select_nth_unstable_by(mid - start, |a, b| key(a).total_cmp(&key(b)));
                bvh.nodes.push(Node {
                    aabb,
                    first: 0,
                    count: 0,
                    right: 0,
                });
                // the left child comes next (popped first), the right after
                stack.push((mid, end, index));
                stack.push((start, mid, usize::MAX));
            }
        }
        bvh
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The nearest box the ray enters (within `max`) for which `keep`
    /// holds: its index and distance.
    pub fn raycast(
        &self,
        ray: &Ray,
        boxes: &[Aabb],
        max: f32,
        keep: &dyn Fn(u32) -> bool,
    ) -> Option<(u32, f32)> {
        let mut best: Option<(u32, f32)> = None;
        let mut limit = max;
        let mut stack = Vec::with_capacity(64);
        if !self.nodes.is_empty() {
            stack.push(0usize);
        }
        while let Some(n) = stack.pop() {
            let node = &self.nodes[n];
            if ray.hit_aabb(&node.aabb, limit).is_none() {
                continue;
            }
            if node.count > 0 {
                for &i in &self.items[node.first as usize..(node.first + node.count) as usize] {
                    if let Some(t) = ray.hit_aabb(&boxes[i as usize], limit)
                        && keep(i)
                    {
                        limit = t;
                        best = Some((i, t));
                    }
                }
            } else {
                stack.push(node.right as usize);
                stack.push(n + 1);
            }
        }
        best
    }

    /// [`Bvh::raycast`] against the boxes as drawn: `item(i)` gives box
    /// `i` as the shader draws it (raised, widened), `grow(node)` a node's
    /// box grown enough to hold its items so drawn.
    pub fn raycast_drawn(
        &self,
        ray: &Ray,
        max: f32,
        grow: &dyn Fn(&Aabb) -> Aabb,
        item: &dyn Fn(u32) -> Aabb,
        keep: &dyn Fn(u32) -> bool,
    ) -> Option<(u32, f32)> {
        let mut best: Option<(u32, f32)> = None;
        let mut limit = max;
        let mut stack = Vec::with_capacity(64);
        if !self.nodes.is_empty() {
            stack.push(0usize);
        }
        while let Some(n) = stack.pop() {
            let node = &self.nodes[n];
            if ray.hit_aabb(&grow(&node.aabb), limit).is_none() {
                continue;
            }
            if node.count > 0 {
                for &i in &self.items[node.first as usize..(node.first + node.count) as usize] {
                    if let Some(t) = ray.hit_aabb(&item(i), limit)
                        && keep(i)
                    {
                        limit = t;
                        best = Some((i, t));
                    }
                }
            } else {
                stack.push(node.right as usize);
                stack.push(n + 1);
            }
        }
        best
    }

    /// Whether `p` lies in any box for which `keep` holds.
    pub fn contains(&self, p: Vec3, boxes: &[Aabb], keep: &dyn Fn(u32) -> bool) -> bool {
        let mut stack = Vec::with_capacity(64);
        if !self.nodes.is_empty() {
            stack.push(0usize);
        }
        while let Some(n) = stack.pop() {
            let node = &self.nodes[n];
            if !node.aabb.contains(p) {
                continue;
            }
            if node.count > 0 {
                for &i in &self.items[node.first as usize..(node.first + node.count) as usize] {
                    if boxes[i as usize].contains(p) && keep(i) {
                        return true;
                    }
                }
            } else {
                stack.push(node.right as usize);
                stack.push(n + 1);
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::vec3;
    use crate::walk::Rng;

    fn boxes(n: usize) -> Vec<Aabb> {
        let mut rng = Rng::new(9);
        (0..n)
            .map(|_| {
                let x = rng.unit() as f32 * 1000.0;
                let z = rng.unit() as f32 * 1000.0;
                let w = 0.5 + rng.unit() as f32 * 5.0;
                let h = 1.0 + rng.unit() as f32 * 50.0;
                Aabb {
                    min: vec3(x, 0.0, z),
                    max: vec3(x + w, h, z + w),
                }
            })
            .collect()
    }

    fn brute(ray: &Ray, b: &[Aabb], keep: &dyn Fn(u32) -> bool) -> Option<(u32, f32)> {
        let mut best: Option<(u32, f32)> = None;
        for (i, bx) in b.iter().enumerate() {
            if let Some(t) = ray.hit_aabb(bx, f32::MAX)
                && keep(i as u32)
                && best.is_none_or(|(_, bt)| t < bt)
            {
                best = Some((i as u32, t));
            }
        }
        best
    }

    #[test]
    fn agrees_with_brute_force() {
        let b = boxes(5000);
        let bvh = Bvh::build(&b);
        let mut rng = Rng::new(4);
        for k in 0..300 {
            let origin = vec3(
                rng.unit() as f32 * 1000.0,
                20.0 + rng.unit() as f32 * 200.0,
                rng.unit() as f32 * 1000.0,
            );
            let dir = vec3(
                rng.unit() as f32 - 0.5,
                -0.4 - rng.unit() as f32,
                rng.unit() as f32 - 0.5,
            )
            .normalized();
            let ray = Ray { origin, dir };
            let keep = |i: u32| !(i + k).is_multiple_of(7);
            let got = bvh.raycast(&ray, &b, f32::MAX, &keep);
            let want = brute(&ray, &b, &keep);
            match (got, want) {
                (Some((_, t1)), Some((_, t2))) => assert!((t1 - t2).abs() < 1e-3, "{t1} {t2}"),
                (None, None) => {}
                other => panic!("{other:?}"),
            }
        }
        // containment
        let c = b[17].center();
        assert!(bvh.contains(c, &b, &|_| true));
        assert!(!bvh.contains(vec3(-50.0, 1.0, -50.0), &b, &|_| true));
    }

    #[test]
    fn empty_and_tiny() {
        let bvh = Bvh::build(&[]);
        let ray = Ray {
            origin: Vec3::ZERO,
            dir: vec3(1.0, 0.0, 0.0),
        };
        assert!(bvh.raycast(&ray, &[], 10.0, &|_| true).is_none());
        let one = [Aabb {
            min: vec3(1.0, -1.0, -1.0),
            max: vec3(2.0, 1.0, 1.0),
        }];
        let bvh = Bvh::build(&one);
        assert_eq!(bvh.raycast(&ray, &one, 10.0, &|_| true), Some((0, 1.0)));
        assert!(bvh.raycast(&ray, &one, 10.0, &|_| false).is_none());
    }
}
