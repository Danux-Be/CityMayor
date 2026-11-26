// Ville : façades, toits, chaussées et trottoirs.
//
// Une seule texture-tableau (`urban::LAYER_NAMES`, rendue par Blender). Par
// sommet : uv = coordonnées en répétitions de la couche, couleur RGB = teinte
// (variété des enduits, bardages, tuiles), alpha = indice de couche / 32.

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
};

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var layers: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var layers_sampler: sampler;

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let layer = i32(round(in.color.a * 32.0));
    let texel = textureSample(layers, layers_sampler, in.uv, layer).rgb;
    let albedo = texel * in.color.rgb;
    pbr_input.material.base_color = vec4<f32>(albedo, 1.0);

    // Les vitres (sombres et bleutées) brillent ; le reste est mat.
    let glassy = smoothstep(0.06, 0.0, abs(texel.b - texel.r - 0.05)) * smoothstep(0.35, 0.1, texel.g);
    pbr_input.material.perceptual_roughness = mix(0.85, 0.25, glassy);
    pbr_input.material.reflectance = vec3<f32>(mix(0.3, 0.6, glassy));

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
