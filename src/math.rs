//! The little linear algebra the city needs: 3-vectors, column-major 4x4
//! matrices (as WGSL reads them), rays and boxes. Right-handed, y up; the
//! ground is the x-z plane. Depth is reversed (near 1, far 0) with an
//! infinite far plane: the city spans kilometres, the camera flies a metre
//! above a roof.

use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

pub const fn vec3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3 { x, y, z }
}

impl Vec3 {
    pub const ZERO: Vec3 = vec3(0.0, 0.0, 0.0);
    pub const Y: Vec3 = vec3(0.0, 1.0, 0.0);

    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: Vec3) -> Vec3 {
        vec3(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
    pub fn normalized(self) -> Vec3 {
        let l = self.length();
        if l > 1e-12 {
            self * (1.0 / l)
        } else {
            Vec3::ZERO
        }
    }
    pub fn lerp(self, o: Vec3, t: f32) -> Vec3 {
        self + (o - self) * t
    }
    pub fn min(self, o: Vec3) -> Vec3 {
        vec3(self.x.min(o.x), self.y.min(o.y), self.z.min(o.z))
    }
    pub fn max(self, o: Vec3) -> Vec3 {
        vec3(self.x.max(o.x), self.y.max(o.y), self.z.max(o.z))
    }
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
    pub fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }
}

impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        vec3(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl AddAssign for Vec3 {
    fn add_assign(&mut self, o: Vec3) {
        *self = *self + o;
    }
}
impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        vec3(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl SubAssign for Vec3 {
    fn sub_assign(&mut self, o: Vec3) {
        *self = *self - o;
    }
}
impl Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, k: f32) -> Vec3 {
        vec3(self.x * k, self.y * k, self.z * k)
    }
}
impl Neg for Vec3 {
    type Output = Vec3;
    fn neg(self) -> Vec3 {
        vec3(-self.x, -self.y, -self.z)
    }
}

/// Column-major: `m[c][r]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4(pub [[f32; 4]; 4]);

impl Mat4 {
    pub const IDENTITY: Mat4 = Mat4([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]);

    pub fn mul(&self, o: &Mat4) -> Mat4 {
        let mut r = [[0.0f32; 4]; 4];
        for (c, col) in r.iter_mut().enumerate() {
            for (row, v) in col.iter_mut().enumerate() {
                *v = (0..4).map(|k| self.0[k][row] * o.0[c][k]).sum();
            }
        }
        Mat4(r)
    }

    pub fn transform(&self, p: [f32; 4]) -> [f32; 4] {
        let m = &self.0;
        let mut out = [0.0; 4];
        for (row, o) in out.iter_mut().enumerate() {
            *o = m[0][row] * p[0] + m[1][row] * p[1] + m[2][row] * p[2] + m[3][row] * p[3];
        }
        out
    }

    /// A view matrix: the eye at `eye` looking along `forward` (unit).
    pub fn look_to(eye: Vec3, forward: Vec3, up: Vec3) -> Mat4 {
        let f = forward.normalized();
        let mut s = f.cross(up).normalized();
        if s == Vec3::ZERO {
            s = vec3(1.0, 0.0, 0.0);
        }
        let u = s.cross(f);
        Mat4([
            [s.x, u.x, -f.x, 0.0],
            [s.y, u.y, -f.y, 0.0],
            [s.z, u.z, -f.z, 0.0],
            [-s.dot(eye), -u.dot(eye), f.dot(eye), 1.0],
        ])
    }

    /// A perspective projection with reversed depth and no far plane: the
    /// near plane maps to depth 1, infinity to 0.
    pub fn perspective_reversed_infinite(fov_y: f32, aspect: f32, near: f32) -> Mat4 {
        let f = 1.0 / (fov_y * 0.5).tan();
        Mat4([
            [f / aspect, 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, 0.0, -1.0],
            [0.0, 0.0, near, 0.0],
        ])
    }

    pub fn to_cols(&self) -> [[f32; 4]; 4] {
        self.0
    }
}

/// An axis-aligned box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb {
        min: vec3(f32::INFINITY, f32::INFINITY, f32::INFINITY),
        max: vec3(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY),
    };
    pub fn union(self, o: Aabb) -> Aabb {
        Aabb {
            min: self.min.min(o.min),
            max: self.max.max(o.max),
        }
    }
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
    pub fn contains(&self, p: Vec3) -> bool {
        p.x >= self.min.x
            && p.x <= self.max.x
            && p.y >= self.min.y
            && p.y <= self.max.y
            && p.z >= self.min.z
            && p.z <= self.max.z
    }
    pub fn grown(&self, by: f32) -> Aabb {
        let d = vec3(by, by, by);
        Aabb {
            min: self.min - d,
            max: self.max + d,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x
    }
}

/// A ray: an origin and a direction (not necessarily unit).
#[derive(Clone, Copy, Debug)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

impl Ray {
    /// The entry distance along the ray into `b` (slab test), if it hits
    /// within `0..max`.
    pub fn hit_aabb(&self, b: &Aabb, max: f32) -> Option<f32> {
        let inv = vec3(1.0 / self.dir.x, 1.0 / self.dir.y, 1.0 / self.dir.z);
        let mut t0 = 0.0f32;
        let mut t1 = max;
        for (o, d, lo, hi) in [
            (self.origin.x, inv.x, b.min.x, b.max.x),
            (self.origin.y, inv.y, b.min.y, b.max.y),
            (self.origin.z, inv.z, b.min.z, b.max.z),
        ] {
            let (mut a, mut c) = ((lo - o) * d, (hi - o) * d);
            if a > c {
                std::mem::swap(&mut a, &mut c);
            }
            // (NaN from 0 * inf: a ray in the slab's plane - inside)
            if a.is_nan() || c.is_nan() {
                if o < lo || o > hi {
                    return None;
                }
                continue;
            }
            t0 = t0.max(a);
            t1 = t1.min(c);
            if t0 > t1 {
                return None;
            }
        }
        Some(t0)
    }
    pub fn at(&self, t: f32) -> Vec3 {
        self.origin + self.dir * t
    }
}

