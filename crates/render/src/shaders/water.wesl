// Eau : un plan au niveau de la mer dont la normale est animée par une somme
// d'ondes. Le soleil y dessine des reflets mobiles ; au loin, les ondes
// s'estompent (sinon elles scintillent en sous-échantillonnage).

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    mesh_view_bindings::{globals, view},
};

struct WaterParams {
    // x : amplitude des vagues, y : vitesse, z : distance d'atténuation (m).
    waves: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> params: WaterParams;

fn wave(p: vec2<f32>, dir: vec2<f32>, frequency: f32, speed: f32, t: f32) -> vec2<f32> {
    let d = normalize(dir);
    let phase = dot(d, p) * frequency + t * speed;
    // Dérivée de sin(phase) : pente de la surface dans la direction de l'onde.
    return d * cos(phase) * frequency;
}

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let p = in.world_position.xz;
    let t = globals.time * params.waves.y;
    var slope = vec2<f32>(0.0);
    slope += wave(p, vec2<f32>(1.0, 0.3), 0.21, 1.3, t) * 0.50;
    slope += wave(p, vec2<f32>(-0.4, 1.0), 0.37, 1.7, t) * 0.30;
    slope += wave(p, vec2<f32>(0.8, -0.9), 0.73, 2.3, t) * 0.15;
    slope += wave(p, vec2<f32>(-1.0, -0.2), 1.31, 3.1, t) * 0.08;

    let distance = length(view.world_position.xz - p);
    let fade = 1.0 / (1.0 + distance / params.waves.z);
    slope *= params.waves.x * fade;

    let normal = normalize(vec3<f32>(-slope.x, 1.0, -slope.y));
    pbr_input.N = normal;
    pbr_input.world_normal = normal;

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
