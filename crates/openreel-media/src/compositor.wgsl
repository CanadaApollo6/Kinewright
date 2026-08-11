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
    _padding_0: f32,
    _padding_1: f32,
    _padding_2: f32,
};

@group(0) @binding(0) var layer_texture: texture_2d<f32>;
@group(0) @binding(1) var layer_sampler: sampler;
@group(0) @binding(2) var<uniform> params: LayerParams;

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

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let sampled = textureSample(layer_texture, layer_sampler, input.uv);
    var rgb = sampled.rgb + vec3<f32>(params.brightness);
    rgb = (rgb - vec3<f32>(0.5)) * params.contrast + vec3<f32>(0.5);
    let luminance = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    rgb = mix(vec3<f32>(luminance), rgb, params.saturation);
    rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let fade_color = vec3<f32>(params.fade_white);
    rgb = mix(rgb, fade_color, params.fade_mix);
    var alpha = clamp(sampled.a * params.opacity, 0.0, 1.0);
    if params.fade_mix > 0.0 {
        alpha = 1.0;
    }
    return vec4<f32>(rgb, alpha);
}
