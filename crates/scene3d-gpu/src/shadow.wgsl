// The shadow pass: the scene drawn from the light's point of view, depth only.
//
// A shadow map is the whole trick behind cast shadows: render what the light
// can see, keep how far away it was, and in the main pass ask whether the
// point being shaded is further from the light than whatever the light hit
// first. If it is, something is in the way.
//
// Depth only, so there is no fragment shader and the pass is roughly free
// compared with the main one -- no texturing, no lighting, no blending.

struct LightCamera {
    // World to light clip space. Orthographic, because a directional light has
    // no position: its rays are parallel and a perspective divide would bend
    // them.
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> light: LightCamera;

@vertex
fn vs_main(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return light.view_proj * vec4<f32>(position, 1.0);
}
