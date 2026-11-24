//! The camera: free flight (WASD and QE, the mouse to look, Shift faster,
//! the wheel sets the speed), orbit and map (round a point: drag to turn,
//! pan, wheel to zoom), flights to a place along a path that keeps above
//! every roof, and the teleport (at once, with a short warp).

use crate::math::{Mat4, Vec3, smoothstep, vec3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Fly,
    Orbit,
}

/// What the keys and the mouse ask of the camera this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Input {
    /// Right, up, forward (-1..1 each).
    pub move_dir: [f32; 3],
    pub boost: bool,
    /// The mouse's motion while looking (points).
    pub look: [f32; 2],
    /// Orbit: turn (drag), pan (drag with the middle button or Shift).
    pub orbit: [f32; 2],
    pub pan: [f32; 2],
    /// Wheel steps (+ up).
    pub scroll: f32,
}

#[derive(Clone, Debug)]
pub struct Flight {
    path: Path,
    from: (f32, f32),
    to: (f32, f32),
    t: f32,
    seconds: f32,
    /// The orbit point at the end (orbit mode).
    target: Option<Vec3>,
}

#[derive(Clone, Debug)]
pub struct Camera {
    pub mode: Mode,
    pub pos: Vec3,
    /// Around y; 0 looks along -z.
    pub yaw: f32,
    /// Up and down, within ±89°.
    pub pitch: f32,
    pub target: Vec3,
    pub distance: f32,
    pub fov_y: f32,
    /// The wheel's speed in free flight: 25 is 1x of [`Camera::flight_speed`].
    pub speed: f32,
    /// The free flight's velocity (eased: it speeds up and slows down
    /// over about 0.15 s, never stops dead).
    pub vel: Vec3,
    /// The world's width (m): how fast a flight may get, how far orbit
    /// may zoom out.
    pub world: f32,
    pub sensitivity: f32,
    pub invert_y: bool,
    pub flight: Option<Flight>,
    /// The warp's time left (seconds; 0: none).
    pub warp: f32,
}

pub const WARP_SECONDS: f32 = 0.6;
/// How fast free flight takes up a new velocity, and lets go of one (s).
pub const EASE_IN: f32 = 0.12;
pub const EASE_OUT: f32 = 0.2;
/// The orbit's farthest zoom: this many world widths (at least 2 km).
pub const ORBIT_FAR: f32 = 3.0;
const PITCH_LIMIT: f32 = 1.55;

impl Default for Camera {
    fn default() -> Camera {
        Camera {
            mode: Mode::Orbit,
            pos: vec3(0.0, 120.0, 260.0),
            yaw: 0.0,
            pitch: -0.45,
            target: Vec3::ZERO,
            distance: 300.0,
            fov_y: 60f32.to_radians(),
            speed: 25.0,
            vel: Vec3::ZERO,
            world: 1000.0,
            sensitivity: 1.0,
            invert_y: false,
            flight: None,
            warp: 0.0,
        }
    }
}

pub fn forward_of(yaw: f32, pitch: f32) -> Vec3 {
    vec3(
        yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    )
}

/// The yaw and pitch that look from `from` at `to`.
pub fn look_angles(from: Vec3, to: Vec3) -> (f32, f32) {
    let d = to - from;
    let flat = (d.x * d.x + d.z * d.z).sqrt();
    (
        d.x.atan2(-d.z),
        d.y.atan2(flat.max(1e-6)).clamp(-PITCH_LIMIT, PITCH_LIMIT),
    )
}

