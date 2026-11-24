// The city: the sky with its digital rain, the dark sea with its grid, the
// buildings, blocks, district floors and islands (instanced boxes whose
// edges glow), the beams and the search hits' pillars. Linear HDR out;
// post.wgsl makes the bloom, the scanlines and the picture shown.

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,          // xyz, w: time (s)
    fog: vec4<f32>,          // rgb, density
    sky_top: vec4<f32>,      // rgb, rain amount
    sky_horizon: vec4<f32>,  // rgb, scanlines
    rain: vec4<f32>,         // rgb, bloom
    edge: vec4<f32>,         // rgb, glow boost
    grid: vec4<f32>,         // rgb, motion (1 normal, 0 reduced)
    highlight: vec4<f32>,    // rgb, warp 0..1
    hover_min: vec4<f32>,    // w: 1 when something is under the pointer
    hover_max: vec4<f32>,
    sel_min: vec4<f32>,      // w: 1 when something is chosen
    sel_max: vec4<f32>,
    viewport: vec4<f32>,     // width, height, 1/width, 1/height (pixels)
    params: vec4<f32>,       // x: write linear, y: exposure, z: veil on, w: frame dt
    debug: vec4<f32>,       // x: the tests' flashing signal
    look: vec4<f32>,        // x: most a far roof is raised (0: never), y: how fast, z: the haze's height (m), w: most fog a box gets
    lens: vec4<f32>,        // x: 2 tan(fov / 2) (metres a pixel = distance * x / height), y: how fast the view moves (0 still .. 1 fast)
};

@group(0) @binding(0) var<uniform> g: Globals;

fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(123.34, 456.21));
    q += dot(q, q + 45.32);
    return fract(q.x * q.y);
}

fn hash11(x: f32) -> f32 {
    return fract(sin(x * 91.3458) * 47453.5453);
}

fn unpack_color(c: u32) -> vec3<f32> {
    let v = unpack4x8unorm(c).rgb;
    return pow(v, vec3<f32>(2.2));
}

// One glyph of the rain: a 5 x 7 grid of rounded dots, a third to a half
// lit, chosen by `id`; `p` within the cell (0..1).
fn glyph(p: vec2<f32>, id: f32) -> f32 {
    let cell = floor(p * vec2<f32>(5.0, 7.0));
    if (p.x < 0.0 || p.x >= 1.0 || p.y < 0.0 || p.y >= 1.0) {
        return 0.0;
    }
    // mirrored a little: more like letters
    let cx = select(cell.x, 4.0 - cell.x, hash11(id * 3.1) > 0.6 && cell.x > 2.0);
    let on = step(0.52, hash21(vec2<f32>(cx + id * 7.0, cell.y + id * 13.0)));
    let f = fract(p * vec2<f32>(5.0, 7.0)) - 0.5;
    let dot_ = 1.0 - smoothstep(0.28, 0.48, length(f));
    return on * dot_;
}

// One column's drops: three falling at their own speeds, each a bright
// head and a fading trail; the glyphs change slowly, one cell at a time.
fn rain_column(col: f32, row: f32, x: f32, y: f32, t: f32) -> f32 {
    var best = 0.0;
    for (var k = 0; k < 3; k++) {
        let kk = f32(k);
        let speed = 3.0 + hash11(col * 1.3 + kk * 7.1) * 6.0;
        let phase = hash11(col + 17.0 + kk * 3.3) * 200.0;
        let span = 60.0 + hash11(col + 5.0 + kk * 11.0) * 70.0;
        let length = 10.0 + hash11(col * 0.7 + kk) * 22.0;
        let head = span - fract((t * speed * g.grid.w + phase) / span) * span;
        let behind = row - head;
        if (behind >= 0.0 && behind <= length) {
            let trail = pow(1.0 - behind / length, 1.8);
            best = max(best, trail * select(1.0, 2.4, behind < 1.0));
        }
    }
    if (best <= 0.0) {
        return 0.0;
    }
    let tick = floor(t * (0.4 + hash11(col + row) * 0.6) * g.grid.w + hash11(row * 3.0 + col));
    let gl = glyph(vec2<f32>(x * 1.3 - 0.15, y * 1.12 - 0.06), hash21(vec2<f32>(col, row + tick * 31.0)) * 977.0);
    return gl * best;
}

