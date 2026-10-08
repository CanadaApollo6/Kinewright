@group(0) @binding(0) var input: texture_2d<f32>;
@group(0) @binding(1) var output: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<storage, read> table: array<u32>;
fn byte_at(i: u32) -> u32 { return (table[i / 4u] >> (8u * (i % 4u))) & 255u; }
@compute @workgroup_size(8, 8)
fn encode(@builtin(global_invocation_id) p: vec3<u32>) {
    if any(p.xy >= textureDimensions(input)) { return; }
    let v = textureLoad(input, p.xy, 0);
    let rg = pack2x16float(v.rg);
    let ba = pack2x16float(v.ba);
    let a = byte_at(65536u + (ba >> 16u));
    let r = byte_at(rg & 65535u);
    let g = byte_at(rg >> 16u);
    let b = byte_at(ba & 65535u);
    let base = 131072u + a * 256u;
    textureStore(output, p.xy, vec4<f32>(f32(byte_at(base + r)),
        f32(byte_at(base + g)), f32(byte_at(base + b)), f32(a)) / 255.0);
}