pub fn wrap_angle(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    (a + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI
}

impl Camera {
    pub fn forward(&self) -> Vec3 {
        forward_of(self.yaw, self.pitch)
    }
    pub fn right(&self) -> Vec3 {
        self.forward().cross(Vec3::Y).normalized()
    }

    pub fn view(&self) -> Mat4 {
        Mat4::look_to(self.pos, self.forward(), Vec3::Y)
    }

    pub fn proj(&self, aspect: f32) -> Mat4 {
        // (the warp widens the view a little)
        let kick = self.warp_amount() * 0.25;
        Mat4::perspective_reversed_infinite(self.fov_y + kick, aspect.max(0.01), 0.1)
    }

    /// How strong the warp is now (0..1): strongest at the cut, then
    /// easing away (gone after [`WARP_SECONDS`]).
    pub fn warp_amount(&self) -> f32 {
        if self.warp <= 0.0 {
            return 0.0;
        }
        smoothstep((self.warp / WARP_SECONDS).clamp(0.0, 1.0))
    }

    /// A ray through a point of the view (-1..1 each way, y up).
    pub fn ray(&self, ndc: [f32; 2], aspect: f32) -> crate::math::Ray {
        let f = self.forward();
        let r = self.right();
        let u = r.cross(f);
        let h = (self.fov_y * 0.5).tan();
        let dir = (f + r * (ndc[0] * h * aspect) + u * (ndc[1] * h)).normalized();
        crate::math::Ray {
            origin: self.pos,
            dir,
        }
    }

    /// Free flight's speed here (m/s): about 0.8 x the height over the
    /// ground (slow in a street, fast high up), at most a third of the
    /// world a second, times the wheel's factor.
    pub fn flight_speed(&self) -> f32 {
        let factor = (self.speed / 25.0).clamp(0.02, 200.0);
        let top = (self.world * 0.33).max(60.0);
        (self.pos.y.max(0.0) * 0.8).clamp(12.0, top) * factor
    }

    /// Switches between free flight and orbit, keeping the view.
    pub fn set_mode(&mut self, mode: Mode) {
        if mode == self.mode {
            return;
        }
        if mode == Mode::Orbit {
            // round the point on the ground ahead (as far as it is, up to
            // a few world widths); looking level or up: the ground 120 m
            // ahead, looked at - the pivot is never in the air
            let f = self.forward();
            if f.y < -0.05 {
                let far = (self.world * ORBIT_FAR).max(2000.0);
                self.distance = (self.pos.y / -f.y).clamp(5.0, far);
                self.target = self.pos + f * self.distance;
                self.target.y = self.target.y.max(0.0);
            } else {
                let flat = vec3(f.x, 0.0, f.z).normalized();
                self.target = vec3(self.pos.x, 0.0, self.pos.z) + flat * 120.0;
                let (yaw, pitch) = look_angles(self.pos, self.target);
                self.yaw = yaw;
                self.pitch = pitch;
                self.distance = (self.target - self.pos).length().max(5.0);
            }
        }
        self.vel = Vec3::ZERO;
        self.mode = mode;
    }

    fn sync_orbit(&mut self) {
        self.pos = self.target - self.forward() * self.distance;
        if self.pos.y < 0.5 {
            // never below the ground: the point is looked at from above
            self.pos.y = 0.5;
        }
    }

    /// Out of a building, if the eye is in one (a zoom into a roof, a
    /// switch of mode, a layout grown round it): straight up to the roof
    /// and a metre over it. Whether it had to move.
    pub fn free(&mut self, solid: &dyn Fn(Vec3) -> bool) -> bool {
        if !solid(self.pos) {
            return false;
        }
        let mut lifted = 0.0;
        while solid(self.pos) && lifted < 5000.0 {
            self.pos.y += 1.0;
            lifted += 1.0;
        }
        self.pos.y += 1.0;
        self.vel = Vec3::ZERO;
        if self.mode == Mode::Orbit {
            // (still looking at its point, from above the roof)
            let (yaw, pitch) = look_angles(self.pos, self.target);
            self.yaw = yaw;
            self.pitch = pitch;
            self.distance = (self.target - self.pos).length().max(2.0);
        }
        true
    }

    /// One frame of input; `solid` says whether a point is inside a
    /// building (free flight slides along walls instead of entering; an
    /// eye inside one is lifted out first, and an orbit's eye never ends
    /// inside one).
    pub fn update(&mut self, dt: f32, input: &Input, solid: &dyn Fn(Vec3) -> bool) {
        let dt = dt.clamp(0.0, 0.1);
        if self.flight.is_none() {
            self.free(solid);
        }
        self.warp = (self.warp - dt).max(0.0);
        if let Some(f) = self.flight.as_mut() {
            // any steering ends a flight
            let steer = input.move_dir.iter().any(|v| *v != 0.0)
                || input.look != [0.0, 0.0]
                || input.orbit != [0.0, 0.0]
                || input.pan != [0.0, 0.0];
            if !steer {
                f.t = (f.t + dt / f.seconds).min(1.0);
                // (eased: slow off and slow in - the last metres among the
                // towers are where the picture changes most)
                let e = f.t * f.t * f.t * (f.t * (f.t * 6.0 - 15.0) + 10.0);
                self.pos = f.path.at(e);
                let k = smoothstep((f.t * 1.4).min(1.0));
                self.yaw = f.from.0 + wrap_angle(f.to.0 - f.from.0) * k;
                self.pitch = f.from.1 + (f.to.1 - f.from.1) * k;
                if f.t >= 1.0 {
                    if let Some(t) = f.target {
                        self.target = t;
                        self.distance = (t - self.pos).length().max(2.0);
                    } else {
                        // (orbit round what it looks at: the camera stays)
                        self.target = self.pos + self.forward() * self.distance;
                    }
                    self.flight = None;
                }
                return;
            }
            self.flight = None;
        }
        let sens = 0.0035 * self.sensitivity;
        let inv = if self.invert_y { -1.0 } else { 1.0 };
        match self.mode {
            Mode::Fly => {
                self.yaw = wrap_angle(self.yaw + input.look[0] * sens);
                self.pitch =
                    (self.pitch - input.look[1] * sens * inv).clamp(-PITCH_LIMIT, PITCH_LIMIT);
                if input.scroll != 0.0 {
                    self.speed = (self.speed * 1.15f32.powf(input.scroll)).clamp(1.0, 5000.0);
                }
                let f = self.forward();
                let r = self.right();
                let m = input.move_dir;
                let mut v = r * m[0] + Vec3::Y * m[1] + f * m[2];
                if v.length() > 1.0 {
                    v = v.normalized();
                }
                let speed = self.flight_speed() * if input.boost { 5.0 } else { 1.0 };
                // eased: towards the wanted velocity, quicker to start than
                // to stop (no dead stop, no lurch)
                let want = v * speed;
                let tau = if v.length() > 0.0 { EASE_IN } else { EASE_OUT };
                self.vel += (want - self.vel) * (1.0 - (-dt / tau).exp());
                if v.length() == 0.0 && self.vel.length() < 0.02 {
                    self.vel = Vec3::ZERO;
                }
                // in steps of at most 2 m: fast flight never jumps through
                // a wall or the ground
                let step = self.vel * dt;
                let n = ((step.length() / 2.0).ceil() as usize).clamp(1, 256);
                for _ in 0..n {
                    let before = self.pos;
                    self.pos = slide(self.pos, step * (1.0 / n as f32), solid);
                    self.pos.y = self.pos.y.max(0.6);
                    if self.pos == before {
                        // (stopped by a wall: its speed is lost)
                        self.vel = Vec3::ZERO;
                        break;
                    }
                }
            }
            Mode::Orbit => {
                self.yaw = wrap_angle(self.yaw + input.orbit[0] * sens);
                self.pitch = (self.pitch - input.orbit[1] * sens * inv).clamp(-PITCH_LIMIT, 0.2);
                if input.scroll != 0.0 {
                    let far = (self.world * ORBIT_FAR).max(2000.0);
                    self.distance = (self.distance * 0.88f32.powf(input.scroll)).clamp(2.0, far);
                }
                if input.pan != [0.0, 0.0] {
                    let r = self.right();
                    let flat = vec3(self.forward().x, 0.0, self.forward().z).normalized();
                    let k = self.distance * 0.0016;
                    self.target += r * (-input.pan[0] * k) + flat * (input.pan[1] * k);
                }
                // keys move the point too
                let m = input.move_dir;
                if m != [0.0, 0.0, 0.0] {
                    let r = self.right();
                    let flat = vec3(self.forward().x, 0.0, self.forward().z).normalized();
                    let speed = self.distance * 0.8 * if input.boost { 4.0 } else { 1.0 };
                    self.target += (r * m[0] + flat * m[2] + Vec3::Y * m[1]) * (speed * dt);
                    self.target.y = self.target.y.max(0.0);
                }
                self.sync_orbit();
                self.free(solid);
            }
        }
    }

    /// Flies to look at a box (centre `c`, size `size`), along a path above
    /// the roofs (`ceiling(a, b)`: the highest roof under the line a-b).
    pub fn fly_to(&mut self, c: Vec3, size: f32, ceiling: &dyn Fn(Vec3, Vec3) -> f32, slow: bool) {
        // (never closer than about 60 m: a flight that ends down among the
        // towers sweeps the picture hard in its last second)
        let dist = (size * 1.4).max(60.0);
        let mut away = vec3(self.pos.x - c.x, 0.0, self.pos.z - c.z);
        if away.length() < 1e-3 {
            away = vec3(0.3, 0.0, 1.0);
        }
        let away = away.normalized();
        let mut end = c + away * (dist * 0.75) + Vec3::Y * (dist * 0.7);
        // (never inside a roof at the end)
        end.y = end.y.max(ceiling(end, end) + 3.0);
        let to = look_angles(end, c);
        self.fly_path(end, to, ceiling, slow, Some(c));
    }

    /// Flies to `end`, looking `to` (yaw, pitch).
    pub fn fly_path(
        &mut self,
        end: Vec3,
        to: (f32, f32),
        ceiling: &dyn Fn(Vec3, Vec3) -> f32,
        slow: bool,
        target: Option<Vec3>,
    ) {
        let start = self.pos;
        self.vel = Vec3::ZERO;
        let path = safe_path(start, end, ceiling);
        let length = (end - start).length();
        let mut seconds = (0.8 + length.max(1.0).log10() * 0.45).clamp(0.8, 3.0);
        if slow {
            seconds *= 1.6;
        }
        self.flight = Some(Flight {
            path,
            from: (self.yaw, self.pitch),
            to,
            t: 0.0,
            seconds,
            target,
        });
    }

    /// Jumps at once (the teleport), with the warp.
    pub fn teleport(&mut self, pos: Vec3, yaw: f32, pitch: f32, warp: bool) {
        self.flight = None;
        self.vel = Vec3::ZERO;
        self.pos = pos;
        self.yaw = wrap_angle(yaw);
        self.pitch = pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT);
        if self.mode == Mode::Orbit {
            // (round the point on the ground it looks at, when it looks
            // down: an island framed is turned round its middle)
            let f = self.forward();
            self.distance = if f.y < -0.05 {
                (self.pos.y / -f.y).clamp(2.0, 50_000.0)
            } else {
                self.distance.clamp(10.0, 400.0)
            };
            self.target = self.pos + f * self.distance;
        }
        if warp {
            self.warp = WARP_SECONDS;
        }
    }

    pub fn flying(&self) -> bool {
        self.flight.is_some()
    }
}

