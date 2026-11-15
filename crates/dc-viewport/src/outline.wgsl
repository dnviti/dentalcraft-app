// DentalCraft viewport: the selection outline, composited over the resolved
// 3D image. The mask holds, in red, where the selected mesh is the front
// surface and, in green, its whole silhouette (both antialiased). The outline
// is a band of `radius` pixels around them: solid around what is seen, at
// `hidden` strength around the parts other meshes cover.

struct OutlineU {
    // linear rgb, a = opacity
    color: vec4<f32>,
    // x: radius (pixels), y: strength around the hidden parts
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: OutlineU;
@group(0) @binding(1) var mask: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // one triangle over the whole target
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

@fragment
fn fs_outline(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(textureDimensions(mask));
    let p = vec2<i32>(pos.xy);
    let here = textureLoad(mask, p, 0).rg;
    let r = u.params.x;
    let ri = i32(ceil(r));
    var near = vec2<f32>(0.0);
    for (var dy = -ri; dy <= ri; dy++) {
        for (var dx = -ri; dx <= ri; dx++) {
            // a soft rim at the outer edge of the band
            let w = clamp(r + 0.5 - length(vec2<f32>(f32(dx), f32(dy))), 0.0, 1.0);
            if w > 0.0 {
                let q = clamp(p + vec2<i32>(dx, dy), vec2<i32>(0), dims - vec2<i32>(1));
                near = max(near, textureLoad(mask, q, 0).rg * w);
            }
        }
    }
    let seen = clamp(near.x - here.x, 0.0, 1.0);
    let hidden = clamp(near.y - here.y, 0.0, 1.0) * u.params.y;
    return vec4<f32>(u.color.rgb, max(seen, hidden) * u.color.a);
}