// The rain at angles (around 0..1, up in radians).
fn rain_at(around: f32, up: f32, t: f32) -> f32 {
    let cols = 330.0;
    let col = floor(around * cols);
    let x = fract(around * cols);
    let cell_h = 1.3 / cols * 6.2831;
    let row_f = up / cell_h;
    return rain_column(col, floor(row_f), x, fract(row_f), t);
}

struct FullOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> FullOut {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: FullOut;
    o.ndc = uv * 2.0 - 1.0;
    o.pos = vec4<f32>(o.ndc, 0.0, 1.0);
    return o;
}

fn view_ray(ndc: vec2<f32>) -> vec3<f32> {
    let a = g.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    let b = g.inv_view_proj * vec4<f32>(ndc, 0.5, 1.0);
    return normalize(b.xyz / b.w - a.xyz / a.w);
}

// The horizon's colour (the sky meets the sea's fog in it: no seam).
fn horizon() -> vec3<f32> {
    return g.sky_horizon.rgb * 1.6;
}

fn sky(dir: vec3<f32>) -> vec3<f32> {
    let up = clamp(dir.y, 0.0, 1.0);
    var c = mix(horizon(), g.sky_top.rgb, pow(clamp(up * 3.0, 0.0, 1.0), 0.55));
    // a faint green glow along the horizon (the city's light in the haze)
    c += g.grid.rgb * (0.012 * exp(-up * 14.0) + 0.004 * exp(-up * 3.0));
    if (g.sky_top.w > 0.0 && dir.y > -0.02) {
        let around = atan2(dir.x, -dir.z) / 6.2831853 + 0.5;
        let elev = asin(clamp(dir.y, -1.0, 1.0));
        let r = rain_at(around, elev, g.eye.w);
        // thinner towards the zenith, gone below the horizon
        let fade = smoothstep(-0.02, 0.08, dir.y) * (1.0 - smoothstep(0.9, 1.45, elev));
        // (calmer while the view turns: the drops' heads sweep less)
        c += g.rain.rgb * r * 0.55 * fade * g.sky_top.w * (1.0 - 0.5 * g.lens.y);
    }
    return c;
}

@fragment
fn fs_sky(in: FullOut) -> @location(0) vec4<f32> {
    let dir = view_ray(in.ndc);
    return vec4<f32>(sky(dir), 1.0);
}

struct SeaOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

const SEA: f32 = -1.0;