/// The low view's angle: 15 degrees below the horizon (the horizon in the
/// upper third, the sky and its rain over the city).
pub const LOW_PITCH: f32 = 15.0 * std::f32::consts::PI / 180.0;

/// Where a point lands in a view from `eye` along -z, `pitch` radians
/// below the horizon (x right, y up; -1..1 is in the view), or None
/// behind the eye.
pub fn project_low(eye: Vec3, p: Vec3, pitch: f32, fov_y: f32, aspect: f32) -> Option<[f32; 2]> {
    let (s, c) = pitch.sin_cos();
    let f = vec3(0.0, -s, -c);
    let up = vec3(0.0, c, -s);
    let rel = p - eye;
    let depth = rel.dot(f);
    if depth < 0.1 {
        return None;
    }
    let h = (fov_y * 0.5).tan();
    Some([rel.x / depth / (h * aspect), rel.dot(up) / depth / h])
}

/// Where to stand to see the box `min..max` whole, looking along -z at
/// `pitch` below the horizon straight at its middle (on the ground): as
/// near as can be with every corner inside the view (a margin left). The
/// eye; the point it looks at is the box's middle - orbit turns round it.
pub fn frame_box(min: Vec3, max: Vec3, pitch: f32, fov_y: f32, aspect: f32) -> Vec3 {
    let side = (max.x - min.x).max(max.z - min.z).max(1.0);
    let centre = vec3((min.x + max.x) * 0.5, 0.0, (min.z + max.z) * 0.5);
    let back = vec3(0.0, pitch.sin(), pitch.cos());
    let mut corners = Vec::with_capacity(8);
    for x in [min.x, max.x] {
        for y in [min.y.min(0.0), max.y] {
            for z in [min.z, max.z] {
                corners.push(vec3(x, y, z));
            }
        }
    }
    let margin = 0.94;
    let fits = |eye: Vec3| {
        corners.iter().all(|p| {
            project_low(eye, *p, pitch, fov_y, aspect)
                .is_some_and(|q| q[0].abs() <= margin && q[1].abs() <= margin)
        })
    };
    // (the nearest distance that fits: in steps of 1 %, from a third of
    // the box's side out)
    let mut d = side * 0.3;
    for _ in 0..800 {
        let eye = centre + back * d;
        if fits(eye) && eye.y > max.y * 0.3 + 2.0 {
            return eye;
        }
        d *= 1.01;
    }
    centre + back * d
}