/// A view frustum as six planes (normals inward: `n . p + d >= 0` inside).
#[derive(Clone, Copy, Debug)]
pub struct Frustum {
    planes: [[f32; 4]; 6],
}

impl Frustum {
    /// From a view-projection matrix whose depth runs 1 (near) to 0 (far,
    /// at infinity): left, right, bottom, top and near are taken; the far
    /// plane is at infinity and left out (kept as an always-true plane).
    pub fn from_view_proj(m: &Mat4) -> Frustum {
        let row = |r: usize| [m.0[0][r], m.0[1][r], m.0[2][r], m.0[3][r]];
        let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
        let add = |a: [f32; 4], b: [f32; 4]| [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]];
        let sub = |a: [f32; 4], b: [f32; 4]| [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]];
        let norm = |p: [f32; 4]| {
            let l = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt().max(1e-12);
            [p[0] / l, p[1] / l, p[2] / l, p[3] / l]
        };
        Frustum {
            planes: [
                norm(add(r3, r0)),
                norm(sub(r3, r0)),
                norm(add(r3, r1)),
                norm(sub(r3, r1)),
                // reversed depth: z_ndc <= 1 is the near plane (w - z >= 0)
                norm(sub(r3, r2)),
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    /// Whether all of the box is inside (every corner in front of every
    /// plane): what lies in it is inside too.
    pub fn contains(&self, b: &Aabb) -> bool {
        self.planes.iter().all(|p| {
            let x = if p[0] >= 0.0 { b.min.x } else { b.max.x };
            let y = if p[1] >= 0.0 { b.min.y } else { b.max.y };
            let z = if p[2] >= 0.0 { b.min.z } else { b.max.z };
            p[0] * x + p[1] * y + p[2] * z + p[3] >= 0.0
        })
    }

    /// Whether any of the box may be visible.
    pub fn intersects(&self, b: &Aabb) -> bool {
        self.planes.iter().all(|p| {
            let x = if p[0] >= 0.0 { b.max.x } else { b.min.x };
            let y = if p[1] >= 0.0 { b.max.y } else { b.min.y };
            let z = if p[2] >= 0.0 { b.max.z } else { b.min.z };
            p[0] * x + p[1] * y + p[2] * z + p[3] >= 0.0
        })
    }
}

/// Smooth 0..1 ease in and out.
pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A stable 32-bit hash of bytes (FNV-1a), for the shapes' variety.
pub fn hash32(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_hits_box_from_outside_and_inside() {
        let b = Aabb {
            min: vec3(0.0, 0.0, 0.0),
            max: vec3(1.0, 1.0, 1.0),
        };
        let r = Ray {
            origin: vec3(-1.0, 0.5, 0.5),
            dir: vec3(1.0, 0.0, 0.0),
        };
        assert_eq!(r.hit_aabb(&b, 100.0), Some(1.0));
        let inside = Ray {
            origin: vec3(0.5, 0.5, 0.5),
            dir: vec3(0.0, 1.0, 0.0),
        };
        assert_eq!(inside.hit_aabb(&b, 100.0), Some(0.0));
        let miss = Ray {
            origin: vec3(-1.0, 2.0, 0.5),
            dir: vec3(1.0, 0.0, 0.0),
        };
        assert_eq!(miss.hit_aabb(&b, 100.0), None);
    }

    #[test]
    fn projection_maps_near_to_one_and_far_towards_zero() {
        let p = Mat4::perspective_reversed_infinite(1.0, 1.0, 0.5);
        let near = p.transform([0.0, 0.0, -0.5, 1.0]);
        assert!((near[2] / near[3] - 1.0).abs() < 1e-6);
        let far = p.transform([0.0, 0.0, -1e6, 1.0]);
        assert!(far[2] / far[3] < 1e-5);
    }

    #[test]
    fn frustum_culls_what_is_behind() {
        let view = Mat4::look_to(Vec3::ZERO, vec3(0.0, 0.0, -1.0), Vec3::Y);
        let proj = Mat4::perspective_reversed_infinite(1.0, 1.0, 0.1);
        let f = Frustum::from_view_proj(&proj.mul(&view));
        let ahead = Aabb {
            min: vec3(-1.0, -1.0, -11.0),
            max: vec3(1.0, 1.0, -9.0),
        };
        let behind = Aabb {
            min: vec3(-1.0, -1.0, 9.0),
            max: vec3(1.0, 1.0, 11.0),
        };
        let far_ahead = Aabb {
            min: vec3(-1.0, -1.0, -1e5),
            max: vec3(1.0, 1.0, -1e5 + 1.0),
        };
        assert!(f.intersects(&ahead));
        assert!(!f.intersects(&behind));
        assert!(f.intersects(&far_ahead));
        // wholly inside: ahead and narrow; across an edge: not wholly
        assert!(f.contains(&ahead));
        assert!(!f.contains(&behind));
        let across = Aabb {
            min: vec3(-100.0, -1.0, -11.0),
            max: vec3(1.0, 1.0, -9.0),
        };
        assert!(f.intersects(&across) && !f.contains(&across));
    }
}
