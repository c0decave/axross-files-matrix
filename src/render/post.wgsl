// After the city: the bloom (a bright pass, a chain of downsamples, the
// way back up added), the picture (tone, scanlines, vignette, the warp's
// streaks, the flash guard's veil), its slow average for the veil, the
// coarse luminance grid the flash guard reads, and the copy into egui's
// frame.

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    fog: vec4<f32>,
    sky_top: vec4<f32>,
    sky_horizon: vec4<f32>,  // w: scanlines
    rain: vec4<f32>,         // w: bloom
    edge: vec4<f32>,
    grid: vec4<f32>,
    highlight: vec4<f32>,    // w: warp 0..1
    hover_min: vec4<f32>,
    hover_max: vec4<f32>,
    sel_min: vec4<f32>,
    sel_max: vec4<f32>,
    viewport: vec4<f32>,
    params: vec4<f32>,       // x: write linear, y: exposure, z: veil on, w: history weight
    debug: vec4<f32>,       // x: the tests' flashing signal
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var t0: texture_2d<f32>;
@group(1) @binding(1) var t1: texture_2d<f32>;
@group(1) @binding(2) var t2: texture_2d<f32>;
@group(1) @binding(3) var t3: texture_2d<f32>;
@group(1) @binding(4) var s: sampler;

struct FullOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> FullOut {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: FullOut;
    o.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    o.pos = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return o;
}

