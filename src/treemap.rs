//! The squarified treemap (Bruls, Huizing and van Wijk, 2000): a rectangle
//! cut into one piece per weight, each piece's area its share of the total,
//! the pieces kept as close to squares as the order allows. Weights come
//! sorted largest first; rows are laid along the rectangle's shorter side.

/// A rectangle on the ground: x0, z0 (one corner), x1, z1 (the other).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub z0: f32,
    pub x1: f32,
    pub z1: f32,
}

impl Rect {
    pub const fn new(x0: f32, z0: f32, x1: f32, z1: f32) -> Rect {
        Rect { x0, z0, x1, z1 }
    }
    pub fn w(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn h(&self) -> f32 {
        self.z1 - self.z0
    }
    pub fn area(&self) -> f32 {
        self.w().max(0.0) * self.h().max(0.0)
    }
    pub fn center(&self) -> (f32, f32) {
        ((self.x0 + self.x1) * 0.5, (self.z0 + self.z1) * 0.5)
    }
    /// Smaller by `m` on every side (never inside out: at most to its
    /// centre line).
    pub fn inset(&self, m: f32) -> Rect {
        let mx = m.min(self.w() * 0.5).max(0.0);
        let mz = m.min(self.h() * 0.5).max(0.0);
        Rect::new(self.x0 + mx, self.z0 + mz, self.x1 - mx, self.z1 - mz)
    }
    /// Smaller by a share `k` (0..0.5) of each side's length.
    pub fn shrunk(&self, k: f32) -> Rect {
        let (mx, mz) = (self.w() * k, self.h() * k);
        Rect::new(self.x0 + mx, self.z0 + mz, self.x1 - mx, self.z1 - mz)
    }
    pub fn aspect(&self) -> f32 {
        let (w, h) = (self.w(), self.h());
        if w <= 0.0 || h <= 0.0 {
            return f32::INFINITY;
        }
        (w / h).max(h / w)
    }
}

/// The worst aspect ratio in a row of areas `row` laid along a side of
/// length `side`.
fn worst(sum: f64, min: f64, max: f64, side: f64) -> f64 {
    if sum <= 0.0 || side <= 0.0 {
        return f64::INFINITY;
    }
    let s2 = sum * sum;
    let w2 = side * side;
    (w2 * max / s2).max(s2 / (w2 * min.max(1e-300)))
}

/// Lays `weights` (sorted largest first, none negative) into `rect`; the
/// pieces in the same order. Zero weights get empty pieces at a corner.
pub fn squarify(weights: &[f64], rect: Rect) -> Vec<Rect> {
    let n = weights.len();
    let mut out = vec![Rect::new(rect.x0, rect.z0, rect.x0, rect.z0); n];
    let total: f64 = weights.iter().copied().filter(|w| *w > 0.0).sum();
    if n == 0 || total <= 0.0 || rect.area() <= 0.0 {
        return out;
    }
    let scale = rect.area() as f64 / total;
    let (mut x0, mut z0, mut x1, mut z1) = (
        rect.x0 as f64,
        rect.z0 as f64,
        rect.x1 as f64,
        rect.z1 as f64,
    );
    let mut i = 0;
    // the weights that count (sorted: zeros are at the end)
    let end = weights.iter().position(|w| *w <= 0.0).unwrap_or(n);
    while i < end {
        let (w, h) = (x1 - x0, z1 - z0);
        let side = w.min(h);
        // grow the row while the worst aspect gets better
        let first = weights[i] * scale;
        let (mut sum, mut min, mut max) = (first, first, first);
        let mut j = i + 1;
        let mut current = worst(sum, min, max, side);
        while j < end {
            let a = weights[j] * scale;
            let next = worst(sum + a, min.min(a), max.max(a), side);
            if next > current {
                break;
            }
            sum += a;
            min = min.min(a);
            max = max.max(a);
            current = next;
            j += 1;
        }
        // the last row takes what is left (rounding)
        if j == end {
            sum = w * h;
        }
        // lay the row along the shorter side
        let thick = if side > 0.0 { sum / side } else { 0.0 };
        let row_sum: f64 = weights[i..j].iter().sum::<f64>() * scale;
        let mut at = 0.0;
        for (k, piece) in out[i..j].iter_mut().enumerate() {
            let a = weights[i + k] * scale;
            let len = if row_sum > 0.0 {
                side * a / row_sum
            } else {
                0.0
            };
            *piece = if w >= h {
                // a column at the left, pieces from top to bottom
                Rect::new(
                    x0 as f32,
                    (z0 + at) as f32,
                    (x0 + thick) as f32,
                    (z0 + at + len) as f32,
                )
            } else {
                // a row at the top, pieces from left to right
                Rect::new(
                    (x0 + at) as f32,
                    z0 as f32,
                    (x0 + at + len) as f32,
                    (z0 + thick) as f32,
                )
            };
            at += len;
        }
        if w >= h {
            x0 = (x0 + thick).min(x1);
        } else {
            z0 = (z0 + thick).min(z1);
        }
        i = j;
        if x1 - x0 <= 0.0 || z1 - z0 <= 0.0 {
            // nothing left (rounding): the rest stay at the corner
            x1 = x1.max(x0);
            z1 = z1.max(z0);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlap(a: &Rect, b: &Rect) -> f32 {
        let w = (a.x1.min(b.x1) - a.x0.max(b.x0)).max(0.0);
        let h = (a.z1.min(b.z1) - a.z0.max(b.z0)).max(0.0);
        w * h
    }

    #[test]
    fn the_classic_example() {
        // Bruls et al.'s example: 6, 6, 4, 3, 2, 2, 1 in a 6 x 4 rectangle
        let w = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let r = squarify(&w, Rect::new(0.0, 0.0, 6.0, 4.0));
        for (piece, weight) in r.iter().zip(w) {
            assert!(
                (piece.area() - weight as f32).abs() < 1e-3,
                "{piece:?} {weight}"
            );
        }
        // the first row: two 3 x 2 pieces at the left
        assert!((r[0].w() - 3.0).abs() < 1e-4 && (r[0].h() - 2.0).abs() < 1e-4);
        let worst = r.iter().map(|p| p.aspect()).fold(0.0f32, f32::max);
        assert!(worst < 3.0, "worst aspect {worst}");
    }

    #[test]
    fn areas_add_up_and_nothing_overlaps() {
        let mut w: Vec<f64> = (0..500).map(|i| ((i * 7919) % 1000 + 1) as f64).collect();
        w.sort_by(|a, b| b.partial_cmp(a).unwrap());
        let rect = Rect::new(10.0, -5.0, 110.0, 45.0);
        let r = squarify(&w, rect);
        let total: f64 = w.iter().sum();
        let mut area = 0.0f32;
        for (piece, weight) in r.iter().zip(&w) {
            let want = (*weight / total) as f32 * rect.area();
            assert!((piece.area() - want).abs() < want * 0.01 + 1e-3);
            assert!(piece.x0 >= rect.x0 - 1e-3 && piece.x1 <= rect.x1 + 1e-3);
            assert!(piece.z0 >= rect.z0 - 1e-3 && piece.z1 <= rect.z1 + 1e-3);
            area += piece.area();
        }
        assert!((area - rect.area()).abs() < rect.area() * 1e-3);
        for i in 0..r.len() {
            for j in i + 1..r.len().min(i + 40) {
                assert!(overlap(&r[i], &r[j]) < 1e-2, "{i} {j}");
            }
        }
    }

    #[test]
    fn zeros_and_degenerate_rectangles() {
        let r = squarify(&[5.0, 0.0], Rect::new(0.0, 0.0, 2.0, 2.0));
        assert!((r[0].area() - 4.0).abs() < 1e-4);
        assert_eq!(r[1].area(), 0.0);
        assert!(squarify(&[], Rect::new(0.0, 0.0, 1.0, 1.0)).is_empty());
        let flat = squarify(&[1.0, 1.0], Rect::new(0.0, 0.0, 0.0, 1.0));
        assert!(flat.iter().all(|p| p.area() == 0.0));
        let one = squarify(&[3.0], Rect::new(0.0, 0.0, 4.0, 1.0));
        assert_eq!(one[0], Rect::new(0.0, 0.0, 4.0, 1.0));
    }

    #[test]
    fn extreme_ratios_stay_inside() {
        let mut w = vec![1e12];
        w.extend(std::iter::repeat_n(1.0, 1000));
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let r = squarify(&w, rect);
        for p in &r {
            assert!(p.x0.is_finite() && p.z1.is_finite());
            assert!(p.x0 >= -1e-3 && p.x1 <= 100.001 && p.z0 >= -1e-3 && p.z1 <= 100.001);
        }
    }

    #[test]
    fn inset_never_turns_inside_out() {
        let r = Rect::new(0.0, 0.0, 1.0, 10.0).inset(2.0);
        assert!(r.w() >= 0.0 && r.h() >= 0.0);
        assert_eq!(r.w(), 0.0);
        assert_eq!(r.h(), 6.0);
    }
}
