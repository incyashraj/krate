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
    // x: ambient floor for face-normal shading, y: the floor for smooth
    // shading, z: specular strength, w: shininess.
    surface: vec4<f32>,
    // rgb: fog colour, a: density. Zero density is off.
    fog: vec4<f32>,
    // Direction the fill light travels, normalised. w is 1 when a fill light
    // is set at all, so the shader can skip it rather than adding black.
    fill_dir: vec4<f32>,
    fill_color: vec4<f32>,
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
    // Whether this triangle wears a real texture. The two CPU paths clamp the
    // tint DIFFERENTLY and the difference is load-bearing, so the GPU has to
    // know which it is reproducing -- see the fragment shader.
    @location(5) textured_flag: f32,
    // World position, so the fragment can work out which way the eye is and
    // put a highlight where one would fall. Specular is the difference
    // between a surface that looks like plastic and one that looks like
    // paper, and it cannot be computed without knowing where you are
    // standing.
    @location(6) world: vec3<f32>,
    // Distance from the eye, for fog.
    @location(7) view_depth: f32,
};

const FLAG_SMOOTH: u32 = 1u;
const FLAG_TEXTURED: u32 = 2u;

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
    out.textured_flag = f32((in.flags & FLAG_TEXTURED) >> 1u);
    out.layer = f32(in.flags >> 8u);
    out.world = in.position;
    out.view_depth = cz;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let facing = dot(n, camera.light.xyz);

    // The ambient floor. `surface.x` and `surface.y` carry the defaults --
    // 0.35 for face-normal shading, 0.22 for smooth -- unless the app has set
    // its own, so a scene that never calls `set-lighting` shades exactly as
    // the software rasterizer does.
    var floor_term: f32;
    var lambert: f32;
    if (in.smooth_flag > 0.5) {
        // One-sided, matching `Scene::smooth`: an author who supplied normals
        // has said which way is out, so the dark side can be dark.
        floor_term = camera.surface.y;
        lambert = max(-facing, 0.0);
    } else {
        // Two-sided, matching `Scene::triangle`: a closed mesh's winding
        // decides which way its face normals point and half come out inward,
        // so the term is absolute and the dark side is lit like the light one.
        floor_term = camera.surface.x;
        lambert = abs(facing);
    }
    var shade = floor_term + (1.0 - floor_term) * lambert;

    // A second directional light, standing in for light bounced off
    // everything else. A key light alone leaves the shadow side black; a key
    // and a fill is most of what reads as a lit room rather than a lamp in a
    // void. Added to the shade rather than replacing it, and only when the
    // app has actually set one.
    var fill_rgb = vec3<f32>(0.0);
    if (camera.fill_dir.w > 0.5) {
        let fill_facing = dot(n, camera.fill_dir.xyz);
        let fill_lambert = select(abs(fill_facing), max(-fill_facing, 0.0), in.smooth_flag > 0.5);
        fill_rgb = camera.fill_color.rgb * fill_lambert;
    }

    let layer = i32(round(in.layer));
    let sampled = textureSample(atlas, atlas_sampler, in.uv, layer);

    // The two CPU paths clamp the tint differently, and the difference is
    // load-bearing rather than an oversight to tidy away:
    //
    // - `pack_shaded`, for an untextured triangle, computes
    //   `(tint * shade).clamp(0,1)` -- the tint is NOT clamped first, so a
    //   tint above 1 really does brighten. The racing game's HUD passes 2.7
    //   to climb out of the 0.35 ambient floor and is white because of it.
    // - `shade_sample`, for a textured one, clamps the tint to 0..1 BEFORE
    //   multiplying, so a texture cannot be over-brightened. The showcase's
    //   sky dome passes 2.6 and stays a gradient because of that.
    //
    // Clamping both ways broke the HUD; clamping neither blew out the sky. So
    // the flag says which, and each is reproduced exactly.
    var base: vec3<f32>;
    if (in.textured_flag > 0.5) {
        let tint = clamp(in.tint.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
        base = sampled.rgb * tint * shade;
    } else {
        base = in.tint.rgb * shade;
    }
    base = base + base * fill_rgb;

    // Specular: a Blinn-Phong highlight where the eye would see the light
    // reflected. Without it every surface in a scene is chalk, however well
    // it is lit -- it is the single cheapest thing that makes a rendered
    // object look like a material rather than a coloured shape.
    //
    // Off by default (`surface.z` is zero), so a scene that never asks is
    // pixel-identical to the software rasterizer.
    let specular_strength = camera.surface.z;
    if (specular_strength > 0.0) {
        let to_eye = normalize(camera.eye.xyz - in.world);
        // The light TRAVELS along `camera.light`, so the direction toward it
        // is the negative. Getting this backwards puts the highlight on the
        // dark side, which reads as a rendering bug rather than a sign error.
        let to_light = -camera.light.xyz;
        let halfway = normalize(to_eye + to_light);
        let spec = pow(max(dot(n, halfway), 0.0), max(camera.surface.w, 1.0));
        base = base + vec3<f32>(spec * specular_strength);
    }

    // Fog last, because it is what the air does to everything in front of it,
    // including the highlight.
    let density = camera.fog.a;
    if (density > 0.0) {
        // Exponential-squared: distance fades slowly at first and then
        // quickly, which is what atmosphere actually does and what stops the
        // near field looking hazy.
        let f = in.view_depth * density;
        let fog_amount = clamp(1.0 - exp(-f * f), 0.0, 1.0);
        base = mix(base, camera.fog.rgb, fog_amount);
    }

    let rgb = clamp(base, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(rgb, sampled.a * clamp(in.tint.a, 0.0, 1.0));
}