fn texel(t: texture_2d<f32>) -> vec2<f32> {
    return 1.0 / vec2<f32>(textureDimensions(t));
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// the bright parts, at half size (a soft knee)
@fragment
fn fs_bright(in: FullOut) -> @location(0) vec4<f32> {
    let d = texel(t0);
    var c = textureSample(t0, s, in.uv + d * vec2<f32>(-0.5, -0.5)).rgb;
    c += textureSample(t0, s, in.uv + d * vec2<f32>(0.5, -0.5)).rgb;
    c += textureSample(t0, s, in.uv + d * vec2<f32>(-0.5, 0.5)).rgb;
    c += textureSample(t0, s, in.uv + d * vec2<f32>(0.5, 0.5)).rgb;
    c *= 0.25;
    let l = luma(c);
    let knee = 0.15;
    let threshold = 0.3;
    let soft = clamp(l - threshold + knee, 0.0, 2.0 * knee);
    let w = max(soft * soft / (4.0 * knee + 1e-5), l - threshold) / max(l, 1e-5);
    return vec4<f32>(min(c * w, vec3<f32>(40.0)), 1.0);
}

// half size again (13 taps, as a box of boxes)
@fragment
fn fs_down(in: FullOut) -> @location(0) vec4<f32> {
    let d = texel(t0);
    let a = textureSample(t0, s, in.uv + d * vec2<f32>(-1.0, -1.0)).rgb;
    let b = textureSample(t0, s, in.uv + d * vec2<f32>(1.0, -1.0)).rgb;
    let c = textureSample(t0, s, in.uv + d * vec2<f32>(-1.0, 1.0)).rgb;
    let e = textureSample(t0, s, in.uv + d * vec2<f32>(1.0, 1.0)).rgb;
    let m = textureSample(t0, s, in.uv).rgb;
    let f = textureSample(t0, s, in.uv + d * vec2<f32>(-2.0, 0.0)).rgb;
    let h = textureSample(t0, s, in.uv + d * vec2<f32>(2.0, 0.0)).rgb;
    let i = textureSample(t0, s, in.uv + d * vec2<f32>(0.0, -2.0)).rgb;
    let j = textureSample(t0, s, in.uv + d * vec2<f32>(0.0, 2.0)).rgb;
    let r = (a + b + c + e) * 0.125 + m * 0.25 + (f + h + i + j) * 0.0625;
    return vec4<f32>(r, 1.0);
}

// up again (a tent), added to the larger level
@fragment
fn fs_up(in: FullOut) -> @location(0) vec4<f32> {
    let d = texel(t0) * 1.0;
    var c = textureSample(t0, s, in.uv).rgb * 4.0;
    c += textureSample(t0, s, in.uv + d * vec2<f32>(-1.0, 0.0)).rgb * 2.0;
    c += textureSample(t0, s, in.uv + d * vec2<f32>(1.0, 0.0)).rgb * 2.0;
    c += textureSample(t0, s, in.uv + d * vec2<f32>(0.0, -1.0)).rgb * 2.0;
    c += textureSample(t0, s, in.uv + d * vec2<f32>(0.0, 1.0)).rgb * 2.0;
    c += textureSample(t0, s, in.uv + d * vec2<f32>(-1.0, -1.0)).rgb;
    c += textureSample(t0, s, in.uv + d * vec2<f32>(1.0, -1.0)).rgb;
    c += textureSample(t0, s, in.uv + d * vec2<f32>(-1.0, 1.0)).rgb;
    c += textureSample(t0, s, in.uv + d * vec2<f32>(1.0, 1.0)).rgb;
    // (each level up weighs a little less: a glow round the lights, not a
    // haze over the whole sky)
    return vec4<f32>(c / 16.0 * 0.6, 1.0);
}

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn from_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

// the veil between its cells (bilinear: no blocks at its edge)
fn veil_at(uv: vec2<f32>) -> f32 {
    let dims = vec2<i32>(textureDimensions(t3));
    let p = uv * vec2<f32>(dims) - 0.5;
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    let hi = dims - vec2<i32>(1, 1);
    let a = textureLoad(t3, clamp(i, vec2<i32>(0, 0), hi), 0).r;
    let b = textureLoad(t3, clamp(i + vec2<i32>(1, 0), vec2<i32>(0, 0), hi), 0).r;
    let c = textureLoad(t3, clamp(i + vec2<i32>(0, 1), vec2<i32>(0, 0), hi), 0).r;
    let d = textureLoad(t3, clamp(i + vec2<i32>(1, 1), vec2<i32>(0, 0), hi), 0).r;
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// the picture as the city makes it: the scene and its bloom, toned,
// sRGB-encoded (and the tests' signal) - what the flash guard measures
fn raw_at(uv: vec2<f32>) -> vec3<f32> {
    let c = textureSampleLevel(t0, s, uv, 0.0).rgb + textureSampleLevel(t1, s, uv, 0.0).rgb * g.rain.w;
    let o = to_srgb(1.0 - exp(-c * g.params.y));
    // (the tests' signal: the whole picture flashing - for the guard)
    return mix(o, vec3<f32>(0.85, 0.9, 0.85), g.debug.x);
}

// the picture shown: the warp, scanlines, vignette, the veil; sRGB out
@fragment
fn fs_final(in: FullOut) -> @location(0) vec4<f32> {
    let warp = g.highlight.w;
    var o = vec3<f32>(0.0);
    if (warp > 0.001) {
        // streaks towards the middle
        let center = vec2<f32>(0.5, 0.5);
        for (var k = 0; k < 8; k++) {
            let f = f32(k) / 7.0;
            o += raw_at(center + (in.uv - center) * (1.0 - warp * 0.12 * f));
        }
        o /= 8.0;
    } else {
        o = raw_at(in.uv);
    }
    // scanlines: every third pixel row a little darker, never flickering
    let row = in.pos.y;
    let line = 0.5 - 0.5 * cos(row * 6.2831853 / 3.0);
    o *= 1.0 - g.sky_horizon.w * line;
    // vignette
    let q = in.uv * 2.0 - 1.0;
    o *= mix(1.0, smoothstep(1.6, 0.35, length(q)), 0.4);
    // the flash guard's veil: towards the picture's own slow average,
    // darkened as the veil deepens - a fade through black, not a ghost of
    // the view before
    if (g.params.z > 0.5) {
        let v = clamp(veil_at(in.uv), 0.0, 0.95);
        let slow = textureSampleLevel(t2, s, in.uv, 0.0).rgb;
        let ground = slow * mix(1.0, 0.2, v);
        o = mix(o, ground, v);
    }
    return vec4<f32>(clamp(o, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}

// the slow average of what is shown (the veil's ground)
@fragment
fn fs_history(in: FullOut) -> @location(0) vec4<f32> {
    let prev = textureSample(t0, s, in.uv).rgb;
    let now = textureSample(t1, s, in.uv).rgb;
    return vec4<f32>(mix(prev, now, g.params.w), 1.0);
}

// a cell's mean relative luminance and saturated red (WCAG), from 4 x 4
// taps of the picture as the city makes it (before the veil: the guard
// keeps seeing the danger it veils)
@fragment
fn fs_luma(in: FullOut) -> @location(0) vec4<f32> {
    // (the grid's size: crate::flash::GRID_W x GRID_H)
    let cells = vec2<f32>(48.0, 27.0);
    let base = floor(in.pos.xy) / cells;
    var l = 0.0;
    var red = 0.0;
    for (var y = 0; y < 4; y++) {
        for (var x = 0; x < 4; x++) {
            let uv = base + (vec2<f32>(f32(x), f32(y)) + 0.5) / (4.0 * cells);
            let c = from_srgb(raw_at(uv));
            l += luma(c);
            let sum = c.r + c.g + c.b;
            if (sum > 0.0 && c.r / sum >= 0.8) {
                red += 320.0 * max(0.0, c.r - c.g - c.b);
            }
        }
    }
    // (8 bits each: the red value over its largest, 320)
    return vec4<f32>(l / 16.0, red / 16.0 / 320.0, 0.0, 1.0);
}

// into egui's frame (its own encoding)
@fragment
fn fs_blit(in: FullOut) -> @location(0) vec4<f32> {
    let c = textureSample(t0, s, in.uv).rgb;
    if (g.params.x > 0.5) {
        return vec4<f32>(from_srgb(c), 1.0);
    }
    return vec4<f32>(c, 1.0);
}