/// Moves `from` by `step`, sliding along what is solid (axis by axis).
fn slide(from: Vec3, step: Vec3, solid: &dyn Fn(Vec3) -> bool) -> Vec3 {
    let to = from + step;
    if !solid(to) {
        return to;
    }
    let mut p = from;
    for axis in [
        vec3(step.x, 0.0, 0.0),
        vec3(0.0, step.y, 0.0),
        vec3(0.0, 0.0, step.z),
    ] {
        if !solid(p + axis) {
            p += axis;
        }
    }
    p
}

/// A path from `a` to `b` that keeps above the roofs: up from `a` to a
/// cruising height over the highest roof on the way (with room), across,
/// down to `b`. The climb and the descent overlap the crossing a little
/// (a smoother flight) unless that would graze a roof - then strictly up,
/// across, down.
#[derive(Clone, Copy, Debug)]
pub struct Path {
    pub a: Vec3,
    pub b: Vec3,
    pub cruise: f32,
    pub strict: bool,
}

impl Path {
    pub fn at(&self, s: f32) -> Vec3 {
        let s = s.clamp(0.0, 1.0);
        let (rise, across0, across1, fall) = if self.strict {
            (0.3, 0.3, 0.7, 0.7)
        } else {
            (0.35, 0.2, 0.8, 0.65)
        };
        let h = smoothstep((s - across0) / (across1 - across0));
        let y = if s < rise {
            self.a.y + (self.cruise - self.a.y) * smoothstep(s / rise)
        } else if s > fall {
            self.cruise + (self.b.y - self.cruise) * smoothstep((s - fall) / (1.0 - fall))
        } else {
            self.cruise
        };
        vec3(
            self.a.x + (self.b.x - self.a.x) * h,
            y,
            self.a.z + (self.b.z - self.a.z) * h,
        )
    }
}

