//! An independent flash meter (from axross-amp's tests/support/m11.rs),
//! written from WCAG 2.3.1 / 2.3.2 and the Understanding docs, not from the
//! program's own guard: the city's frames are held to it.
//!
//! Per pixel: relative luminance L (sRGB linearised, 0.2126/0.7152/0.0722)
//! and the red value Rv = 320 * max(0, R-G-B) for saturated red pixels
//! (R/(R+G+B) >= 0.8, linear values; 0 otherwise).
//! A transition is a swing between two confirmed extrema (hysteresis) of
//! >= 0.1 in L with the darker end < 0.8, or >= 20 in Rv.
//! Fields: 1/3 x 1/3 of the picture (the picture taken as the whole screen:
//! 341 x 256 of 1024 x 768 is a 10 degree field), stride 1/12.
//! Readings (max transitions in any 1 s window; > 6 = more than 3 flashes):
//!  C  - concurrent area: a field transition is >= 25 % of the field's
//!       pixels completing a transition of one direction within 40 ms;
//!       field transitions must alternate in direction.
//!  P  - per-pixel strict: the largest share of a field whose pixels each
//!       made > 6 transitions in the same trailing second (fail >= 25 %).
//!  F  - field mean: transitions of the field's mean L (and mean Rv).
// (kept as axross-amp has it: its lints allowed)
#![allow(dead_code, unused_mut, unexpected_cfgs, clippy::all)]

