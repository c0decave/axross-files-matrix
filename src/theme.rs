//! The look: a theme is a set of tokens - the window's chrome (as
//! axross-amp's themes: ground, surfaces, text, accent, states) and the
//! city's own (the sky, the fog, the rain, the building colours by file
//! type, the glow). Matrix is the only one for now; the rest of the
//! spec's themes (Tron, night city, blueprint, clay, amber CRT) slot in
//! here as further entries of [`THEMES`].
//!
//! The chrome's text, muted text and accent are checked against the
//! surfaces they lie on (WCAG AA: 4.5 : 1) by the tests below.

use crate::tree::FileType;
use egui::{Color32, CornerRadius, Stroke};

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub key: &'static str,
    pub name: &'static str,
    // the window's chrome
    pub ground: Color32,
    pub surface: Color32,
    pub raised: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub text: Color32,
    pub strong: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub border: Color32,
    pub selection: Color32,
    pub warn: Color32,
    pub bad: Color32,
    // the city (linear-ish RGB 0..1 for the shaders)
    pub sky_top: [f32; 3],
    pub sky_horizon: [f32; 3],
    pub fog: [f32; 3],
    pub rain: [f32; 3],
    pub grid: [f32; 3],
    pub edge: [f32; 3],
    pub highlight: [f32; 3],
    /// A building's colour by file type (index: [`FileType`] as u8).
    pub types: [Color32; 11],
    /// The labels above the city.
    pub label: Color32,
    pub label_dim: Color32,
    /// Bloom strength, scanline depth (0..1), how much the rain shows.
    pub bloom: f32,
    pub scanlines: f32,
    pub rain_amount: f32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub const MATRIX: Theme = Theme {
    key: "matrix",
    name: "Matrix",
    ground: rgb(0x020803),
    surface: rgb(0x061208),
    raised: rgb(0x0b1f0f),
    hover: rgb(0x123019),
    pressed: rgb(0x1a4424),
    text: rgb(0x9cf5ae),
    strong: rgb(0xd8ffe0),
    muted: rgb(0x5fb874),
    accent: rgb(0x33ff66),
    border: rgb(0x1f5a2c),
    selection: rgb(0x145c26),
    warn: rgb(0xd6e85a),
    bad: rgb(0xff8a6a),
    sky_top: [0.0, 0.0006, 0.0001],
    sky_horizon: [0.0005, 0.0045, 0.0016],
    fog: [0.0003, 0.0028, 0.001],
    rain: [0.25, 1.0, 0.45],
    grid: [0.05, 0.55, 0.18],
    edge: [0.35, 1.0, 0.5],
    highlight: [0.85, 1.0, 0.9],
    types: [
        rgb(0x1d7a3a), // folder
        rgb(0x39ff6a), // code
        rgb(0x8cf0aa), // document
        rgb(0x2fe0b4), // image
        rgb(0xa6f53c), // video
        rgb(0x62f7b8), // audio
        rgb(0xd4e05a), // archive
        rgb(0x1fae4a), // binary
        rgb(0x44c8e0), // data
        rgb(0x6fd88a), // config
        rgb(0x46a35e), // other
    ],
    label: rgb(0xb8ffc8),
    label_dim: rgb(0x58b06c),
    bloom: 0.5,
    scanlines: 0.10,
    rain_amount: 1.0,
};

/// The themes, the first the default.
pub const THEMES: [Theme; 1] = [MATRIX];

pub fn find(key: &str) -> &'static Theme {
    THEMES.iter().find(|t| t.key == key).unwrap_or(&THEMES[0])
}

impl Theme {
    pub fn type_color(&self, t: FileType) -> Color32 {
        self.types[t as usize]
    }
}

/// egui's look for a theme.
pub fn visuals(t: &Theme) -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.dark_mode = true;
    v.override_text_color = Some(t.text);
    v.panel_fill = t.ground;
    v.window_fill = t.surface;
    v.extreme_bg_color = t.ground;
    v.faint_bg_color = t.raised;
    v.code_bg_color = t.ground;
    v.hyperlink_color = t.accent;
    v.warn_fg_color = t.warn;
    v.error_fg_color = t.bad;
    v.window_stroke = Stroke::new(1.0, t.border);
    v.window_corner_radius = CornerRadius::same(4);
    v.menu_corner_radius = CornerRadius::same(4);
    v.selection.bg_fill = t.selection;
    v.selection.stroke = Stroke::new(1.0, t.strong);
    v.window_shadow = egui::Shadow {
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: Color32::from_rgba_unmultiplied(0, 40, 10, 90),
    };
    v.popup_shadow = v.window_shadow;
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = t.surface;
    w.noninteractive.weak_bg_fill = t.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    w.inactive.bg_fill = t.raised;
    w.inactive.weak_bg_fill = t.raised;
    w.inactive.bg_stroke = Stroke::new(1.0, t.border);
    w.inactive.fg_stroke = Stroke::new(1.0, t.text);
    w.hovered.bg_fill = t.hover;
    w.hovered.weak_bg_fill = t.hover;
    w.hovered.bg_stroke = Stroke::new(1.0, t.accent);
    w.hovered.fg_stroke = Stroke::new(1.5, t.strong);
    w.active.bg_fill = t.pressed;
    w.active.weak_bg_fill = t.pressed;
    w.active.bg_stroke = Stroke::new(1.0, t.accent);
    w.active.fg_stroke = Stroke::new(1.5, t.strong);
    w.open.bg_fill = t.pressed;
    w.open.weak_bg_fill = t.pressed;
    w.open.fg_stroke = Stroke::new(1.0, t.strong);
    for s in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        s.corner_radius = CornerRadius::same(3);
    }
    v
}

/// A colour's linear RGB (0..1).
pub fn linear(c: Color32) -> [f32; 3] {
    let f = |v: u8| {
        let c = v as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    [f(c.r()), f(c.g()), f(c.b())]
}

/// WCAG relative luminance.
pub fn luminance(c: Color32) -> f32 {
    let l = linear(c);
    0.2126 * l[0] + 0.7152 * l[1] + 0.0722 * l[2]
}

pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_readable_on_every_surface() {
        for t in &THEMES {
            for surface in [
                t.ground,
                t.surface,
                t.raised,
                t.hover,
                t.pressed,
                t.selection,
            ] {
                assert!(
                    contrast(t.text, surface) >= 4.5,
                    "{} text on {surface:?}",
                    t.key
                );
                assert!(contrast(t.strong, surface) >= 4.5, "{} strong", t.key);
            }
            for surface in [t.ground, t.surface, t.raised] {
                assert!(
                    contrast(t.muted, surface) >= 4.5,
                    "{} muted on {surface:?}",
                    t.key
                );
                assert!(contrast(t.accent, surface) >= 4.5, "{} accent", t.key);
                assert!(contrast(t.warn, surface) >= 4.5 && contrast(t.bad, surface) >= 4.5);
            }
            // the labels float over a dark city
            assert!(contrast(t.label, Color32::BLACK) >= 7.0);
            assert!(contrast(t.label_dim, Color32::BLACK) >= 4.5);
        }
    }

    #[test]
    fn no_saturated_red_in_the_city() {
        // (WCAG 2.3.2's red flashes: no building colour is saturated red)
        for t in &THEMES {
            for c in t.types {
                let l = linear(c);
                let sum = l[0] + l[1] + l[2];
                assert!(sum <= 0.0 || l[0] / sum < 0.8, "{c:?}");
            }
        }
    }

    #[test]
    fn unknown_keys_fall_back_to_matrix() {
        assert_eq!(find("nope").key, "matrix");
    }
}
