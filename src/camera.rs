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
    /// Metres a second in free flight.
    pub speed: f32,
    pub sensitivity: f32,
    pub invert_y: bool,
    pub flight: Option<Flight>,
    /// The warp's time left (seconds; 0: none).
    pub warp: f32,
}

pub const WARP_SECONDS: f32 = 0.6;
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

fn wrap_angle(a: f32) -> f32 {
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
        let kick = if self.warp > 0.0 {
            let t = 1.0 - self.warp / WARP_SECONDS;
            (std::f32::consts::PI * t).sin() * 0.25
        } else {
            0.0
        };
        Mat4::perspective_reversed_infinite(self.fov_y + kick, aspect.max(0.01), 0.1)
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

    /// Switches between free flight and orbit, keeping the view.
    pub fn set_mode(&mut self, mode: Mode) {
        if mode == self.mode {
            return;
        }
        if mode == Mode::Orbit {
            // round the point on the ground ahead (or 120 m ahead)
            let f = self.forward();
            let d = if f.y < -0.05 {
                (self.pos.y / -f.y).min(2000.0)
            } else {
                120.0
            };
            self.distance = d.max(5.0);
            self.target = self.pos + f * self.distance;
        }
        self.mode = mode;
    }

    fn sync_orbit(&mut self) {
        self.pos = self.target - self.forward() * self.distance;
        if self.pos.y < 0.5 {
            // never below the ground: the point is looked at from above
            self.pos.y = 0.5;
        }
    }

    /// One frame of input; `solid` says whether a point is inside a
    /// building (free flight slides along walls instead of entering).
    pub fn update(&mut self, dt: f32, input: &Input, solid: &dyn Fn(Vec3) -> bool) {
        let dt = dt.clamp(0.0, 0.1);
        self.warp = (self.warp - dt).max(0.0);
        if let Some(f) = self.flight.as_mut() {
            // any steering ends a flight
            let steer = input.move_dir.iter().any(|v| *v != 0.0)
                || input.look != [0.0, 0.0]
                || input.orbit != [0.0, 0.0]
                || input.pan != [0.0, 0.0];
            if !steer {
                f.t = (f.t + dt / f.seconds).min(1.0);
                self.pos = f.path.at(f.t);
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
                let speed = self.speed * if input.boost { 5.0 } else { 1.0 };
                let step = v * (speed * dt);
                self.pos = slide(self.pos, step, solid);
                self.pos.y = self.pos.y.max(0.6);
            }
            Mode::Orbit => {
                self.yaw = wrap_angle(self.yaw + input.orbit[0] * sens);
                self.pitch = (self.pitch - input.orbit[1] * sens * inv).clamp(-PITCH_LIMIT, 0.2);
                if input.scroll != 0.0 {
                    self.distance =
                        (self.distance * 0.88f32.powf(input.scroll)).clamp(2.0, 50_000.0);
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
            }
        }
    }

    /// Flies to look at a box (centre `c`, size `size`), along a path above
    /// the roofs (`ceiling(a, b)`: the highest roof under the line a-b).
    pub fn fly_to(&mut self, c: Vec3, size: f32, ceiling: &dyn Fn(Vec3, Vec3) -> f32, slow: bool) {
        let dist = (size * 1.4).max(10.0);
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
        self.pos = pos;
        self.yaw = wrap_angle(yaw);
        self.pitch = pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT);
        if self.mode == Mode::Orbit {
            self.distance = self.distance.clamp(10.0, 400.0);
            self.target = self.pos + self.forward() * self.distance;
        }
        if warp {
            self.warp = WARP_SECONDS;
        }
    }

    pub fn flying(&self) -> bool {
        self.flight.is_some()
    }
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
    fn the_teleport_warps_and_ends() {
        let mut cam = Camera::default();
        cam.teleport(vec3(10.0, 20.0, 30.0), 1.0, -0.3, true);
        assert_eq!(cam.pos, vec3(10.0, 20.0, 30.0));
        assert!(cam.warp > 0.0);
        for _ in 0..60 {
            cam.update(1.0 / 60.0, &Input::default(), &|_| false);
        }
        assert_eq!(cam.warp, 0.0);
    }
}