@fragment
fn fs_sea(in: FullOut) -> SeaOut {
    let dir = view_ray(in.ndc);
    var o: SeaOut;
    if (dir.y >= -1e-4) {
        discard;
    }
    let t = (SEA - g.eye.y) / dir.y;
    let p = g.eye.xyz + dir * t;
    // the grid: every 20 m, brighter every 100 m, fading off
    let q = p.xz / 20.0;
    let w = fwidth(q);
    let gl = abs(fract(q - 0.5) - 0.5) / max(w, vec2<f32>(1e-4));
    let line = 1.0 - smoothstep(0.0, 1.5, min(gl.x, gl.y));
    let q2 = p.xz / 100.0;
    let w2 = fwidth(q2);
    let gl2 = abs(fract(q2 - 0.5) - 0.5) / max(w2, vec2<f32>(1e-4));
    let line2 = 1.0 - smoothstep(0.0, 1.5, min(gl2.x, gl2.y));
    let fade = exp(-t * 0.0012);
    // the lines fade where they are finer than a pixel (no moire, no
    // scribbles at a grazing angle): by the larger of the two derivatives
    let fine = 1.0 - smoothstep(0.2, 0.5, max(w.x, w.y));
    let fine2 = 1.0 - smoothstep(0.15, 0.45, max(w2.x, w2.y));
    var c = g.fog.rgb * 0.5 + g.grid.rgb * (line * 0.08 * fine + line2 * 0.14 * fine2) * fade;
    // far off the sea becomes the horizon (the sky goes on from there)
    let fog = 1.0 - exp(-t * g.fog.w * 1.6);
    // (with the sky's glow at the horizon: no seam where they meet)
    c = mix(c, horizon() + g.grid.rgb * 0.016, fog);
    // the rain mirrored in the far water: never a flat black half
    if (g.sky_top.w > 0.0 && fog > 0.15) {
        let around = atan2(dir.x, -dir.z) / 6.2831853 + 0.5;
        let elev = asin(clamp(-dir.y, 0.0, 1.0));
        let r = rain_at(around, elev, g.eye.w);
        c += g.rain.rgb * r * 0.07 * smoothstep(0.15, 0.7, fog) * g.sky_top.w;
    }
    o.color = vec4<f32>(c, 1.0);
    let clip = g.view_proj * vec4<f32>(p, 1.0);
    o.depth = clip.z / clip.w;
    return o;
}

// ---------------------------------------------------------------- boxes

struct Inst {
    @location(0) pos: vec3<f32>,
    @location(1) info: u32,
    @location(2) size: vec3<f32>,
    @location(3) color: u32,
};

struct BoxOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) local: vec3<f32>,
    @location(2) @interpolate(flat) normal: vec3<f32>,
    @location(3) @interpolate(flat) size: vec3<f32>,
    @location(4) @interpolate(flat) info: u32,
    @location(5) @interpolate(flat) color: vec3<f32>,
    @location(6) @interpolate(flat) bmin: vec3<f32>,
    @location(7) @interpolate(flat) seed: f32,
    // (the instance's own corner and size: the hover's and choice's match)
    @location(8) @interpolate(flat) own_size: vec3<f32>,
    @location(9) @interpolate(flat) own_min: vec3<f32>,
};

// How much higher a building or block `d` metres away is drawn: 1 near,
// growing with the log of the distance from 300 m on (eased in: no
// kink), at most look.x - the far city keeps its relief.
fn stretch_at(d: f32) -> f32 {
    if (g.look.x <= 1.0) {
        return 1.0;
    }
    let x = max(d / 300.0 - 1.0, 0.0);
    return min(1.0 + g.look.y * 0.5 * log(1.0 + x * x), g.look.x);
}

// Metres a pixel spans `d` metres away.
fn pixel_at(d: f32) -> f32 {
    return d * g.lens.x / max(g.viewport.y, 1.0);
}

// The faces: +x, -x, +y, +z, -z (the bottom is never seen); each with two
// tangents whose cross product is its normal (counter-clockwise outside).
fn face_corner(face: u32, u: f32, v: f32) -> vec3<f32> {
    switch face {
        case 0u: { return vec3<f32>(1.0, u, v); }
        case 1u: { return vec3<f32>(0.0, v, u); }
        case 2u: { return vec3<f32>(v, 1.0, u); }
        case 3u: { return vec3<f32>(u, v, 1.0); }
        default: { return vec3<f32>(v, u, 0.0); }
    }
}

fn face_normal(face: u32) -> vec3<f32> {
    switch face {
        case 0u: { return vec3<f32>(1.0, 0.0, 0.0); }
        case 1u: { return vec3<f32>(-1.0, 0.0, 0.0); }
        case 2u: { return vec3<f32>(0.0, 1.0, 0.0); }
        case 3u: { return vec3<f32>(0.0, 0.0, 1.0); }
        default: { return vec3<f32>(0.0, 0.0, -1.0); }
    }
}

