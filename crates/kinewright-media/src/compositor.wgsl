struct LayerParams {
    brightness: f32,
    contrast: f32,
    saturation: f32,
    opacity: f32,
    scale: f32,
    offset_x: f32,
    offset_y: f32,
    fade_mix: f32,
    fade_white: f32,
    crop_left: f32,
    crop_right: f32,
    crop_top: f32,
    crop_bottom: f32,
    reframe_aspect: f32,
    reframe_focus_x: f32,
    reframe_focus_y: f32,
    exposure: f32,
    temperature: f32,
    tint: f32,
    lut_preset: f32,
    lut_intensity: f32,
    mask_shape: f32,
    mask_center_x: f32,
    mask_center_y: f32,
    mask_width: f32,
    mask_height: f32,
    mask_feather: f32,
    mask_invert: f32,
    key_red: f32,
    key_green: f32,
    key_blue: f32,
    key_threshold: f32,
    key_softness: f32,
    key_spill: f32,
    external_lut_enabled: f32,
    external_lut_intensity: f32,
    external_domain_min_r: f32,
    external_domain_min_g: f32,
    external_domain_min_b: f32,
    external_domain_max_r: f32,
    external_domain_max_g: f32,
    external_domain_max_b: f32,
    input_linear: f32,
    legacy_stage_active: f32,
    // CC4 4.1: the two words that used to be `_uniform_padding` now address
    // the legacy `cube_lut`'s slot inside the shared depth-packed atlas, so
    // `LayerParams` stays exactly 48 floats.
    external_lut_z_origin: f32,
    external_lut_size: f32,
};

// CC3 3.2: ONE read-only storage buffer carries the whole ordered managed
// colour-node stack -- primary, wheels, and curves -- plus the curve payload
// region.  Keep the host ABI explicit:
//
//   header.x  active node count (inactive nodes are never written, CC3 3.3)
//   header.y  word index, into `words`, where the curve payload region starts
//   header.z  ABI version, currently 2 (CC4 4.2 added the LUT kinds)
//   header.w  reserved, 0
//
// `words` begins at byte 16, so `words[0]` is the buffer's word 4 and node
// record `i` occupies `words[i * 16 .. i * 16 + 16]`:
//
//   [kind, payload_word_offset, bypass, reserved, v0 .. v11]
//
// `kind` and `payload_word_offset` are stored as f32 and read with
// `u32(round(w))`; `bypass` is 0.0 or 1.0.  Every stored word offset is an
// index into `words`, which is what `curve_eval` consumes directly.
struct GradeBuffer {
    header: vec4<u32>,
    words: array<f32>,
};

// Node record stride, in words.
const GRADE_NODE_STRIDE: u32 = 16u;
// Offset of `v0` inside a node record, in words.
const GRADE_NODE_VALUES: u32 = 4u;
// `[count, x0, y0, m0, ... x15, y15, m15]` for one curve.
const GRADE_CURVE_SLOT_WORDS: u32 = 49u;

@group(0) @binding(0) var layer_texture: texture_2d<f32>;
@group(0) @binding(1) var layer_sampler: sampler;
@group(0) @binding(2) var<uniform> params: LayerParams;
@group(0) @binding(3) var lut_texture: texture_3d<f32>;
@group(0) @binding(4) var<storage, read> grade_buffer: GradeBuffer;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vertex_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let positions = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0,  1.0),
    );
    let uvs = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
    );
    var output: VertexOutput;
    let translated = positions[vertex_index] * params.scale
        + vec2<f32>(params.offset_x, -params.offset_y);
    output.position = vec4<f32>(translated, 0.0, 1.0);
    output.uv = uvs[vertex_index];
    return output;
}

