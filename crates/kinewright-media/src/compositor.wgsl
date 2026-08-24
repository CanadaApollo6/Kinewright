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
    _uniform_padding: vec2<f32>,
};

struct PrimaryNode {
    values: array<vec4<f32>, 3>,
};

struct PrimaryBuffer {
    // Keep the host ABI explicit: the count occupies the first u32 of a
    // 16-byte header, and the node array begins immediately at byte 16.
    header: vec4<u32>,
    nodes: array<PrimaryNode>,
};

@group(0) @binding(0) var layer_texture: texture_2d<f32>;
@group(0) @binding(1) var layer_sampler: sampler;
@group(0) @binding(2) var<uniform> params: LayerParams;
@group(0) @binding(3) var lut_texture: texture_3d<f32>;
@group(0) @binding(4) var<storage, read> primary_buffer: PrimaryBuffer;

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
    let maximum = vec3<i32>(textureDimensions(lut_texture)) - vec3<i32>(1);
    let scaled = normalized * vec3<f32>(maximum);
    let low = vec3<i32>(floor(scaled));
    let high = min(low + vec3<i32>(1), maximum);
    let fraction = fract(scaled);
    let c000 = textureLoad(lut_texture, vec3<i32>(low.x, low.y, low.z), 0).rgb;
    let c100 = textureLoad(lut_texture, vec3<i32>(high.x, low.y, low.z), 0).rgb;
    let c010 = textureLoad(lut_texture, vec3<i32>(low.x, high.y, low.z), 0).rgb;
    let c110 = textureLoad(lut_texture, vec3<i32>(high.x, high.y, low.z), 0).rgb;
    let c001 = textureLoad(lut_texture, vec3<i32>(low.x, low.y, high.z), 0).rgb;
    let c101 = textureLoad(lut_texture, vec3<i32>(high.x, low.y, high.z), 0).rgb;
    let c011 = textureLoad(lut_texture, vec3<i32>(low.x, high.y, high.z), 0).rgb;
    let c111 = textureLoad(lut_texture, vec3<i32>(high.x, high.y, high.z), 0).rgb;
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

fn smooth_weight(start: f32, end: f32, value: f32) -> f32 {
    return smoothstep(start, end, value);
}

fn apply_primary_node(linear_rgb: vec3<f32>, node: PrimaryNode) -> vec3<f32> {
    let first = node.values[0];
    let second = node.values[1];
    let third = node.values[2];
    let temperature = first.y;
    let tint = first.z;
    let red_gain = 1.0 + 0.1 * temperature;
    let green_gain = 1.0 - 0.1 * tint;
    let blue_gain = 1.0 - 0.1 * temperature;
    let exposure_gain = exp2(first.x);
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
            + 0.25 * second.y * black_weight
            + 0.20 * second.z * shadow_weight
            + 0.20 * second.w * highlight_weight
            + 0.25 * third.x * white_weight;
    }
    let pivot = second.x;
    let contrast_scale = 1.0 + first.w;
    corrected = vec3<f32>(
        pivot + (corrected.r - pivot) * contrast_scale,
        pivot + (corrected.g - pivot) * contrast_scale,
        pivot + (corrected.b - pivot) * contrast_scale,
    );
    let luminance = dot(corrected, vec3<f32>(0.2126, 0.7152, 0.0722));
    let saturation_scale = 1.0 + third.y;
    return vec3<f32>(
        luminance + (corrected.r - luminance) * saturation_scale,
        luminance + (corrected.g - luminance) * saturation_scale,
        luminance + (corrected.b - luminance) * saturation_scale,
    );
}

fn apply_primary_nodes(input_rgb: vec3<f32>) -> vec3<f32> {
    var corrected = input_rgb;
    for (var index = 0u; index < primary_buffer.header.x; index++) {
        corrected = apply_primary_node(corrected, primary_buffer.nodes[index]);
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
    if primary_buffer.header.x > 0u {
        linear_rgb = apply_primary_nodes(linear_rgb);
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