@vertex
fn vs_box(@builtin(vertex_index) vi: u32, inst: Inst) -> BoxOut {
    let face = vi / 6u;
    let k = vi % 6u;
    var uv = vec2<f32>(0.0, 0.0);
    switch k {
        case 1u: { uv = vec2<f32>(1.0, 0.0); }
        case 2u, 4u: { uv = vec2<f32>(1.0, 1.0); }
        case 5u: { uv = vec2<f32>(0.0, 1.0); }
        default: {}
    }
    let local = face_corner(face, uv.x, uv.y);
    let shape = inst.info & 15u;
    var size = inst.size;
    var corner = inst.pos;
    let d = length(inst.pos.xz + inst.size.xz * 0.5 - g.eye.xz);
    if (shape <= 1u && (inst.info & 64u) == 0u) {
        // (the same as crate::render::Raise::apply: picking, the highlight
        // and the labels follow what is drawn)
        if (shape == 0u) {
            // a far tower: never thinner than two pixels (no needles)
            let want = pixel_at(d) * 2.0;
            let gx = max(want - size.x, 0.0);
            let gz = max(want - size.z, 0.0);
            corner = vec3<f32>(corner.x - gx * 0.5, corner.y, corner.z - gz * 0.5);
            size = vec3<f32>(size.x + gx, size.y, size.z + gz);
        }
        // far roofs raised a little (never the machine's parts) - seen
        // from aside; looked down on from above, as they are (no towers
        // leaning out of the map); a slim tower at most twelve of its
        // widths high
        let rel = vec3<f32>(d, g.eye.y - inst.pos.y, 0.0);
        let down = clamp(rel.y / max(length(rel), 1e-3), 0.0, 1.0);
        let k = 1.0 - smoothstep(0.55, 0.92, down);
        var f = 1.0 + (stretch_at(d) - 1.0) * k;
        f = min(f, max(1.0, 12.0 * min(size.x, size.z) / max(size.y, 1e-3)));
        size.y *= f;
        if (shape == 1u) {
            // a far block stands back from its plate's edge by at least
            // a pixel and a half: the streets stay lit lines
            let inset = min(pixel_at(d) * 0.9, min(size.x, size.z) * 0.22);
            corner = vec3<f32>(corner.x + inset, corner.y, corner.z + inset);
            size = vec3<f32>(size.x - inset * 2.0, size.y, size.z - inset * 2.0);
        }
    } else if (shape == 5u || shape == 6u) {
        // a pillar and its ring: never thinner than a few pixels
        let want = pixel_at(d) * select(3.0, 14.0, shape == 6u);
        let grow = max(want - inst.size.x, 0.0);
        corner = vec3<f32>(corner.x - grow * 0.5, corner.y, corner.z - grow * 0.5);
        size = vec3<f32>(size.x + grow, size.y, size.z + grow);
    }
    var p = local * size;
    var n = face_normal(face);
    let turn = f32(inst.info >> 16u) / 65536.0 * 6.2831853;
    if (turn != 0.0) {
        let c = cos(turn);
        let s = sin(turn);
        p = vec3<f32>(p.x * c - p.z * s, p.y, p.x * s + p.z * c);
        n = vec3<f32>(n.x * c - n.z * s, n.y, n.x * s + n.z * c);
    }
    let world = corner + p;
    var o: BoxOut;
    o.clip = g.view_proj * vec4<f32>(world, 1.0);
    o.world = world;
    o.local = local;
    o.normal = n;
    o.size = size;
    o.info = inst.info;
    o.color = unpack_color(inst.color);
    o.bmin = corner;
    o.own_min = inst.pos;
    o.own_size = inst.size;
    o.seed = fract(inst.pos.x * 0.1731 + inst.pos.z * 0.3117 + inst.size.y * 0.071);
    return o;
}

