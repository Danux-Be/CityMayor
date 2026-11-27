// Sol : mélange de six textures photographiques (Poly Haven, CC0).
//
// Poids par sommet (calculés dans `nature::splat_weights`) :
//   couleur RGBA = prairie, herbe sèche, sous-bois, roche
//   uv           = sable, neige
// Le mélange est « par hauteur » (la texture la plus contrastée gagne aux
// transitions), la roche est projetée en triplanaire (pas d'étirement sur les
// falaises), et chaque couche est échantillonnée à deux échelles qui se
// fondent avec la distance pour casser la répétition.

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    mesh_view_bindings::view,
};

struct TerrainParams {
    // Taille d'une répétition (m) des couches 0..3 et 4..5 ; scales_b.z :
    // détail (0 = une lecture par couche, sans triplanaire : préréglage Bas).
    scales_a: vec4<f32>,
    scales_b: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var layers: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var layers_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var<uniform> params: TerrainParams;

// Échantillon à deux échelles : détail proche + variation lointaine.
fn sample_layer(layer: i32, uv: vec2<f32>, far_mix: f32) -> vec3<f32> {
    let near = textureSample(layers, layers_sampler, uv, layer).rgb;
    // Préréglage Bas : une seule lecture (branche uniforme : même valeur
    // pour tous les pixels, les dérivées restent valides).
    if (params.scales_b.z < 0.5) {
        return near;
    }
    let far = textureSample(layers, layers_sampler, uv * 0.173 + vec2<f32>(0.31, 0.67), layer).rgb;
    return mix(mix(near, far, 0.3), far, far_mix);
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let p = in.world_position.xyz;
    let n = normalize(in.world_normal);
    let distance = length(view.world_position.xyz - p);
    let far_mix = smoothstep(120.0, 1400.0, distance);

    // Six couches, mêmes appels quel que soit le pixel (flux uniforme,
    // obligatoire pour les dérivées implicites de textureSample).
    let c0 = sample_layer(0, p.xz / params.scales_a.x, far_mix);
    let c1 = sample_layer(1, p.xz / params.scales_a.y, far_mix);
    // Sous-bois : la photo est un tapis de feuilles mortes orangé ; sous une
    // canopée, vu d'avion, le sol est sombre et verdâtre.
    let c2 = sample_layer(2, p.xz / params.scales_a.z, far_mix) * vec3<f32>(0.45, 0.58, 0.36);
    let c4 = sample_layer(4, p.xz / params.scales_b.x, far_mix);
    let c5 = sample_layer(5, p.xz / params.scales_b.y, far_mix);

    // Roche triplanaire : sur une falaise, la projection verticale étire la
    // texture ; on mélange les trois projections selon la normale.
    let rock_scale = params.scales_a.w;
    var c3 = sample_layer(3, p.xz / rock_scale, far_mix);
    if (params.scales_b.z >= 0.5) {
        let tri = pow(abs(n), vec3<f32>(4.0));
        let tri_w = tri / (tri.x + tri.y + tri.z);
        let r_x = sample_layer(3, p.zy / rock_scale, far_mix);
        let r_z = sample_layer(3, p.xy / rock_scale, far_mix);
        c3 = r_x * tri_w.x + c3 * tri_w.y + r_z * tri_w.z;
    }

    // Mélange par hauteur : chaque poids est modulé par la luminance de sa
    // texture, puis accentué. Les transitions suivent les cailloux, les
    // touffes d'herbe et les plaques de neige au lieu d'un fondu flou.
    let w_in = array<f32, 6>(in.color.r, in.color.g, in.color.b, in.color.a, in.uv.x, in.uv.y);
    let colors = array<vec3<f32>, 6>(c0, c1, c2, c3, c4, c5);
    var weights = array<f32, 6>(0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    var total = 0.0;
    for (var i = 0; i < 6; i++) {
        let w = max(w_in[i], 0.0) * (0.35 + luminance(colors[i]));
        let sharp = w * w * w;
        weights[i] = sharp;
        total += sharp;
    }
    var albedo = vec3<f32>(0.0);
    for (var i = 0; i < 6; i++) {
        albedo += colors[i] * (weights[i] / max(total, 1e-5));
    }

    // Variation de très basse fréquence (prairies plus ou moins vertes),
    // indispensable vue d'avion pour éviter un tapis uniforme.
    let macro_tone = textureSample(layers, layers_sampler, p.xz / 3100.0, 0).g;
    albedo *= mix(0.82, 1.12, macro_tone);

    // Berges humides : le sol s'assombrit près de l'eau.
    let wet = 1.0 - smoothstep(-0.2, 0.9, p.y);
    albedo *= mix(1.0, 0.62, wet);

    pbr_input.material.base_color = vec4<f32>(albedo, 1.0);
    // La neige et la roche mouillée brillent un peu, l'herbe pas du tout.
    let snow = weights[5] / max(total, 1e-5);
    pbr_input.material.perceptual_roughness = mix(mix(0.93, 0.62, snow), 0.45, wet * 0.6);
    pbr_input.material.reflectance = vec3<f32>(mix(0.25, 0.45, snow));

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
