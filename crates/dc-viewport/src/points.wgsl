// DentalCraft viewport: point clouds as instanced screen-space quads (the
// size is in pixels on every backend; WebGPU points are fixed at 1 px). One
// instance per point, six vertices per quad, round sprites. Output is linear
// colour written to an sRGB render target.

struct CameraU {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    // x: 1 = orthographic
    params: vec4<f32>,
    wire_color: vec4<f32>,
    back_color: vec4<f32>,
    hover_color: vec4<f32>,
};

struct PointsU {
    // linear colour, multiplied with the per-point colour
    color: vec4<f32>,
    // x: sprite diameter (pixels), y/z: target size (pixels),
    // w: 1 = per-point normals present
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> cam: CameraU;
@group(1) @binding(0) var<uniform> pts: PointsU;

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) view_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) corner: vec2<f32>,
};

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@vertex
fn vs_points(
    @builtin(vertex_index) vi: u32,
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec4<f32>,
    @location(2) col: vec4<f32>,
) -> VOut {
    var quad = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    let corner = quad[vi];
    let vp = cam.view * vec4<f32>(pos, 1.0);
    var clip = cam.proj * vp;
    // half the diameter in pixels, as a clip-space offset
    let half_ndc = corner * pts.params.x / pts.params.yz;
    clip = vec4<f32>(clip.xy + half_ndc * clip.w, clip.zw);
    var out: VOut;
    out.clip = clip;
    out.view_pos = vp.xyz;
    // the view matrix is a rigid transform: its upper 3x3 turns the normal
    out.normal = (cam.view * vec4<f32>(nrm.xyz, 0.0)).xyz;
    out.color = vec4<f32>(srgb_to_linear(col.rgb), col.a) * pts.color;
    out.corner = corner;
    return out;
}

@fragment
fn fs_points(in: VOut) -> @location(0) vec4<f32> {
    if dot(in.corner, in.corner) > 1.0 {
        discard;
    }
    var rgb = in.color.rgb;
    if pts.params.w > 0.5 {
        var v = vec3<f32>(0.0, 0.0, 1.0);
        if cam.params.x < 0.5 {
            v = normalize(-in.view_pos);
        }
        var n = normalize(in.normal);
        // a cloud has no front or back: light the side facing the eye
        if dot(n, v) < 0.0 {
            n = -n;
        }
        let ndl = max(dot(n, v), 0.0);
        let sky = vec3<f32>(0.42, 0.44, 0.48);
        let ground = vec3<f32>(0.20, 0.18, 0.17);
        let ambient = mix(ground, sky, n.y * 0.5 + 0.5);
        rgb = rgb * (ambient + vec3<f32>(0.72 * ndl)) + vec3<f32>(pow(ndl, 48.0) * 0.12);
    } else {
        rgb = rgb * 0.95;
    }
    return vec4<f32>(rgb, in.color.a);
}