fn same_box(bmin: vec3<f32>, size: vec3<f32>, lo: vec4<f32>, hi: vec4<f32>) -> bool {
    if (lo.w < 0.5) {
        return false;
    }
    let e = 0.02 + 0.001 * length(size);
    return all(abs(bmin - lo.xyz) < vec3<f32>(e)) && all(abs(bmin + size - hi.xyz) < vec3<f32>(e));
}

// How near a point on a face is to the face's edges, in pixels (for
// lines about `width` pixels wide whatever the distance).
fn edge_px(local: vec3<f32>, size: vec3<f32>, n: vec3<f32>) -> f32 {
    let q = local * size;
    let d = min(q, size - q);
    var a = 1e9;
    var b = 1e9;
    let an = abs(n);
    if (an.x > 0.5) { a = d.y; b = d.z; }
    else if (an.y > 0.5) { a = d.x; b = d.z; }
    else { a = d.x; b = d.y; }
    let da = a / max(fwidth(a), 1e-5);
    let db = b / max(fwidth(b), 1e-5);
    return min(da, db);
}

// How many pixels the face spans at this point (its smaller side).
fn face_px(local: vec3<f32>, size: vec3<f32>, n: vec3<f32>) -> f32 {
    let q = local * size;
    let an = abs(n);
    var a = 0.0;
    var b = 0.0;
    var sa = 0.0;
    var sb = 0.0;
    if (an.x > 0.5) { a = q.y; b = q.z; sa = size.y; sb = size.z; }
    else if (an.y > 0.5) { a = q.x; b = q.z; sa = size.x; sb = size.z; }
    else { a = q.x; b = q.y; sa = size.x; sb = size.y; }
    return min(sa / max(fwidth(a), 1e-5), sb / max(fwidth(b), 1e-5));
}

// How much fog lies between the eye and `world`: thicker near the ground
// (towers stand out of the haze), never more than look.w (a city is never
// under a quarter of its contrast).
fn fog_amount(world: vec3<f32>) -> f32 {
    let d = length(world - g.eye.xyz);
    let h = max(g.look.z, 1.0);
    let y0 = max(g.eye.y, 0.0);
    let y1 = max(world.y, 0.0);
    let dy = y1 - y0;
    var low = exp(-y0 / h);
    if (abs(dy) > 0.01 * h) {
        low = h * (exp(-y0 / h) - exp(-y1 / h)) / dy;
    }
    let f = 1.0 - exp(-d * g.fog.w * (0.4 + 0.9 * low));
    return min(f, select(1.0, g.look.w, g.look.w > 0.0));
}

fn fog_mix(c: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    return mix(c, mix(g.fog.rgb, horizon(), 0.6), fog_amount(world));
}

// Lit specks on a far face: cells about three pixels wide (by powers of
// two, each level blending into the next: nothing swims), a share
// `density` of them on.
fn speckle(a: f32, b: f32, seed: f32, density: f32) -> f32 {
    let px = max(max(fwidth(a), fwidth(b)), 1e-4);
    let lv = log2(px * 3.0);
    let l0 = floor(lv);
    let f = lv - l0;
    let s0 = exp2(l0);
    let q = vec2<f32>(a, b);
    let h0 = step(hash21(floor(q / s0) + seed * 57.0), density);
    let h1 = step(hash21(floor(q / (s0 * 2.0)) + seed * 57.0 + 7.0), density);
    return mix(h0, h1, f);
}

