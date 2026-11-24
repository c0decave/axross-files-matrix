//! The photosensitivity guard, as axross-amp's (src/viz/flashguard.rs):
//! whatever the city shows - a fast flight past lit towers, the teleport's
//! warp, a search hit's pillar - may not flash more than three times in any
//! second (WCAG 2.3.1: six transitions) nor flash in saturated red (2.3.2).
//!
//! The picture as shown is measured on the GPU on a grid of cells (each
//! cell's mean relative luminance over its area and its saturated-red
//! value, read back two frames later). Each cell's luminance is followed
//! with hysteresis: a transition is a swing of 0.1 or more between two
//! extremes whose darker end is below 0.8 (or 20 in the red value). A
//! *field* - a third of the picture each way, a 10° view when the city
//! fills the screen, at a twelfth's stride - transits when a quarter of its
//! cells, net, transit in one direction within 100 ms (more turning bright
//! than dark by a quarter of the field: lit things moving across a field
//! brighten some cells as they darken others, and are no flash),
//! alternating with its last. Where a field has transited four times in
//! the last second, the guard lays a veil over its cells, with soft edges:
//! the picture there is blended towards its own slow average (a third of a
//! second), 85 % - 95 % once it reaches six, dimmed by at most 40 % (never
//! black) - and the veil lifts a second after the danger has passed,
//! slowly.

/// The grid's cells across and down (the luminance texture's size).
pub const GRID_W: usize = 48;
pub const GRID_H: usize = 27;
/// A field: a third of the picture each way, moved by a twelfth.
const FIELD_W: usize = GRID_W / 3;
const FIELD_H: usize = GRID_H / 3;
const STRIDE_W: usize = GRID_W / 12;
const STRIDE_H: usize = GRID_H / 9;
/// WCAG's thresholds.
const STEP: f32 = 0.1;
const DARK: f32 = 0.8;
const RED_STEP: f32 = 20.0;
/// What happens "at once" (seconds).
const AT_ONCE: f32 = 0.1;
/// The share of a field that must transit together.
const AREA: f32 = 0.25;
/// Field transitions in the last second from which the veil comes, and
/// from which it closes further.
const VEIL_AT: usize = 4;
const CLOSE_AT: usize = 6;

#[derive(Clone, Copy, Debug)]
struct Track {
    /// The last confirmed extreme, and the running extreme since.
    last: f32,
    run: f32,
    /// +1 rising, -1 falling, 0 not yet known.
    dir: i8,
    /// When it last transited, and which way.
    when: f32,
    which: i8,
}

impl Track {
    fn new(v: f32) -> Track {
        Track {
            last: v,
            run: v,
            dir: 0,
            when: -10.0,
            which: 0,
        }
    }