pub fn safe_path(a: Vec3, b: Vec3, ceiling: &dyn Fn(Vec3, Vec3) -> f32) -> Path {
    let roof = ceiling(a, b);
    let span = vec3(b.x - a.x, 0.0, b.z - a.z).length();
    let cruise = (roof + 6.0 + span * 0.05).max(a.y.max(b.y));
    let smooth = Path {
        a,
        b,
        cruise,
        strict: false,
    };
    // every piece of the smooth path checked against the roofs below it
    let clear = (0..48).all(|i| {
        let p = smooth.at(i as f32 / 48.0);
        let q = smooth.at((i + 1) as f32 / 48.0);
        let moving = (vec3(q.x - p.x, 0.0, q.z - p.z)).length() > 1e-3;
        !moving || p.y.min(q.y) >= ceiling(p, q) + 0.5
    });
    Path {
        strict: !clear,
        ..smooth
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn look_angles_round_trip() {
        let from = vec3(1.0, 2.0, 3.0);
        let to = vec3(-4.0, -1.0, 10.0);
        let (yaw, pitch) = look_angles(from, to);
        let f = forward_of(yaw, pitch);
        let want = (to - from).normalized();
        assert!((f - want).length() < 1e-4);
    }

    #[test]
    fn a_flight_clears_a_tall_tower_on_the_way() {
        let tower = 300.0;
        let ceiling = |_a: Vec3, _b: Vec3| tower;
        let a = vec3(0.0, 20.0, 0.0);
        let b = vec3(1000.0, 30.0, 0.0);
        let p = safe_path(a, b, &ceiling);
        // wherever it moves across, it is above the tower
        for i in 0..200 {
            let (x, y) = (p.at(i as f32 / 200.0), p.at((i + 1) as f32 / 200.0));
            if (vec3(y.x - x.x, 0.0, y.z - x.z)).length() > 1e-3 {
                assert!(x.y.min(y.y) >= tower, "at {i}: {}", x.y);
            }
        }
        assert_eq!(p.at(0.0), a);
        assert!((p.at(1.0) - b).length() < 1e-3);
        let mut cam = Camera {
            pos: a,
            ..Default::default()
        };
        cam.fly_path(b, (0.0, 0.0), &ceiling, false, None);
        let none = |_: Vec3| false;
        for _ in 0..400 {
            cam.update(1.0 / 60.0, &Input::default(), &none);
        }
        assert!(!cam.flying());
        assert!((cam.pos - b).length() < 1e-3);
    }

    #[test]
    fn steering_ends_a_flight() {
        let mut cam = Camera::default();
        cam.fly_to(vec3(500.0, 0.0, 0.0), 20.0, &|_, _| 0.0, false);
        assert!(cam.flying());
        let input = Input {
            move_dir: [0.0, 0.0, 1.0],
            ..Default::default()
        };
        cam.update(0.016, &input, &|_| false);
        assert!(!cam.flying());
    }

    #[test]
    fn free_flight_slides_along_walls_and_keeps_above_ground() {
        let mut cam = Camera {
            mode: Mode::Fly,
            pos: vec3(0.0, 5.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            ..Default::default()
        };
        // a wall at z < -2
        let solid = |p: Vec3| p.z < -2.0;
        let input = Input {
            move_dir: [0.5, -1.0, 1.0],
            ..Default::default()
        };
        for _ in 0..200 {
            cam.update(0.05, &input, &solid);
        }
        assert!(cam.pos.z >= -2.0);
        assert!(cam.pos.x > 1.0, "slid sideways");
        assert!(cam.pos.y >= 0.6);
    }

    #[test]
    fn orbit_stays_above_ground_and_zooms() {
        let mut cam = Camera::default();
        cam.set_mode(Mode::Orbit);
        let d0 = cam.distance;
        cam.update(
            0.016,
            &Input {
                scroll: 3.0,
                ..Default::default()
            },
            &|_| false,
        );
        assert!(cam.distance < d0);
        cam.update(
            0.016,
            &Input {
                orbit: [0.0, -10_000.0],
                ..Default::default()
            },
            &|_| false,
        );
        assert!(cam.pos.y >= 0.5);
    }

    #[test]
    fn flight_speed_grows_with_height_and_the_world() {
        let mut cam = Camera {
            mode: Mode::Fly,
            world: 2500.0,
            ..Default::default()
        };
        cam.pos.y = 5.0;
        let street = cam.flight_speed();
        cam.pos.y = 400.0;
        let high = cam.flight_speed();
        assert!((10.0..=20.0).contains(&street), "{street}");
        assert!(high > street * 10.0, "{street} {high}");
        // never more than a third of the world a second (wheel at 1x)
        cam.pos.y = 1e6;
        assert!(cam.flight_speed() <= 2500.0 * 0.34);
        // a bigger world: faster at the same height
        let mut big = cam.clone();
        big.world = 25_000.0;
        assert!(big.flight_speed() > cam.flight_speed());
        // the wheel is a factor
        cam.pos.y = 400.0;
        let before = cam.flight_speed();
        cam.update(
            0.016,
            &Input {
                scroll: 5.0,
                ..Default::default()
            },
            &|_| false,
        );
        assert!(cam.flight_speed() > before * 1.9);
    }

    #[test]
    fn flight_eases_in_and_out_never_a_dead_stop() {
        let mut cam = Camera {
            mode: Mode::Fly,
            pos: vec3(0.0, 50.0, 0.0),
            world: 2500.0,
            ..Default::default()
        };
        let go = Input {
            move_dir: [0.0, 0.0, 1.0],
            ..Default::default()
        };
        let dt = 1.0 / 60.0;
        cam.update(dt, &go, &|_| false);
        let first = cam.vel.length();
        assert!(
            first > 0.0 && first < cam.flight_speed() * 0.3,
            "eased in: {first}"
        );
        for _ in 0..18 {
            cam.update(dt, &go, &|_| false);
        }
        assert!(
            cam.vel.length() > cam.flight_speed() * 0.85,
            "up to speed in 0.3 s"
        );
        // let go: it glides on a little, then stops
        let at = cam.pos;
        cam.update(dt, &Input::default(), &|_| false);
        assert!((cam.pos - at).length() > 0.1, "no dead stop");
        for _ in 0..120 {
            cam.update(dt, &Input::default(), &|_| false);
        }
        assert_eq!(cam.vel, Vec3::ZERO, "stopped after two seconds");
        let rest = cam.pos;
        cam.update(dt, &Input::default(), &|_| false);
        assert_eq!(cam.pos, rest);
    }

    #[test]
    fn fast_flight_never_passes_through_a_wall_or_the_ground() {
        let mut cam = Camera {
            mode: Mode::Fly,
            pos: vec3(0.0, 30.0, 0.0),
            world: 100_000.0,
            speed: 5000.0,
            ..Default::default()
        };
        // a 3 m thick wall 100 m ahead
        let wall = |p: Vec3| p.z < -100.0 && p.z > -103.0;
        let go = Input {
            move_dir: [0.0, 0.0, 1.0],
            boost: true,
            ..Default::default()
        };
        for _ in 0..60 {
            cam.update(0.1, &go, &wall);
        }
        assert!(cam.pos.z >= -100.0, "through the wall: {:?}", cam.pos);
        // straight down at full speed: stays over the ground
        cam.pitch = -1.5;
        for _ in 0..60 {
            cam.update(0.1, &go, &|_| false);
        }
        assert!(cam.pos.y >= 0.6, "{:?}", cam.pos);
    }

    #[test]
    fn orbit_zooms_out_to_a_few_worlds_not_further() {
        let mut cam = Camera {
            world: 2500.0,
            ..Default::default()
        };
        for _ in 0..200 {
            cam.update(
                0.016,
                &Input {
                    scroll: -5.0,
                    ..Default::default()
                },
                &|_| false,
            );
        }
        assert!(
            (cam.distance - 2500.0 * ORBIT_FAR).abs() < 1.0,
            "{}",
            cam.distance
        );
    }

    #[test]
    fn the_low_view_frames_the_whole_island_with_the_sky_above() {
        for (side, top, aspect) in [
            (800.0, 40.0, 1.6),
            (2500.0, 90.0, 1.6),
            (9000.0, 200.0, 1.6),
            (2500.0, 90.0, 0.6),
            (2500.0, 90.0, 2.4),
            (160.0, 0.5, 1.6),
        ] {
            let min = vec3(100.0, 0.0, -50.0);
            let max = vec3(100.0 + side, top, -50.0 + side);
            let fov = 60f32.to_radians();
            let eye = frame_box(min, max, LOW_PITCH, fov, aspect);
            // looking at the middle: it is in the view's middle
            let mid = vec3(100.0 + side * 0.5, 0.0, -50.0 + side * 0.5);
            let q = project_low(eye, mid, LOW_PITCH, fov, aspect).unwrap();
            assert!(q[0].abs() < 1e-3 && q[1].abs() < 1e-3, "{q:?}");
            let mut lo = [f32::MAX; 2];
            let mut hi = [f32::MIN; 2];
            for x in [min.x, max.x] {
                for y in [0.0, top] {
                    for z in [min.z, max.z] {
                        let q = project_low(eye, vec3(x, y, z), LOW_PITCH, fov, aspect)
                            .expect("in front");
                        assert!(
                            q[0].abs() <= 0.95 && q[1].abs() <= 0.95,
                            "{side} {aspect}: {q:?}"
                        );
                        lo = [lo[0].min(q[0]), lo[1].min(q[1])];
                        hi = [hi[0].max(q[0]), hi[1].max(q[1])];
                    }
                }
            }
            // and large: it fills the view's width or height
            assert!(
                hi[0] - lo[0] > 1.7 || hi[1] - lo[1] > 1.7,
                "{side} {aspect}: small {lo:?} {hi:?}"
            );
            // the horizon in the upper third
            let horizon = LOW_PITCH.tan() / (fov * 0.5).tan();
            assert!((1.0 - horizon) / 2.0 < 0.34);
        }
    }

    #[test]
    fn the_eye_is_never_shut_in_a_building() {
        // a tower 0..10 x 0..60 x 0..10
        let tower = |p: Vec3| p.x > 0.0 && p.x < 10.0 && p.z > 0.0 && p.z < 10.0 && p.y < 60.0;
        // free flight from inside it: lifted out, and the keys move it
        let mut cam = Camera {
            mode: Mode::Fly,
            pos: vec3(5.0, 20.0, 5.0),
            world: 2500.0,
            ..Default::default()
        };
        let go = Input {
            move_dir: [0.0, 0.0, 1.0],
            ..Default::default()
        };
        let at = cam.pos;
        for _ in 0..30 {
            cam.update(1.0 / 30.0, &go, &tower);
        }
        assert!(!tower(cam.pos), "{:?}", cam.pos);
        assert!((cam.pos - at).length() > 5.0, "it moved: {:?}", cam.pos);
        // orbit: zoomed into the roof, the eye stays out of it
        let mut cam = Camera {
            mode: Mode::Orbit,
            target: vec3(5.0, 0.0, 5.0),
            distance: 300.0,
            pitch: -0.3,
            world: 2500.0,
            ..Default::default()
        };
        for _ in 0..200 {
            cam.update(
                1.0 / 30.0,
                &Input {
                    scroll: 3.0,
                    ..Default::default()
                },
                &tower,
            );
            assert!(!tower(cam.pos), "{:?}", cam.pos);
        }
        // and switched to free flight there: the keys move it (sideways:
        // ahead is the roof it looks down at)
        cam.set_mode(Mode::Fly);
        let at = cam.pos;
        let side = Input {
            move_dir: [1.0, 0.0, 0.0],
            ..Default::default()
        };
        for _ in 0..30 {
            cam.update(1.0 / 30.0, &side, &tower);
        }
        assert!((cam.pos - at).length() > 1.0);
    }

    #[test]
    fn the_orbit_s_pivot_is_on_the_ground() {
        // far up, looking down at 15 degrees: the ground point, however far
        let mut cam = Camera {
            mode: Mode::Fly,
            pos: vec3(0.0, 2000.0, 0.0),
            pitch: -LOW_PITCH,
            world: 9000.0,
            ..Default::default()
        };
        cam.set_mode(Mode::Orbit);
        assert!(cam.target.y.abs() < 1.0, "{:?}", cam.target);
        assert!(cam.distance > 7000.0);
        // looking level: the ground ahead, looked at
        let mut cam = Camera {
            mode: Mode::Fly,
            pos: vec3(0.0, 50.0, 0.0),
            pitch: 0.1,
            ..Default::default()
        };
        cam.set_mode(Mode::Orbit);
        assert_eq!(cam.target.y, 0.0);
        let f = cam.forward();
        let to = (cam.target - cam.pos).normalized();
        assert!(f.dot(to) > 0.999, "it looks at its pivot");
    }

    #[test]
    fn the_teleport_warps_and_ends() {
        let mut cam = Camera::default();
        cam.teleport(vec3(10.0, 20.0, 30.0), 1.0, -0.3, true);
        assert_eq!(cam.pos, vec3(10.0, 20.0, 30.0));
        assert!(cam.warp > 0.0);
        // strongest at the cut, then only weaker
        assert_eq!(cam.warp_amount(), 1.0);
        let mut last = 1.0;
        for _ in 0..12 {
            cam.update(1.0 / 60.0, &Input::default(), &|_| false);
            let w = cam.warp_amount();
            assert!(w < last, "{w} after {last}");
            last = w;
        }
        for _ in 0..60 {
            cam.update(1.0 / 60.0, &Input::default(), &|_| false);
        }
        assert_eq!(cam.warp, 0.0);
    }
}