@fragment
fn fs_box(in: BoxOut) -> @location(0) vec4<f32> {
    let shape = in.info & 15u;
    let unread = (in.info & 16u) != 0u;
    let denied = (in.info & 32u) != 0u;
    let hardware = (in.info & 64u) != 0u;
    let remote = (in.info & 128u) != 0u;
    let glow = f32((in.info >> 8u) & 255u) / 255.0;
    let n = in.normal;
    let t = g.eye.w;
    var base = in.color;
    if (remote) {
        base = mix(base, vec3<f32>(0.05, 0.6, 0.85), 0.45);
    }
    let ep = edge_px(in.local, in.size, n);
    let edge = 1.0 - smoothstep(0.6, 1.9, ep);
    let halo = exp(-ep * 0.6) * 0.18;
    // how many pixels the face spans: a face of a few pixels is mostly
    // edge - its light is turned down, or the far city turns white
    let across_face = face_px(in.local, in.size, n);
    // (below two pixels a face is its flat average: no shimmer; a floor's
    // edge - the street - stays a line)
    var small = clamp((across_face - 2.0) / 10.0, 0.0, 1.0);
    if (shape == 2u) {
        // (a floor of a few pixels: its outline fades to the floor's glow
        // - tiny districts' lines would shimmer as the view moves)
        small = max(small, 0.45 * smoothstep(3.0, 9.0, across_face));
    }
    let lit = 0.35 + 0.65 * max(dot(n, normalize(vec3<f32>(0.35, 0.9, 0.25))), 0.0);
    var c = vec3<f32>(0.0);
    var edge_gain = 1.0;
    let is_top = n.y > 0.5;
    switch shape {
        // a building: dark glass, lit windows, glowing edges
        case 0u, 1u: {
            let block = shape == 1u;
            c = base * (0.035 + 0.03 * lit);
            if (!is_top) {
                // windows: by the face's width and the height in metres
                let across = select(in.local.x * in.size.x, in.local.z * in.size.z, abs(n.x) > 0.5);
                let h = in.local.y * in.size.y;
                let cw = select(0.7, 0.45, block);
                let ch = select(0.9, 0.6, block);
                let cell = vec2<f32>(floor(across / cw), floor(h / ch));
                let f = vec2<f32>(fract(across / cw), fract(h / ch));
                let p_on = 0.12 + 0.45 * glow;
                let on = step(hash21(cell + in.seed * 113.0), p_on);
                let win = on * step(0.3, f.x) * step(f.x, 0.7) * step(0.34, f.y) * step(f.y, 0.66);
                // too small to see: lit specks (the far city's windows)
                let fine = smoothstep(0.35, 0.9, max(fwidth(across) / cw, fwidth(h) / ch));
                // (fainter while the view moves fast: specks sweeping past
                // are no flicker)
                let far_lit = speckle(across, h, in.seed, p_on * 0.55) * 0.7 * (1.0 - 0.65 * g.lens.y);
                var w = mix(win, far_lit, fine);
                // windows passing while the view moves: their mean light - a
                // wall flown along is no strobe
                // (and while the view moves fast, every window of more than
                // a few pixels: lit squares sweeping by are what the eye
                // takes as flicker - back the moment the view rests)
                let px = min(fwidth(across) / cw, fwidth(h) / ch);
                let big = 1.0 - smoothstep(0.06, 0.2, px);
                w = mix(w, p_on * 0.15, big * g.lens.y);
                c += base * w * (0.22 + 0.55 * glow);
                // darker towards the street, lighter up high: depth
                c *= 0.55 + 0.6 * in.local.y;
                // code running down some of the tallest faces
                if (in.size.y > 6.0 && g.sky_top.w > 0.0 && in.seed < 0.45) {
                    let col = floor(across / 0.55);
                    let speed = 1.2 + hash11(col + in.seed * 50.0) * 2.0;
                    let head = fract(-(h / in.size.y) + t * speed * 0.03 * g.grid.w + hash11(col * 1.7 + in.seed));
                    let tr = pow(head, 10.0) * (1.0 - fine) * (1.0 - g.lens.y);
                    c += base * tr * 0.45 * (0.3 + glow);
                }
            } else {
                // the roof: a lit rim inside the edge; a far block's roof
                // in its type's colour with specks of light (the city from
                // afar is never black, never grey)
                c = base * select(0.025 + 0.05 * glow, 0.09 + 0.12 * glow, block);
                if (block) {
                    let q = in.local.xz * in.size.xz;
                    c += base * speckle(q.x, q.y, in.seed + 0.5, 0.18 + 0.3 * glow) * (0.25 + 0.4 * glow) * (1.0 - 0.65 * g.lens.y);
                }
            }
            edge_gain = select(1.0, 0.75, block) * (0.55 + 1.6 * glow);
            if (unread) {
                // a folder not read yet: dashed and dim
                let s = (in.local.x * in.size.x + in.local.y * in.size.y + in.local.z * in.size.z) * 0.8;
                edge_gain *= 0.35 + 0.65 * step(0.5, fract(s));
                c *= 0.4;
            }
        }
        // a district's floor: almost black, its outline the street light
        case 2u: {
            // (from afar only its edges and the streets between far blocks
            // show: a faint glow, so the districts read as a lit board)
            let away = smoothstep(150.0, 900.0, length(in.world - g.eye.xyz));
            c = base * 0.02 + g.grid.rgb * 0.035 * away;
            edge_gain = 0.55 * (1.0 - glow * 0.6) * (1.0 + away);
        }
        // an island: its ground and its rim
        case 3u: {
            c = vec3<f32>(0.004, 0.012, 0.006);
            if (is_top && hardware) {
                // the mainboard: traces between pads
                let q = in.local.xz * in.size.xz;
                let lane = floor(q / 2.5);
                let f = fract(q / 2.5);
                let w = fwidth(q.x) / 2.5;
                let along_x = step(hash21(lane.yy * 1.7 + 3.0), 0.55) * step(hash21(vec2<f32>(floor(q.x / 14.0), lane.y)), 0.6);
                let along_z = step(hash21(lane.xx * 2.3 + 9.0), 0.35) * step(hash21(vec2<f32>(lane.x, floor(q.y / 18.0))), 0.5);
                let tx = along_x * (1.0 - smoothstep(0.06, 0.06 + w * 1.5, abs(f.y - 0.5)));
                let tz = along_z * (1.0 - smoothstep(0.06, 0.06 + w * 1.5, abs(f.x - 0.5)));
                let pad = step(hash21(lane + 41.0), 0.06) * (1.0 - smoothstep(0.16, 0.2, length(f - 0.5)));
                let fine = 1.0 - smoothstep(0.15, 0.4, w);
                c += g.grid.rgb * (max(tx, tz) * 0.16 + pad * 0.3) * fine;
            } else if (is_top) {
                let q = (in.local.xz * in.size.xz) / 10.0;
                let w = fwidth(q);
                let gl = abs(fract(q - 0.5) - 0.5) / max(w, vec2<f32>(1e-4));
                let line = 1.0 - smoothstep(0.0, 1.5, min(gl.x, gl.y));
                // (thinned with distance, gone below half a pixel)
                let fine = 1.0 - smoothstep(0.15, 0.45, max(w.x, w.y));
                c += g.grid.rgb * line * 0.05 * fine;
                if (denied) {
                    // failed: amber hatching across the plot
                    let s = (in.local.x * in.size.x + in.local.z * in.size.z) / 9.0;
                    let sw = max(fwidth(s), 1e-4);
                    let stripe = smoothstep(0.5 - sw, 0.5 + sw, fract(s)) * (1.0 - smoothstep(0.8 - sw, 0.8 + sw, fract(s)));
                    let hfine = 1.0 - smoothstep(0.2, 0.5, sw);
                    c += vec3<f32>(0.5, 0.3, 0.04) * stripe * 0.09 * hfine + vec3<f32>(0.02, 0.012, 0.0);
                }
            }
            edge_gain = 1.4;
            if (unread) {
                // connecting: the outline breathes (half a second's swell,
                // well under three a second; still with reduced motion)
                edge_gain *= 0.7 + 0.6 * (0.5 + 0.5 * sin(t * 3.0 * g.grid.w));
            }
        }
        // the beam: light that runs from the disk to the island
        case 4u: {
            let along = in.local.x * in.size.x;
            let pulse = smoothstep(0.85, 1.0, fract(along / 60.0 - t * 0.25 * g.grid.w));
            let core = 1.0 - abs(in.local.z - 0.5) * 2.0;
            let e = (0.6 + 0.5 * pulse) * core;
            return vec4<f32>(fog_mix(base * e * 1.6, in.world), 1.0);
        }
        // a search hit's pillar: fading as it rises, breathing slowly
        case 5u: {
            let k = pow(1.0 - in.local.y, 2.0);
            let breathe = 0.85 + 0.15 * sin(t * 2.0 * g.grid.w);
            let across = select(in.local.x, in.local.z, abs(n.x) > 0.5);
            let side = 1.0 - abs(across - 0.5) * 2.0;
            // (close by it fades: a beam, never a white wall; fainter while
            // the view moves fast - beams sweeping past are no flicker)
            let near = clamp(length(in.world - g.eye.xyz) / 250.0, 0.05, 1.0) * (1.0 - 0.85 * g.lens.y);
            // tinted by what was found, its core a little whiter
            let tint = mix(g.highlight.rgb, base, 0.75 - 0.35 * side * side);
            return vec4<f32>(tint * k * breathe * 0.6 * near * (0.25 + 0.75 * side), 1.0);
        }
        // its ring on the ground: bright near, fading with distance
        case 6u: {
            if (!is_top) {
                return vec4<f32>(0.0, 0.0, 0.0, 1.0);
            }
            let r = length(in.local.xz - vec2<f32>(0.5)) * 2.0;
            let w = max(fwidth(r), 1e-4);
            let ring = smoothstep(0.78 - w, 0.78 + w, r) * (1.0 - smoothstep(0.92 - w, 0.92 + w, r));
            let inner = (1.0 - smoothstep(0.0, 0.8, r)) * 0.15;
            let away = exp(-length(in.world - g.eye.xyz) / 2500.0);
            let tint = mix(g.highlight.rgb, base, 0.5);
            return vec4<f32>(tint * (ring + inner) * (0.25 + 0.75 * away) * 0.9, 1.0);
        }
        default: {}
    }
    var edge_color = mix(base, g.edge.rgb, 0.35) * g.edge.w;
    if (hardware) {
        edge_color = mix(edge_color, vec3<f32>(0.4, 1.0, 0.8), 0.3);
    }
    if (denied) {
        // amber, never saturated red
        edge_color = vec3<f32>(0.9, 0.6, 0.12);
    }
    var gain = edge_gain;
    if (same_box(in.own_min, in.own_size, g.hover_min, g.hover_max)) {
        gain *= 2.0;
        c += base * 0.025;
    }
    if (same_box(in.own_min, in.own_size, g.sel_min, g.sel_max)) {
        edge_color = g.highlight.rgb;
        gain = 2.6 + 0.4 * sin(t * 2.6);
        c += g.highlight.rgb * 0.05;
    }
    // (the edges' glow a little softer while the view moves fast: lit lines
    // sweeping across the picture, not flashing)
    let calm = 1.0 - 0.35 * g.lens.y;
    c += edge_color * (edge * gain * 1.3 + halo * gain * 0.25) * small * calm;
    // (what the edges give a face too small to draw them: their average)
    c += edge_color * gain * 0.08 * (1.0 - small);
    // far away, bright points are pressed down (no blown-out specks for the
    // bloom to spread); near, nothing changes
    // (and never more than a bright light: no infinity reaches the bloom)
    c = min(max(c, vec3<f32>(0.0)), vec3<f32>(48.0));
    let far = clamp(length(in.world - g.eye.xyz) / 1200.0, 0.0, 1.5);
    c = c / (1.0 + far * max(max(c.r, c.g), c.b));
    return vec4<f32>(fog_mix(c, in.world), 1.0);
}