pub fn lin(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[derive(Clone, Copy)]
struct Hyst {
    last: f32,
    run: f32,
    run_t: i32,
    min: f32,
    max: f32,
    min_t: i32,
    max_t: i32,
    dir: i8,
}

impl Hyst {
    fn new(v: f32) -> Hyst {
        Hyst {
            last: v,
            run: v,
            run_t: 0,
            min: v,
            max: v,
            min_t: 0,
            max_t: 0,
            dir: 0,
        }
    }
    /// Feed v at frame t; returns Some((dir, t_of_extremum)) when a
    /// transition (last -> run) is confirmed and counts.
    fn push(&mut self, v: f32, t: i32, th: f32, dark_rule: bool) -> Option<(i8, i32)> {
        let counts =
            |a: f32, b: f32| -> bool { (a - b).abs() >= th && (!dark_rule || a.min(b) < 0.8) };
        match self.dir {
            0 => {
                if v < self.min {
                    self.min = v;
                    self.min_t = t;
                }
                if v > self.max {
                    self.max = v;
                    self.max_t = t;
                }
                if v - self.min >= th {
                    self.dir = 1;
                    self.last = self.min;
                    self.run = v;
                    self.run_t = t;
                } else if self.max - v >= th {
                    self.dir = -1;
                    self.last = self.max;
                    self.run = v;
                    self.run_t = t;
                }
                None
            }
            1 => {
                if v >= self.run {
                    self.run = v;
                    self.run_t = t;
                    None
                } else if self.run - v >= th {
                    let r = if counts(self.last, self.run) {
                        Some((1, self.run_t))
                    } else {
                        None
                    };
                    self.last = self.run;
                    self.run = v;
                    self.run_t = t;
                    self.dir = -1;
                    r
                } else {
                    None
                }
            }
            _ => {
                if v <= self.run {
                    self.run = v;
                    self.run_t = t;
                    None
                } else if v - self.run >= th {
                    let r = if counts(self.last, self.run) {
                        Some((-1, self.run_t))
                    } else {
                        None
                    };
                    self.last = self.run;
                    self.run = v;
                    self.run_t = t;
                    self.dir = 1;
                    r
                } else {
                    None
                }
            }
        }
    }
    /// The ongoing swing at the end, if it already counts.
    fn tail(&self, th: f32, dark_rule: bool) -> Option<(i8, i32)> {
        if self.dir != 0
            && (self.run - self.last).abs() >= th
            && (!dark_rule || self.run.min(self.last) < 0.8)
        {
            Some((self.dir, self.run_t))
        } else {
            None
        }
    }
}

#[derive(Default, Clone, Copy, Debug)]
pub struct Reading {
    /// C: max field transitions in 1 s (lum, red)
    pub c: [usize; 2],
    /// P: max share of a field with > 6 per-pixel transitions in 1 s (lum, red)
    pub p: [f32; 2],
    /// F: max transitions of a field's mean in 1 s (lum, red)
    pub f: [usize; 2],
    /// frame of the worst C reading
    pub c_at: usize,
    /// field of the worst C reading
    pub c_field: (usize, usize),
}

impl Reading {
    pub fn fails(&self) -> bool {
        self.c[0] > 6 || self.c[1] > 6
    }
    pub fn strict_fails(&self) -> bool {
        self.p[0] >= 0.25 || self.p[1] >= 0.25
    }
}

pub struct M11 {
    w: usize,
    h: usize,
    fps: f32,
    t: i32,
    lum: Vec<Hyst>,
    red: Vec<Hyst>,
    /// per pixel: frames of its recent counted transitions (lum, red)
    recent: [Vec<std::collections::VecDeque<i32>>; 2],
    /// per field, per frame: pixels completing an up / down transition
    fup: [Vec<Vec<u32>>; 2],
    fdn: [Vec<Vec<u32>>; 2],
    fields: Vec<(usize, usize, usize, usize)>,
    fmean: [Vec<Hyst>; 2],
    fmean_ev: [Vec<Vec<i32>>; 2],
    pub p: [f32; 2],
    gamma_red: bool,
}

impl M11 {
    pub fn new(w: usize, h: usize, fps: f32) -> M11 {
        Self::with(w, h, fps, false)
    }
    pub fn with(w: usize, h: usize, fps: f32, gamma_red: bool) -> M11 {
        Self::with_field(w, h, fps, gamma_red, w / 3, h / 3)
    }
    /// Fields of fw x fh pixels (a third of the screen the picture is on).
    pub fn with_field(w: usize, h: usize, fps: f32, gamma_red: bool, fw: usize, fh: usize) -> M11 {
        let fw = fw.clamp(1, w);
        let fh = fh.clamp(1, h);
        let sx = (w / 12).max(1);
        let sy = (h / 12).max(1);
        let mut fields = Vec::new();
        let mut y = 0;
        while y + fh <= h {
            let mut x = 0;
            while x + fw <= w {
                fields.push((x, y, x + fw, y + fh));
                x += sx;
            }
            y += sy;
        }
        let n = w * h;
        M11 {
            w,
            h,
            fps,
            t: -1,
            lum: Vec::new(),
            red: Vec::new(),
            recent: [vec![Default::default(); n], vec![Default::default(); n]],
            fup: [
                vec![Vec::new(); fields.len()],
                vec![Vec::new(); fields.len()],
            ],
            fdn: [
                vec![Vec::new(); fields.len()],
                vec![Vec::new(); fields.len()],
            ],
            fmean: [Vec::new(), Vec::new()],
            fmean_ev: [
                vec![Vec::new(); fields.len()],
                vec![Vec::new(); fields.len()],
            ],
            fields,
            p: [0.0; 2],
            gamma_red,
        }
    }

    fn values(&self, img: &[[u8; 3]]) -> (Vec<f32>, Vec<f32>) {
        let mut l = Vec::with_capacity(img.len());
        let mut r = Vec::with_capacity(img.len());
        for p in img {
            let (rl, gl, bl) = (lin(p[0]), lin(p[1]), lin(p[2]));
            l.push(0.2126 * rl + 0.7152 * gl + 0.0722 * bl);
            let (rr, gg, bb) = if self.gamma_red {
                (
                    p[0] as f32 / 255.0,
                    p[1] as f32 / 255.0,
                    p[2] as f32 / 255.0,
                )
            } else {
                (rl, gl, bl)
            };
            let s = rr + gg + bb;
            let v = if s > 0.0 && rr / s >= 0.8 {
                320.0 * (rr - gg - bb).max(0.0)
            } else {
                0.0
            };
            r.push(v);
        }
        (l, r)
    }

    #[cfg(not(standalone))]
    pub fn push_rgba(&mut self, img: &image::RgbaImage) {
        let px: Vec<[u8; 3]> = img.pixels().map(|p| [p[0], p[1], p[2]]).collect();
        self.push(&px);
    }

    pub fn push(&mut self, px: &[[u8; 3]]) {
        assert_eq!(px.len(), self.w * self.h);
        self.t += 1;
        let t = self.t;
        let (l, r) = self.values(px);
        let win = self.fps.round() as i32; // 1 s
        if t == 0 {
            self.lum = l.iter().map(|&v| Hyst::new(v)).collect();
            self.red = r.iter().map(|&v| Hyst::new(v)).collect();
        }
        for k in 0..2 {
            let vals = if k == 0 { &l } else { &r };
            let (th, dark) = if k == 0 { (0.1, true) } else { (20.0, false) };
            let mut ev = Vec::new();
            let mut hot = vec![0u32; self.w * self.h];
            for i in 0..vals.len() {
                let hy = if k == 0 {
                    &mut self.lum[i]
                } else {
                    &mut self.red[i]
                };
                if let Some((d, at)) = hy.push(vals[i], t, th, dark) {
                    ev.push((i as u32, d));
                    let _ = at;
                    self.recent[k][i].push_back(t);
                }
                let q = &mut self.recent[k][i];
                while q.front().is_some_and(|&f| f <= t - win) {
                    q.pop_front();
                }
                if q.len() > 6 {
                    hot[i] = 1;
                }
            }
            let w = self.w;
            let mut um = vec![0u32; w * self.h];
            let mut dm = vec![0u32; w * self.h];
            for &(i, d) in &ev {
                if d > 0 {
                    um[i as usize] = 1
                } else {
                    dm[i as usize] = 1
                }
            }
            let integ = |m: &[u32]| -> Vec<u32> {
                let mut ii = vec![0u32; (w + 1) * (self.h + 1)];
                for y in 0..self.h {
                    let mut row = 0;
                    for x in 0..w {
                        row += m[y * w + x];
                        ii[(y + 1) * (w + 1) + x + 1] = ii[y * (w + 1) + x + 1] + row;
                    }
                }
                ii
            };
            let (iu, id) = (integ(&um), integ(&dm));
            for (fi, &(x0, y0, x1, y1)) in self.fields.iter().enumerate() {
                let q = |ii: &[u32]| {
                    ii[y1 * (w + 1) + x1] + ii[y0 * (w + 1) + x0]
                        - ii[y0 * (w + 1) + x1]
                        - ii[y1 * (w + 1) + x0]
                };
                self.fup[k][fi].push(q(&iu));
                self.fdn[k][fi].push(q(&id));
            }
            // P: share of hot pixels per field (integral image)
            let w = self.w;
            let mut ii = vec![0u32; (w + 1) * (self.h + 1)];
            for y in 0..self.h {
                let mut row = 0;
                for x in 0..w {
                    row += hot[y * w + x];
                    ii[(y + 1) * (w + 1) + x + 1] = ii[y * (w + 1) + x + 1] + row;
                }
            }
            for &(x0, y0, x1, y1) in &self.fields {
                let s = ii[y1 * (w + 1) + x1] + ii[y0 * (w + 1) + x0]
                    - ii[y0 * (w + 1) + x1]
                    - ii[y1 * (w + 1) + x0];
                let share = s as f32 / ((x1 - x0) * (y1 - y0)) as f32;
                if share > self.p[k] {
                    self.p[k] = share;
                }
            }
            // F: field means
            let mut ii = vec![0f64; (w + 1) * (self.h + 1)];
            for y in 0..self.h {
                let mut row = 0.0;
                for x in 0..w {
                    row += vals[y * w + x] as f64;
                    ii[(y + 1) * (w + 1) + x + 1] = ii[y * (w + 1) + x + 1] + row;
                }
            }
            let means: Vec<f32> = self
                .fields
                .iter()
                .map(|&(x0, y0, x1, y1)| {
                    let s = ii[y1 * (w + 1) + x1] + ii[y0 * (w + 1) + x0]
                        - ii[y0 * (w + 1) + x1]
                        - ii[y1 * (w + 1) + x0];
                    (s / ((x1 - x0) * (y1 - y0)) as f64) as f32
                })
                .collect();
            if t == 0 {
                self.fmean[k] = means.iter().map(|&v| Hyst::new(v)).collect();
            }
            for (fi, &m) in means.iter().enumerate() {
                if let Some((_, at)) = self.fmean[k][fi].push(m, t, th, dark) {
                    self.fmean_ev[k][fi].push(at);
                }
            }
        }
    }

    fn max_in_window(times: &mut Vec<i32>, win: i32) -> (usize, i32) {
        times.sort();
        let mut best = (0, 0);
        let mut j = 0;
        for i in 0..times.len() {
            while times[i] - times[j] >= win {
                j += 1;
            }
            if i + 1 - j > best.0 {
                best = (i + 1 - j, times[i]);
            }
        }
        best
    }

    pub fn reading(&mut self) -> Reading {
        let win = self.fps.round() as i32;
        let conc = ((0.040 * self.fps).round() as i32).max(1); // frames within 40 ms
        let mut out = Reading::default();
        out.p = self.p;
        // tails of the per-pixel swings
        let n = self.w * self.h;
        for k in 0..2 {
            let (th, dark) = if k == 0 { (0.1, true) } else { (20.0, false) };
            let mut tails = Vec::new();
            for i in 0..n {
                let hy = if k == 0 { &self.lum[i] } else { &self.red[i] };
                if let Some((d, at)) = hy.tail(th, dark) {
                    tails.push((i as u32, d, at));
                }
            }
            let frames = self.fup[k][0].len();
            let mut tail_map = vec![0i8; n];
            let _ = &tails; // (an unconfirmed swing at the very end is not counted)
            let w = self.w;
            for (fi, &(x0, y0, x1, y1)) in self.fields.iter().enumerate() {
                let area = ((x1 - x0) * (y1 - y0)) as f32;
                let mut up = self.fup[k][fi].clone();
                let mut dn = self.fdn[k][fi].clone();
                for y in y0..y1 {
                    for x in x0..x1 {
                        match tail_map[y * w + x] {
                            1 => up[frames - 1] += 1,
                            -1 => dn[frames - 1] += 1,
                            _ => {}
                        }
                    }
                }
                let mut times = Vec::new();
                let mut lastdir = 0i8;
                let mut f = 0usize;
                while f < frames {
                    let lo = f.saturating_sub(conc as usize - 1);
                    let su: u32 = up[lo..=f].iter().sum();
                    let sd: u32 = dn[lo..=f].iter().sum();
                    let (bu, bd) = (su as f32 >= 0.25 * area, sd as f32 >= 0.25 * area);
                    let d = if bu && bd {
                        if lastdir == 1 { -1 } else { 1 }
                    } else if bu {
                        1
                    } else if bd {
                        -1
                    } else {
                        0
                    };
                    if d != 0 && d != lastdir {
                        times.push(f as i32);
                        lastdir = d;
                        if bu && bd {
                            f += conc as usize;
                            continue;
                        }
                    }
                    f += 1;
                }
                let (c, at) = Self::max_in_window(&mut times, win);
                if c > out.c[k] {
                    out.c[k] = c;
                    out.c_at = at as usize;
                    out.c_field = (x0, y0);
                }
                let mut ft = self.fmean_ev[k][fi].clone();
                let (fm, _) = Self::max_in_window(&mut ft, win);
                out.f[k] = out.f[k].max(fm);
            }
        }
        out
    }
}
