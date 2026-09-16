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
    // The camera basis, as three rows plus the eye. The projection is applied
    // in the shader rather than baked into a matrix because the CPU path does
    // not use a standard perspective matrix: it divides by camera-space z and
    // uses that z DIRECTLY as depth, with no near/far remap. Reproducing that
    // exactly is what makes an image diff between the backends meaningful.
    right: vec4<f32>,
    up: vec4<f32>,
    forward: vec4<f32>,
    eye: vec4<f32>,
    // x: tan(fov/2), y: aspect, z: 1/far for the depth mapping, w: unused.
    lens: vec4<f32>,
    // Direction the light travels, normalised.
    light: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var atlas: texture_2d_array<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tint: vec4<f32>,
    // Bit 0: this vertex carries a real normal rather than a face normal.
    // Bits 8..: the texture layer to sample.
    @location(4) flags: u32,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) tint: vec4<f32>,
    // Split into two FLOATS rather than carried as one flat u32.
    //
    // A `u32` varying must be `@interpolate(flat)`, and flat takes its value
    // from the provoking vertex -- which is correct in principle and did not
    // survive this backend in practice: every triangle sampled the LAST
    // layer uploaded rather than its own. Floats interpolate across a
    // triangle whose three corners hold the same value, so they arrive
    // unchanged, and rounding at the far end recovers the integer.
    @location(3) smooth_flag: f32,
    @location(4) layer: f32,
};

const FLAG_SMOOTH: u32 = 1u;

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;

    // Into camera space, the same three dot products the CPU path takes.
    let rel = in.position - camera.eye.xyz;
    let cx = dot(rel, camera.right.xyz);
    let cy = dot(rel, camera.up.xyz);
    let cz = dot(rel, camera.forward.xyz);

    let half_fov = camera.lens.x;
    let aspect = camera.lens.y;
    let inv_far = camera.lens.z;

    // The CPU path divides by camera z and flips y for the screen. Clip space
    // wants the divide done by w, so x and y are scaled and w carries z --
    // which gives the same perspective AND makes the interpolation
    // perspective-correct, which the CPU path has to do by hand.
    let x = cx / (half_fov * aspect);
    let y = cy / half_fov;

    // Depth: camera z normalised to 0..1, because a depth buffer needs a
    // bounded range where the CPU path could just compare raw z. Multiplied
    // by w here since the hardware divides by it.
    let depth = clamp(cz * inv_far, 0.0, 1.0) * cz;

    // A point at or behind the eye has no meaningful projection; the CPU path
    // drops the whole triangle. Pushing w to a tiny positive number keeps the
    // vertex out of view without a divide by zero.
    let w = max(cz, 1e-6);
    out.clip = vec4<f32>(x, y, depth, w);

    out.normal = in.normal;
    out.uv = in.uv;
    out.tint = in.tint;
    out.smooth_flag = f32(in.flags & FLAG_SMOOTH);
    out.layer = f32(in.flags >> 8u);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let facing = dot(n, camera.light.xyz);

    var shade: f32;
    if (in.smooth_flag > 0.5) {
        // One-sided, matching `Scene::smooth`: an author who supplied normals
        // has said which way is out, so the dark side can be dark.
        shade = 0.22 + 0.78 * max(-facing, 0.0);
    } else {
        // Two-sided, matching `Scene::triangle`: a closed mesh's winding
        // decides which way its face normals point and half come out inward,
        // so the term is absolute and the dark side is lit like the light one.
        shade = 0.35 + 0.65 * abs(facing);
    }

    let layer = i32(round(in.layer));
    let sampled = textureSample(atlas, atlas_sampler, in.uv, layer);
    // The TINT is clamped to 0..1 before it multiplies anything, and the
    // product is clamped again. Both, in that order, because that is what
    // `shade_sample` does on the CPU side -- clamping only the product lets a
    // tint above 1 brighten a surface, and the showcase's sky dome passes 2.6
    // deliberately. On the CPU that 2.6 becomes 1.0 and the sky keeps its
    // gradient; on the GPU, before this, it saturated the whole dome to white.
    let tint = clamp(in.tint.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let rgb = clamp(sampled.rgb * tint * shade, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(rgb, sampled.a * clamp(in.tint.a, 0.0, 1.0));
}
