// DentalCraft viewport: Blinn-Phong with a headlight and a soft hemisphere
// ambient. Output is linear colour written to an sRGB render target.

const PALETTE_SIZE: u32 = 32u;

struct CameraU {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    // x: 1 = orthographic, y: wireframe depth offset (clip w fraction)
    params: vec4<f32>,
    wire_color: vec4<f32>,
    back_color: vec4<f32>,
    // tint of the mesh under the pointer (linear rgb, a = strength)
    hover_color: vec4<f32>,
};

struct MeshU {
    model: mat4x4<f32>,
    // inverse transpose of view * model (upper 3x3 used)
    normal_mat: mat4x4<f32>,
    color: vec4<f32>,
    // x: colour mode (0 uniform, 1 per-vertex RGBA8 sRGB, 2 label palette)
    // y: backface mode (0 same, 1 tint, 2 cull)
    // z: opacity, w: palette colour count (labels 1..)
    params: vec4<f32>,
    // x: 1 = under the pointer (hover tint)
    extra: vec4<f32>,
    // [0] = unlabeled colour, [1..] = label colours (linear)
    palette: array<vec4<f32>, PALETTE_SIZE>,
};

@group(0) @binding(0) var<uniform> cam: CameraU;
@group(1) @binding(0) var<uniform> mesh: MeshU;

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) view_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
};

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn vertex_color(attr: u32) -> vec4<f32> {
    let mode = u32(mesh.params.x);
    if mode == 1u {
        let c = unpack4x8unorm(attr);
        return vec4<f32>(srgb_to_linear(c.rgb), c.a) * mesh.color;
    }
    if mode == 2u {
        let n = max(u32(mesh.params.w), 1u);
        var idx = 0u;
        if attr != 0u {
            idx = 1u + (attr - 1u) % n;
        }
        return mesh.palette[min(idx, PALETTE_SIZE - 1u)] * vec4<f32>(1.0, 1.0, 1.0, mesh.color.a);
    }
    return mesh.color;
}

@vertex
fn vs_main(
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) attr: u32,
) -> VOut {
    var out: VOut;
    let world = mesh.model * vec4<f32>(pos, 1.0);
    let vp = cam.view * world;
    out.clip = cam.proj * vp;
    out.view_pos = vp.xyz;
    out.normal = (mesh.normal_mat * vec4<f32>(nrm, 0.0)).xyz;
    out.color = vertex_color(attr);
    return out;
}

@fragment
fn fs_main(in: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let backface = u32(mesh.params.y);
    if !front && backface == 2u {
        discard;
    }
    var n = normalize(in.normal);
    if !front {
        n = -n;
    }
    var v = vec3<f32>(0.0, 0.0, 1.0);
    if cam.params.x < 0.5 {
        v = normalize(-in.view_pos);
    }
    // headlight: the light comes from the eye, so L = V = H
    let ndl = max(dot(n, v), 0.0);
    let spec = pow(ndl, 48.0) * 0.18;
    let sky = vec3<f32>(0.42, 0.44, 0.48);
    let ground = vec3<f32>(0.20, 0.18, 0.17);
    let ambient = mix(ground, sky, n.y * 0.5 + 0.5);
    var base = in.color.rgb;
    if !front && backface == 1u {
        base = mix(base, cam.back_color.rgb, cam.back_color.a);
    }
    var rgb = base * (ambient + vec3<f32>(0.72 * ndl)) + vec3<f32>(spec);
    rgb = mix(rgb, cam.hover_color.rgb, cam.hover_color.a * mesh.extra.x);
    return vec4<f32>(rgb, in.color.a * mesh.params.z);
}

@vertex
fn vs_wire(
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) attr: u32,
) -> @builtin(position) vec4<f32> {
    let vp = cam.view * mesh.model * vec4<f32>(pos, 1.0);
    var clip = cam.proj * vp;
    // pull the lines slightly towards the camera so they win the depth test
    clip.z = clip.z - cam.params.y * clip.w;
    return clip;
}

@fragment
fn fs_wire() -> @location(0) vec4<f32> {
    return vec4<f32>(cam.wire_color.rgb, cam.wire_color.a * mesh.params.z);
}

// Selection mask: the selected mesh again, in a red/green target. The
// "visible" pipeline tests the depth of the scene (red: where the mesh is the
// front surface), the "full" one does not (green: its whole silhouette).
@vertex
fn vs_mask(@location(0) pos: vec3<f32>) -> @builtin(position) vec4<f32> {
    let world = mesh.model * vec4<f32>(pos, 1.0);
    let vp = cam.view * world;
    var clip = cam.proj * vp;
    // a hair towards the camera, so the front surface passes its own depth
    clip.z = clip.z - cam.params.y * 0.05 * clip.w;
    return clip;
}

@fragment
fn fs_mask() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 1.0, 0.0, 1.0);
}
