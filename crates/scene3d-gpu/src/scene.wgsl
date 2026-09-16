// The 3D shader for `gfx.scene3d` on the GPU.
//
// Deliberately matches what the CPU rasterizer does, so the two backends agree
// on the same scene and an image diff is a real test rather than a comparison
// of two different renderers. Where the CPU path has a choice recorded in a
// comment -- one-sided lighting when normals are supplied, the ambient floor,
// the y flip into screen space -- this makes the same one.
//
// What it does NOT yet do is anything the CPU path cannot: no shadows, no
// second light, no specular. Those come in G4, once parity is proven, because
// a backend that looks better AND different is a backend whose differences
// cannot be told from its bugs.

struct Camera {
    // Row-major view-projection. Built on the CPU so both backends share one
    // camera derivation and cannot drift on the basis that has already been
    // got wrong once (`up x forward`, not `forward x up`).
    view_proj: mat4x4<f32>,
    // Direction the light travels, normalised.
    light: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tint: vec4<f32>,
    // Whether this vertex's triangle carries real normals. A mesh drawn
    // through `triangles` or `textured` has none, and its shading has to use
    // the two-sided face term the CPU path uses, or the two backends disagree
    // on every closed mesh whose winding puts normals inward.
    @location(4) flags: u32,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) tint: vec4<f32>,
    @location(3) @interpolate(flat) flags: u32,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip = camera.view_proj * vec4<f32>(in.position, 1.0);
    out.normal = in.normal;
    out.uv = in.uv;
    out.tint = in.tint;
    out.flags = in.flags;
    return out;
}

// Bit 0 of `flags`: the vertex carries a real normal rather than a face normal.
const FLAG_SMOOTH: u32 = 1u;

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let facing = dot(n, camera.light.xyz);

    var shade: f32;
    if ((in.flags & FLAG_SMOOTH) != 0u) {
        // One-sided, matching `Scene::smooth`: an author who supplied normals
        // has said which way is out, so the dark side can be dark.
        shade = 0.22 + 0.78 * max(-facing, 0.0);
    } else {
        // Two-sided, matching `Scene::triangle`: a closed mesh's winding
        // decides which way its face normals point and half come out inward,
        // so the term is absolute and the dark side is lit like the light one.
        shade = 0.35 + 0.65 * abs(facing);
    }

    let sampled = textureSample(atlas, atlas_sampler, in.uv);
    // The tint multiplies the sample and the shade scales the result, with the
    // clamp AFTER the multiply -- which is what lets an app pass a tint over
    // 1.0 to defeat the ambient floor, as the HUD in the racing game does.
    let rgb = clamp(sampled.rgb * in.tint.rgb * shade, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(rgb, sampled.a * in.tint.a);
}