fn sample_external_lut(color: vec3<f32>) -> vec3<f32> {
    let domain_min = vec3<f32>(
        params.external_domain_min_r,
        params.external_domain_min_g,
        params.external_domain_min_b,
    );
    let domain_max = vec3<f32>(
        params.external_domain_max_r,
        params.external_domain_max_g,
        params.external_domain_max_b,
    );
    let normalized = clamp(
        (color - domain_min) / max(domain_max - domain_min, vec3<f32>(0.000001)),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    // CC4 4.1: the legacy stage now lives in slot 4 of the shared atlas, so
    // its edge length and depth origin come from `LayerParams` instead of
    // `textureDimensions`.  The trilinear evaluation below is unchanged.
    let origin = i32(params.external_lut_z_origin);
    let maximum = vec3<i32>(i32(params.external_lut_size)) - vec3<i32>(1);
    let scaled = normalized * vec3<f32>(maximum);
    let low = vec3<i32>(floor(scaled));
    let high = min(low + vec3<i32>(1), maximum);
    let fraction = fract(scaled);
    let low_slice = origin + low.z;
    let high_slice = origin + high.z;
    let c000 = textureLoad(lut_texture, vec3<i32>(low.x, low.y, low_slice), 0).rgb;
    let c100 = textureLoad(lut_texture, vec3<i32>(high.x, low.y, low_slice), 0).rgb;
    let c010 = textureLoad(lut_texture, vec3<i32>(low.x, high.y, low_slice), 0).rgb;
    let c110 = textureLoad(lut_texture, vec3<i32>(high.x, high.y, low_slice), 0).rgb;
    let c001 = textureLoad(lut_texture, vec3<i32>(low.x, low.y, high_slice), 0).rgb;
    let c101 = textureLoad(lut_texture, vec3<i32>(high.x, low.y, high_slice), 0).rgb;
    let c011 = textureLoad(lut_texture, vec3<i32>(low.x, high.y, high_slice), 0).rgb;
    let c111 = textureLoad(lut_texture, vec3<i32>(high.x, high.y, high_slice), 0).rgb;
    let low_z = mix(mix(c000, c100, fraction.x), mix(c010, c110, fraction.x), fraction.y);
    let high_z = mix(mix(c001, c101, fraction.x), mix(c011, c111, fraction.x), fraction.y);
    return mix(low_z, high_z, fraction.z);
}

fn decode_bt709(value: f32) -> f32 {
    if value < 0.081 {
        return value / 4.5;
    }
    return pow((value + 0.099) / 1.099, 1.0 / 0.45);
}

fn encode_bt709(value: f32) -> f32 {
    let sign = select(1.0, -1.0, value < 0.0);
    let magnitude = abs(value);
    if magnitude < 0.018 {
        return sign * 4.5 * magnitude;
    }
    return sign * (1.099 * pow(magnitude, 0.45) - 0.099);
}

// CC4 3.4: the exact sign-preserving inverse of `encode_bt709`, with CC1's
// rounded constants.  It is a NODE-INTERNAL grading parameterization: CC1's
// `decode_bt709` is a SOURCE decode that takes the linear branch for every
// negative argument, so it is not an inverse below -0.018 and must not be
// substituted here.  Neither function may be used as a monitoring or delivery
// transform; that still happens once, after compositing.
//
// `sign` is derived with `select` rather than `sign()` so that `sgn(0) = 0`
// falls out of the zero magnitude, exactly as `encode_bt709` already does.
fn decode_display709(value: f32) -> f32 {
    let sign = select(1.0, -1.0, value < 0.0);
    let magnitude = abs(value);
    if magnitude < 0.081 {
        return sign * magnitude / 4.5;
    }
    return sign * pow((magnitude + 0.099) / 1.099, 1.0 / 0.45);
}

fn smooth_weight(start: f32, end: f32, value: f32) -> f32 {
    return smoothstep(start, end, value);
}

// CC1 managed primary correction.  `values` is the word index of `v0` inside
// the node record; the arithmetic below is unchanged from the CC1 node loop,
// so a primary-only stack renders bit-identically across the CC3 ABI change.
fn apply_primary_node(linear_rgb: vec3<f32>, values: u32) -> vec3<f32> {
    let exposure = grade_buffer.words[values];
    let temperature = grade_buffer.words[values + 1u];
    let tint = grade_buffer.words[values + 2u];
    let contrast = grade_buffer.words[values + 3u];
    let pivot_value = grade_buffer.words[values + 4u];
    let blacks = grade_buffer.words[values + 5u];
    let shadows = grade_buffer.words[values + 6u];
    let highlights = grade_buffer.words[values + 7u];
    let whites = grade_buffer.words[values + 8u];
    let saturation = grade_buffer.words[values + 9u];
    let red_gain = 1.0 + 0.1 * temperature;
    let green_gain = 1.0 - 0.1 * tint;
    let blue_gain = 1.0 - 0.1 * temperature;
    let exposure_gain = exp2(exposure);
    var corrected = vec3<f32>(
        linear_rgb.r * red_gain * exposure_gain,
        linear_rgb.g * green_gain * exposure_gain,
        linear_rgb.b * blue_gain * exposure_gain,
    );
    for (var channel = 0u; channel < 3u; channel++) {
        let value = corrected[channel];
        let bounded = clamp(value, 0.0, 1.0);
        let black_weight = 1.0 - smooth_weight(0.0, 0.25, bounded);
        let shadow_weight = 1.0 - smooth_weight(0.15, 0.50, bounded);
        let highlight_weight = smooth_weight(0.50, 0.85, bounded);
        let white_weight = smooth_weight(0.75, 1.0, bounded);
        corrected[channel] = value
            + 0.25 * blacks * black_weight
            + 0.20 * shadows * shadow_weight
            + 0.20 * highlights * highlight_weight
            + 0.25 * whites * white_weight;
    }
    let pivot = pivot_value;
    let contrast_scale = 1.0 + contrast;
    corrected = vec3<f32>(
        pivot + (corrected.r - pivot) * contrast_scale,
        pivot + (corrected.g - pivot) * contrast_scale,
        pivot + (corrected.b - pivot) * contrast_scale,
    );
    let luminance = dot(corrected, vec3<f32>(0.2126, 0.7152, 0.0722));
    let saturation_scale = 1.0 + saturation;
    return vec3<f32>(
        luminance + (corrected.r - luminance) * saturation_scale,
        luminance + (corrected.g - luminance) * saturation_scale,
        luminance + (corrected.b - luminance) * saturation_scale,
    );
}

// CC3 2.1: the grading encoding.  Copied verbatim from the CC3 contract; it
// is an exact analytic bijection on all of R and is deliberately NOT CC1's
// rounded `encode_bt709`/`decode_bt709` monitor pair.
fn grade709_encode(v: f32) -> f32 {
    let s = sign(v); let a = abs(v);
    if a < 0.018053969 { return s * 4.5 * a; }
    return s * (1.0992968 * pow(a, 0.45) - 0.0992968);
}

fn grade709_decode(v: f32) -> f32 {
    let s = sign(v); let a = abs(v);
    if a < 0.08124286 { return s * a / 4.5; }
    return s * pow((a + 0.0992968) / 1.0992968, 2.2222223);
}

// CC3 2.3: monotone cubic Hermite evaluation with linear extrapolation from
// the host-limited end tangents.  `base` is the word index of one curve slot.
fn curve_eval(base: u32, x: f32) -> f32 {
    let count = u32(round(grade_buffer.words[base]));
    if count < 2u { return x; }
    let first = base + 1u;
    let last  = base + 1u + (count - 1u) * 3u;
    if x <  grade_buffer.words[first] {
        return grade_buffer.words[first + 1u]
             + grade_buffer.words[first + 2u] * (x - grade_buffer.words[first]);
    }
    if x >= grade_buffer.words[last] {
        return grade_buffer.words[last + 1u]
             + grade_buffer.words[last + 2u] * (x - grade_buffer.words[last]);
    }
    var segment = 0u;
    for (var i = 0u; i + 1u < count; i = i + 1u) {
        let xi = grade_buffer.words[base + 1u + i * 3u];
        let xn = grade_buffer.words[base + 1u + (i + 1u) * 3u];
        if x >= xi && x < xn { segment = i; }
    }
    let o0 = base + 1u + segment * 3u;
    let o1 = o0 + 3u;
    let x0 = grade_buffer.words[o0];      let y0 = grade_buffer.words[o0 + 1u];
    let m0 = grade_buffer.words[o0 + 2u]; let x1 = grade_buffer.words[o1];
    let y1 = grade_buffer.words[o1 + 1u]; let m1 = grade_buffer.words[o1 + 2u];
    let h = x1 - x0;
    let t = (x - x0) / h;
    let t2 = t * t; let t3 = t2 * t;
    return (2.0*t3 - 3.0*t2 + 1.0) * y0
         + (t3 - 2.0*t2 + t) * h * m0
         + (-2.0*t3 + 3.0*t2) * y1
         + (t3 - t2) * h * m1;
}

// CC3 2.2: ASC CDL slope/offset/power evaluated per channel in `grade709`.
// `base` is the word index of `v0`: v0..v2 slope, v3..v5 offset, v6..v8 power.
//
// The odd extension `sgn(y) * |y| ^ p` replaces ASC CDL's `[0, 1]` clamp so
// recoverable undershoot and over-range highlights survive the node.  `pow`
// requires a non-negative base, so the magnitude is raised and the sign is
// restored afterwards; WGSL `sign(0.0)` is 0, which is exactly `sgn(0) = 0`.
fn apply_wheels_node(base: u32, rgb: vec3<f32>) -> vec3<f32> {
    var result = rgb;
    for (var channel = 0u; channel < 3u; channel++) {
        let slope = grade_buffer.words[base + channel];
        let offset = grade_buffer.words[base + 3u + channel];
        let power = grade_buffer.words[base + 6u + channel];
        let y = grade709_encode(result[channel]) * slope + offset;
        result[channel] = grade709_decode(sign(y) * pow(abs(y), power));
    }
    return result;
}

// CC3 2.3: per-channel curves first, then the master curve applied
// identically to all three channels, all inside `grade709`.  `payload_base` is
// the word index of the node's red slot; the four slots are ordered red,
// green, blue, master.
fn apply_curves_node(payload_base: u32, rgb: vec3<f32>) -> vec3<f32> {
    let master = payload_base + 3u * GRADE_CURVE_SLOT_WORDS;
    var encoded = vec3<f32>(
        grade709_encode(rgb.r),
        grade709_encode(rgb.g),
        grade709_encode(rgb.b),
    );
    for (var channel = 0u; channel < 3u; channel++) {
        let shaped = curve_eval(payload_base + channel * GRADE_CURVE_SLOT_WORDS, encoded[channel]);
        encoded[channel] = curve_eval(master, shaped);
    }
    return vec3<f32>(
        grade709_decode(encoded.r),
        grade709_decode(encoded.g),
        grade709_decode(encoded.b),
    );
}

// CC4 4.1/4.3: one texel of one atlas slot.  Slot `k` occupies
// `z in [z_origin, z_origin + S)` of the shared depth-packed 3D texture at
// binding 3, addressed `x = r`, `y = g`, `z = b`, IRIDAS red-fastest, which is
// exactly the order the parser stores and the host uploads.
//
// `textureLoad` ONLY: hardware filtering is forbidden here (CC4 3.5), and the
// sampler at binding 1 is never used for the atlas.
fn lut_fetch(size: u32, z_origin: u32, index: vec3<u32>) -> vec3<f32> {
    let clamped = min(index, vec3<u32>(size - 1u));
    return textureLoad(
        lut_texture,
        vec3<i32>(i32(clamped.x), i32(clamped.y), i32(z_origin + clamped.z)),
        0,
    ).rgb;
}

// CC4 3.5: tetrahedral interpolation with the contract's EXACT branch
// structure.  All six formulas agree analytically on the shared faces, so the
// tie handling is well defined; the fixed structure is what removes the f32
// association difference that would otherwise let the CPU reference and this
// shader disagree by a ULP at a tie.  No epsilon guard is added.
fn lut_tetrahedral(
    size: u32,
    z_origin: u32,
    dmin: vec3<f32>,
    dmax: vec3<f32>,
    e: vec3<f32>,
) -> vec3<f32> {
    let u = clamp(e, dmin, dmax);
    let t = (u - dmin) / (dmax - dmin);
    let s = t * f32(size - 1u);
    // `u` is clamped into the domain, so `s` is never negative and the u32
    // conversion below is exactly the contract's `min(u32(floor(s)), S - 2)`.
    let i = min(vec3<u32>(floor(s)), vec3<u32>(size - 2u));
    let f = s - vec3<f32>(f32(i.x), f32(i.y), f32(i.z));
    let c000 = lut_fetch(size, z_origin, i + vec3<u32>(0u, 0u, 0u));
    let c100 = lut_fetch(size, z_origin, i + vec3<u32>(1u, 0u, 0u));
    let c010 = lut_fetch(size, z_origin, i + vec3<u32>(0u, 1u, 0u));
    let c110 = lut_fetch(size, z_origin, i + vec3<u32>(1u, 1u, 0u));
    let c001 = lut_fetch(size, z_origin, i + vec3<u32>(0u, 0u, 1u));
    let c101 = lut_fetch(size, z_origin, i + vec3<u32>(1u, 0u, 1u));
    let c011 = lut_fetch(size, z_origin, i + vec3<u32>(0u, 1u, 1u));
    let c111 = lut_fetch(size, z_origin, i + vec3<u32>(1u, 1u, 1u));
    if f.x > f.y {
        if f.y > f.z {
            return c000 + f.x * (c100 - c000) + f.y * (c110 - c100) + f.z * (c111 - c110);
        } else if f.x > f.z {
            return c000 + f.x * (c100 - c000) + f.y * (c111 - c101) + f.z * (c101 - c100);
        } else {
            return c000 + f.x * (c101 - c001) + f.y * (c111 - c101) + f.z * (c001 - c000);
        }
    } else {
        if f.z > f.y {
            return c000 + f.x * (c111 - c011) + f.y * (c011 - c001) + f.z * (c001 - c000);
        } else if f.z > f.x {
            return c000 + f.x * (c111 - c011) + f.y * (c010 - c000) + f.z * (c011 - c010);
        } else {
            return c000 + f.x * (c110 - c010) + f.y * (c010 - c000) + f.z * (c111 - c110);
        }
    }
}

// CC4 3.5: one `technical_lut` / `creative_look` node.  `values` is the word
// index of `v0`:
//
//   v0  atlas slot, 0..=3      v1  mix, 0..=1        v2  input encoding token
//   v3..v5  domain_min rgb     v6..v8  domain_max rgb
//   v9  lattice edge S         v10 atlas z origin    v11 reserved
//
//   e = ENC(x); u = clamp(e, dmin, dmax); y = tetrahedral(u);
//   z = y + (e - u);           <- out-of-domain delta restoration
//   x' = DEC(z); out = x + (x' - x) * mix   <- mix in LINEAR light
//
// The excursion is restored in the ENCODED domain and the mix happens in
// linear light, so an over-range highlight stays recoverable and `mix = 0` and
// `mix = 1` are the exact endpoints.  No RGB clamp survives this function.
fn apply_lut_node(values: u32, rgb: vec3<f32>) -> vec3<f32> {
    let mix_amount = grade_buffer.words[values + 1u];
    let encoding = u32(round(grade_buffer.words[values + 2u]));
    let dmin = vec3<f32>(
        grade_buffer.words[values + 3u],
        grade_buffer.words[values + 4u],
        grade_buffer.words[values + 5u],
    );
    let dmax = vec3<f32>(
        grade_buffer.words[values + 6u],
        grade_buffer.words[values + 7u],
        grade_buffer.words[values + 8u],
    );
    let size = u32(round(grade_buffer.words[values + 9u]));
    let z_origin = u32(round(grade_buffer.words[values + 10u]));
    var e = rgb;
    if encoding == 0u {
        e = vec3<f32>(encode_bt709(rgb.r), encode_bt709(rgb.g), encode_bt709(rgb.b));
    } else if encoding == 2u {
        e = vec3<f32>(grade709_encode(rgb.r), grade709_encode(rgb.g), grade709_encode(rgb.b));
    }
    let u = clamp(e, dmin, dmax);
    let y = lut_tetrahedral(size, z_origin, dmin, dmax, e);
    let z = y + (e - u);
    var decoded = z;
    if encoding == 0u {
        decoded = vec3<f32>(
            decode_display709(z.r),
            decode_display709(z.g),
            decode_display709(z.b),
        );
    } else if encoding == 2u {
        decoded = vec3<f32>(grade709_decode(z.r), grade709_decode(z.g), grade709_decode(z.b));
    }
    return rgb + (decoded - rgb) * mix_amount;
}

// CC3 3.1: one ordered node stack executed in serialized `clip.effects` order.
// The renderer must not flatten, reorder, or merge nodes, and no RGB clamp
// occurs between them.  Inactive nodes are never written to the buffer
// (CC3 3.3); the `bypass` word is still honoured so a stale buffer cannot
// silently apply a bypassed node.
fn apply_color_nodes(input_rgb: vec3<f32>) -> vec3<f32> {
    var corrected = input_rgb;
    for (var index = 0u; index < grade_buffer.header.x; index++) {
        let base = index * GRADE_NODE_STRIDE;
        if grade_buffer.words[base + 2u] >= 1.0 {
            continue;
        }
        let kind = u32(round(grade_buffer.words[base]));
        let values = base + GRADE_NODE_VALUES;
        if kind == 1u {
            corrected = apply_primary_node(corrected, values);
        } else if kind == 2u {
            corrected = apply_wheels_node(values, corrected);
        } else if kind == 3u {
            corrected = apply_curves_node(u32(round(grade_buffer.words[base + 1u])), corrected);
        } else if kind == 4u {
            // CC4 4.2: `technical_lut`.
            corrected = apply_lut_node(values, corrected);
        } else if kind == 5u {
            // CC4 4.2: `creative_look`.  The two kinds are mathematically
            // identical and differ only in stage, role, and mix bounds, all of
            // which the host resolved before the record was written.
            corrected = apply_lut_node(values, corrected);
        }
    }
    return corrected;
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    var sample_uv = input.uv;
    if params.reframe_aspect > 0.0 {
        let dimensions = vec2<f32>(textureDimensions(layer_texture));
        let source_aspect = dimensions.x / max(dimensions.y, 1.0);
        if source_aspect > params.reframe_aspect {
            let visible_width = params.reframe_aspect / source_aspect;
            let left = clamp(
                params.reframe_focus_x - visible_width * 0.5,
                0.0,
                1.0 - visible_width,
            );
            sample_uv.x = left + input.uv.x * visible_width;
        } else if source_aspect < params.reframe_aspect {
            let visible_height = source_aspect / params.reframe_aspect;
            let top = clamp(
                params.reframe_focus_y - visible_height * 0.5,
                0.0,
                1.0 - visible_height,
            );
            sample_uv.y = top + input.uv.y * visible_height;
        }
    }
    let sampled = textureSample(layer_texture, layer_sampler, sample_uv);
    var linear_rgb = vec3<f32>(
        decode_bt709(sampled.r),
        decode_bt709(sampled.g),
        decode_bt709(sampled.b),
    );
    if params.input_linear > 0.5 {
        linear_rgb = sampled.rgb;
    }
    if grade_buffer.header.x > 0u {
        linear_rgb = apply_color_nodes(linear_rgb);
    }
    var output_linear = linear_rgb;
    var alpha = clamp(sampled.a * params.opacity, 0.0, 1.0);
    if params.legacy_stage_active > 0.5 {
        var rgb = vec3<f32>(
            encode_bt709(linear_rgb.r),
            encode_bt709(linear_rgb.g),
            encode_bt709(linear_rgb.b),
        );
        // `color_grade` is canonicalised to `primary_correction` before an
        // effect reaches live project state, so its display-coded exposure,
        // temperature, and tint block is unreachable and was removed. The
        // `exposure`, `temperature`, and `tint` uniform slots are retained so
        // the 48-float `LayerParams` ABI stays byte-identical.
        rgb += vec3<f32>(params.brightness);
        rgb = (rgb - vec3<f32>(0.5)) * params.contrast + vec3<f32>(0.5);
        let luminance = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        rgb = mix(vec3<f32>(luminance), rgb, params.saturation);
        let pre_lut = rgb;
        let preset = u32(round(params.lut_preset));
        if preset == 1u {
            rgb = (rgb - vec3<f32>(0.5)) * 1.08 + vec3<f32>(0.54, 0.50, 0.46);
        } else if preset == 2u {
            rgb = (rgb - vec3<f32>(0.5)) * 1.12 + vec3<f32>(0.46, 0.50, 0.55);
        } else if preset == 3u {
            let mono = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
            rgb = vec3<f32>(mono);
        } else if preset == 4u {
            let bleach_luma = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
            rgb = mix(vec3<f32>(bleach_luma), rgb, 0.35);
            rgb = (rgb - vec3<f32>(0.5)) * 1.35 + vec3<f32>(0.5);
        }
        rgb = mix(pre_lut, rgb, params.lut_intensity);
        if params.external_lut_enabled > 0.5 {
            let lut_rgb = sample_external_lut(rgb);
            rgb = mix(rgb, lut_rgb, params.external_lut_intensity);
        }
        // Legacy display compatibility retains its established display-space
        // clamp. It is outside the managed primary sequence.
        rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
        output_linear = vec3<f32>(
            decode_bt709(rgb.r),
            decode_bt709(rgb.g),
            decode_bt709(rgb.b),
        );
    }
    // CC1 2.2.4: alpha and keying are independent of colour correction, and
    // 2.2.5: no colour stage clamps RGB.  The key distance therefore uses a
    // LOCAL display-coded copy of the working value, while the colour that
    // continues down the pipeline is never clamped.  This runs whether or not
    // a legacy display stage is active, and after it so a legacy + key stack
    // keeps its established behaviour.
    if params.key_threshold >= 0.0 {
        // Only the distance uses the clamped copy: the key colour is a
        // display-coded 0..1 triple, so comparing against an over-range value
        // would report a distance the operator cannot reason about.
        let key_rgb = clamp(
            vec3<f32>(
                encode_bt709(output_linear.r),
                encode_bt709(output_linear.g),
                encode_bt709(output_linear.b),
            ),
            vec3<f32>(0.0),
            vec3<f32>(1.0),
        );
        let key_color = vec3<f32>(params.key_red, params.key_green, params.key_blue);
        let distance = length(key_rgb - key_color) / 1.7320508;
        let key_alpha = smoothstep(
            max(0.0, params.key_threshold - params.key_softness),
            min(1.0, params.key_threshold + params.key_softness + 0.00001),
            distance,
        );
        alpha *= key_alpha;
        // Spill suppression keeps its established DISPLAY-CODED strength:
        // moving the dominance to linear light would silently restrengthen
        // every keyed project that was graded before CC1.  The encode here is
        // the sign-preserving, UNCLAMPED one, so no value is crushed.
        //
        // A fully kept pixel (`key_alpha == 1`) has no suppression to apply,
        // so it skips the round trip entirely and stays bit-identical; a fully
        // keyed pixel is already alpha 0.  Only edge pixels pay the transfer
        // round trip, and they get exactly the pre-CC1 amount.
        if key_alpha < 1.0 {
            let spill_r = encode_bt709(output_linear.r);
            let spill_g = encode_bt709(output_linear.g);
            let spill_b = encode_bt709(output_linear.b);
            let key_dominance = max(0.0, spill_g - max(spill_r, spill_b));
            let suppressed = spill_g - key_dominance * params.key_spill * (1.0 - key_alpha);
            output_linear.g = decode_bt709(suppressed);
        }
    }
    output_linear = mix(output_linear, vec3<f32>(params.fade_white), params.fade_mix);
    if params.fade_mix > 0.0 {
        alpha = 1.0;
    }
    if input.uv.x < params.crop_left
        || input.uv.x > 1.0 - params.crop_right
        || input.uv.y < params.crop_top
        || input.uv.y > 1.0 - params.crop_bottom {
        alpha = 0.0;
    }
    if params.mask_shape > 0.5 {
        let half_size = max(
            vec2<f32>(params.mask_width, params.mask_height) * 0.5,
            vec2<f32>(0.005),
        );
        let normalized = abs(input.uv - vec2<f32>(params.mask_center_x, params.mask_center_y))
            / half_size;
        var distance = max(normalized.x, normalized.y);
        if params.mask_shape > 1.5 {
            distance = length(normalized);
        }
        let feather = params.mask_feather * 0.5;
        var mask_alpha = 1.0 - smoothstep(max(0.0, 1.0 - feather), 1.0, distance);
        if feather <= 0.00001 {
            mask_alpha = select(0.0, 1.0, distance <= 1.0);
        }
        if params.mask_invert > 0.5 {
            mask_alpha = 1.0 - mask_alpha;
        }
        alpha *= mask_alpha;
    }
    return vec4<f32>(output_linear, alpha);
}