    /// Feeds a value; Some(direction) when a transition completes.
    fn push(&mut self, v: f32, step: f32, dark_rule: bool) -> Option<i8> {
        let counts = |a: f32, b: f32| (a - b).abs() >= step && (!dark_rule || a.min(b) < DARK);
        match self.dir {
            0 => {
                if counts(v, self.last) {
                    self.dir = if v > self.last { 1 } else { -1 };
                    self.run = v;
                    let d = self.dir;
                    self.last = v;
                    return Some(d);
                }
                None
            }
            d => {
                // still going the same way: a further extreme
                if (d > 0 && v > self.run) || (d < 0 && v < self.run) {
                    self.run = v;
                    return None;
                }
                // turned back far enough: the swing to the new side counts
                if counts(v, self.run) {
                    self.last = self.run;
                    self.run = v;
                    self.dir = -d;
                    return Some(-d);
                }
                None
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Field {
    /// Its transitions' times in the last second, and the last direction.
    times: Vec<f32>,
    last_dir: i8,
    /// Its mean luminance followed as a cell's is (with a quarter of the
    /// step: a quarter of its area swinging by 0.1): a flash moves it, lit
    /// things passing across hardly.
    mean: Option<Track>,
    /// When it was last dangerous.
    danger_at: f32,
    level: f32,
}

/// The guard's state.
#[derive(Clone, Debug)]
pub struct FlashGuard {
    lum: Vec<Track>,
    red: Vec<Track>,
    fields: Vec<Field>,
    /// Per cell: the veil now (0..0.95).
    pub veil: Vec<f32>,
    time: f32,
    started: bool,
    /// How many times a field reached the veil (tests, the stats).
    pub engaged: u64,
}

impl Default for FlashGuard {
    fn default() -> FlashGuard {
        FlashGuard::new()
    }
}

fn field_origins() -> impl Iterator<Item = (usize, usize)> {
    (0..=(GRID_H - FIELD_H) / STRIDE_H).flat_map(|fy| {
        (0..=(GRID_W - FIELD_W) / STRIDE_W).map(move |fx| (fx * STRIDE_W, fy * STRIDE_H))
    })
}

impl FlashGuard {
    pub fn new() -> FlashGuard {
        FlashGuard {
            lum: Vec::new(),
            red: Vec::new(),
            fields: vec![Field::default(); field_origins().count()],
            veil: vec![0.0; GRID_W * GRID_H],
            time: 0.0,
            started: false,
            engaged: 0,
        }
    }

    /// Whether a veil lies anywhere now.
    pub fn active(&self) -> bool {
        self.veil.iter().any(|v| *v > 0.01)
    }

    /// One measured frame (`lum` and `red` per cell, row by row), `dt`
    /// seconds after the last; the veil per cell.
    pub fn update(&mut self, lum: &[f32], red: &[f32], dt: f32) -> &[f32] {
        let n = GRID_W * GRID_H;
        if lum.len() < n || red.len() < n {
            return &self.veil;
        }
        self.time += dt.clamp(0.0, 0.5);
        let now = self.time;
        if !self.started {
            self.lum = lum[..n].iter().map(|&v| Track::new(v)).collect();
            self.red = red[..n].iter().map(|&v| Track::new(v)).collect();
            self.started = true;
            return &self.veil;
        }
        for i in 0..n {
            if let Some(d) = self.lum[i].push(lum[i], STEP, true) {
                self.lum[i].when = now;
                self.lum[i].which = d;
            }
            if let Some(d) = self.red[i].push(red[i], RED_STEP, false) {
                self.red[i].when = now;
                self.red[i].which = d;
            }
        }
        let mut target = vec![0.0f32; n];
        for (k, (x0, y0)) in field_origins().enumerate() {
            let mut up = [0usize; 2];
            let mut down = [0usize; 2];
            let mut sum = 0.0f32;
            for y in y0..y0 + FIELD_H {
                for x in x0..x0 + FIELD_W {
                    sum += lum[y * GRID_W + x];
                    for (j, tracks) in [&self.lum, &self.red].into_iter().enumerate() {
                        let t = &tracks[y * GRID_W + x];
                        if now - t.when <= AT_ONCE {
                            if t.which > 0 {
                                up[j] += 1;
                            } else if t.which < 0 {
                                down[j] += 1;
                            }
                        }
                    }
                }
            }
            let cells = (FIELD_W * FIELD_H) as f32;
            let f = &mut self.fields[k];
            f.times.retain(|t| now - t <= 1.0);
            let mean = sum / cells;
            let track = f.mean.get_or_insert_with(|| Track::new(mean));
            if let Some(d) = track.push(mean, STEP * AREA, true) {
                track.when = now;
                track.which = d;
            }
            let (mean_when, mean_which) = (track.when, track.which);
            for j in 0..2 {
                // (by the net share: lit things passing - motion - brighten
                // as many cells as they darken, and the field as a whole
                // does not flash; a flash turns its cells one way)
                let net = up[j] as f32 - down[j] as f32;
                let mut dir = if net >= cells * AREA {
                    1
                } else if -net >= cells * AREA {
                    -1
                } else {
                    0
                };
                // (and the field's own mean swung that way too: a quarter of
                // its area flashing by 0.1 moves it by 0.025 at least)
                if j == 0 && dir != 0 && !(mean_which == dir && now - mean_when <= AT_ONCE * 2.0) {
                    dir = 0;
                }
                if dir != 0 && dir != f.last_dir {
                    f.last_dir = dir;
                    f.times.push(now);
                    if std::env::var_os("AFM_GUARD_TRACE").is_some() {
                        eprintln!(
                            "guard {now:.2}s field {k} ({x0},{y0}) {} up {} down {} of {cells}, mean {mean:.3}, count {}",
                            if j == 0 { "lum" } else { "red" },
                            up[j],
                            down[j],
                            f.times.len()
                        );
                    }
                }
            }
            let count = f.times.len();
            if count >= VEIL_AT {
                if f.level == 0.0 {
                    self.engaged += 1;
                }
                f.danger_at = now;
                f.level = if count >= CLOSE_AT { 0.95 } else { 0.85 };
            } else if now - f.danger_at > 1.0 {
                f.level = 0.0;
            }
            if f.level > 0.0 {
                for y in y0..y0 + FIELD_H {
                    for x in x0..x0 + FIELD_W {
                        let c = &mut target[y * GRID_W + x];
                        *c = c.max(f.level);
                    }
                }
            }
        }
        // (soft edges: the veil fades out over two cells round a field,
        // and a little inside it - never a hard-edged block)
        let target = soften(&target);
        // the veil comes at once and goes over about two seconds
        for (v, t) in self.veil.iter_mut().zip(target) {
            *v = if t > *v { t } else { (*v - dt * 0.5).max(t) };
        }
        &self.veil
    }

    /// The veil as bytes (a texture's rows).
    pub fn veil_bytes(&self) -> Vec<u8> {
        self.veil
            .iter()
            .map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8)
            .collect()
    }
}

/// A veil's cells with soft edges: the larger of 80 % of a cell's own
/// level and its neighbourhood's mean (five cells each way).
fn soften(target: &[f32]) -> Vec<f32> {
    let blur = |src: &[f32], dx: usize, dy: usize| -> Vec<f32> {
        let mut out = vec![0.0f32; src.len()];
        for y in 0..GRID_H {
            for x in 0..GRID_W {
                let (mut sum, mut n) = (0.0, 0.0);
                for k in -2i32..=2 {
                    let (xx, yy) = (x as i32 + k * dx as i32, y as i32 + k * dy as i32);
                    if xx >= 0 && yy >= 0 && (xx as usize) < GRID_W && (yy as usize) < GRID_H {
                        sum += src[yy as usize * GRID_W + xx as usize];
                        n += 1.0;
                    }
                }
                out[y * GRID_W + x] = sum / n;
            }
        }
        out
    };
    let b = blur(&blur(target, 1, 0), 0, 1);
    target
        .iter()
        .zip(b)
        .map(|(t, m)| (t * 0.8).max(m))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        guard: &mut FlashGuard,
        seconds: f32,
        fps: f32,
        frame: impl Fn(f32) -> (f32, f32),
    ) -> f32 {
        let n = GRID_W * GRID_H;
        let mut max_veil = 0.0f32;
        let frames = (seconds * fps) as usize;
        for k in 0..frames {
            let t = k as f32 / fps;
            let (l, r) = frame(t);
            let veil = guard.update(&vec![l; n], &vec![r; n], 1.0 / fps);
            max_veil = max_veil.max(veil.iter().copied().fold(0.0, f32::max));
        }
        max_veil
    }

    #[test]
    fn a_ten_hertz_flicker_is_veiled_within_a_second() {
        let mut g = FlashGuard::new();
        let v = run(&mut g, 1.0, 60.0, |t| {
            (
                if ((t * 20.0) as u32).is_multiple_of(2) {
                    0.05
                } else {
                    0.6
                },
                0.0,
            )
        });
        assert!(v >= 0.85, "{v}");
        assert!(g.engaged > 0);
    }

    #[test]
    fn calm_pictures_and_slow_changes_are_left_alone() {
        let mut g = FlashGuard::new();
        assert_eq!(run(&mut g, 3.0, 60.0, |_| (0.2, 0.0)), 0.0);
        // one flash a second: two transitions a second, within the rule
        let mut g = FlashGuard::new();
        assert_eq!(
            run(&mut g, 4.0, 60.0, |t| (
                if t.fract() < 0.5 { 0.05 } else { 0.6 },
                0.0
            )),
            0.0
        );
        // small swings (under 0.1) never count
        let mut g = FlashGuard::new();
        assert_eq!(
            run(&mut g, 2.0, 60.0, |t| (
                if ((t * 20.0) as u32).is_multiple_of(2) {
                    0.10
                } else {
                    0.18
                },
                0.0
            )),
            0.0
        );
        // bright swings above 0.8 never count either
        let mut g = FlashGuard::new();
        assert_eq!(
            run(&mut g, 2.0, 60.0, |t| (
                if ((t * 20.0) as u32).is_multiple_of(2) {
                    0.82
                } else {
                    0.95
                },
                0.0
            )),
            0.0
        );
    }

    #[test]
    fn lit_things_moving_across_are_no_flash() {
        // bright bands sweeping across the picture a cell a frame: each
        // cell lit and dark in turn, but as many brighten as darken
        let mut g = FlashGuard::new();
        let n = GRID_W * GRID_H;
        for k in 0..120 {
            let lum: Vec<f32> = (0..n)
                .map(|i| {
                    let x = i % GRID_W;
                    if ((x + k) / 4).is_multiple_of(2) {
                        0.4
                    } else {
                        0.03
                    }
                })
                .collect();
            g.update(&lum, &vec![0.0; n], 1.0 / 60.0);
        }
        assert!(!g.active(), "motion veiled");
        assert_eq!(g.engaged, 0);
    }

    #[test]
    fn red_flashes_are_veiled_too() {
        let mut g = FlashGuard::new();
        let v = run(&mut g, 1.0, 60.0, |t| {
            (
                0.1,
                if ((t * 16.0) as u32).is_multiple_of(2) {
                    0.0
                } else {
                    60.0
                },
            )
        });
        assert!(v >= 0.85);
    }

    #[test]
    fn the_veil_lifts_after_the_danger() {
        let mut g = FlashGuard::new();
        run(&mut g, 1.0, 60.0, |t| {
            (
                if ((t * 20.0) as u32).is_multiple_of(2) {
                    0.05
                } else {
                    0.6
                },
                0.0,
            )
        });
        assert!(g.active());
        run(&mut g, 4.0, 60.0, |_| (0.3, 0.0));
        assert!(!g.active());
    }

    #[test]
    fn a_flicker_in_one_corner_veils_that_corner_only() {
        let mut g = FlashGuard::new();
        let n = GRID_W * GRID_H;
        for k in 0..60 {
            let mut lum = vec![0.2; n];
            let on = (k / 3) % 2 == 0;
            for y in 0..GRID_H / 3 {
                for x in 0..GRID_W / 3 {
                    lum[y * GRID_W + x] = if on { 0.05 } else { 0.7 };
                }
            }
            g.update(&lum, &vec![0.0; n], 1.0 / 60.0);
        }
        assert!(g.veil[0] > 0.6);
        assert_eq!(g.veil[n - 1], 0.0);
        // with soft edges: the veil's last cells in a row lighter than its
        // middle, none further
        let row = &g.veil[..GRID_W];
        let last = row.iter().rposition(|v| *v > 0.0).unwrap();
        assert!(last + 1 < GRID_W, "{row:?}");
        assert!(row[last] < row[0] * 0.5, "a soft edge: {row:?}");
    }

    #[test]
    fn the_grid_is_cut_evenly() {
        assert_eq!(GRID_W % 12, 0);
        assert_eq!(GRID_H % 9, 0);
        assert_eq!(field_origins().count(), 9 * 7);
        assert_eq!(FlashGuard::new().veil_bytes().len(), GRID_W * GRID_H);
    }
}
