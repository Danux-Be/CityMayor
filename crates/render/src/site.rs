//! Engins et installations de chantier, modélisés dans Blender
//! (`tools/blender/make_site_props.py`) : grue à tour (mât empilable + tête),
//! pelleteuse, bungalow. La couleur de sommet porte la teinte ; la surface
//! utilise la couche de peinture unie (`Layer::Marking`).

use std::sync::OnceLock;

use crate::urban::{Layer, UrbanMesh};

/// Maillage d'un engin (Y vers le haut, pied à l'origine).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PropModel {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

impl PropModel {
    /// Lit un `.glb` à couleur par sommet.
    ///
    /// # Errors
    /// Fichier invalide ou attributs manquants.
    pub fn from_glb(bytes: &[u8]) -> Result<Self, String> {
        let gltf = gltf::Gltf::from_slice(bytes).map_err(|e| e.to_string())?;
        let blob = gltf.blob.as_deref();
        let mesh = gltf.meshes().next().ok_or("aucun maillage")?;
        let primitive = mesh.primitives().next().ok_or("aucune primitive")?;
        let reader = primitive.reader(|buffer| match buffer.source() {
            gltf::buffer::Source::Bin => blob,
            gltf::buffer::Source::Uri(_) => None,
        });
        let positions: Vec<[f32; 3]> = reader.read_positions().ok_or("positions absentes")?.collect();
        let normals: Vec<[f32; 3]> = reader.read_normals().ok_or("normales absentes")?.collect();
        let colors: Vec<[f32; 4]> = reader.read_colors(0).ok_or("couleurs absentes")?.into_rgba_f32().collect();
        let indices: Vec<u32> =
            reader.read_indices().map_or_else(|| (0..positions.len() as u32).collect(), |i| i.into_u32().collect());
        if normals.len() != positions.len() || colors.len() != positions.len() {
            return Err("attributs de longueurs différentes".into());
        }
        Ok(Self { positions, normals, colors, indices })
    }

    /// Hauteur du modèle (m).
    #[must_use]
    pub fn height(&self) -> f32 {
        self.positions.iter().map(|p| p[1]).fold(0.0, f32::max)
    }
}

/// Engins embarqués.
#[derive(Clone, Debug, PartialEq)]
pub struct SiteModels {
    /// Élément de mât de grue, 3 m.
    pub crane_mast: PropModel,
    /// Tête de grue (origine au sommet du mât).
    pub crane_top: PropModel,
    pub excavator: PropModel,
    pub cabin: PropModel,
}

/// Hauteur d'un élément de mât (m).
pub const CRANE_MAST_SECTION_M: f32 = 3.0;

impl SiteModels {
    /// Modèles embarqués dans le binaire, chargés une fois.
    ///
    /// # Panics
    /// Si un `.glb` embarqué est illisible (erreur de build).
    #[must_use]
    pub fn get() -> &'static Self {
        static MODELS: OnceLock<SiteModels> = OnceLock::new();
        MODELS.get_or_init(|| {
            let load = |bytes: &[u8], name: &str| PropModel::from_glb(bytes).unwrap_or_else(|e| panic!("{name} : {e}"));
            Self {
                crane_mast: load(include_bytes!("../assets/site/grue_mat.glb"), "grue_mat"),
                crane_top: load(include_bytes!("../assets/site/grue_tete.glb"), "grue_tete"),
                excavator: load(include_bytes!("../assets/site/pelleteuse.glb"), "pelleteuse"),
                cabin: load(include_bytes!("../assets/site/bungalow.glb"), "bungalow"),
            }
        })
    }
}

/// Ajoute un engin tourné de `yaw` (radians, autour de Y) et posé en `origin`.
pub fn push_prop(mesh: &mut UrbanMesh, model: &PropModel, origin: [f32; 3], yaw: f32) {
    let (sin, cos) = yaw.sin_cos();
    let base = mesh.positions.len() as u32;
    let alpha = Layer::Marking.alpha();
    for ((p, n), c) in model.positions.iter().zip(&model.normals).zip(&model.colors) {
        let (x, z) = (p[0] * cos - p[2] * sin, p[0] * sin + p[2] * cos);
        let world = [origin[0] + x, origin[1] + p[1], origin[2] + z];
        mesh.positions.push(world);
        mesh.normals.push([n[0] * cos - n[2] * sin, n[1], n[0] * sin + n[2] * cos]);
        // Peinture unie projetée : seul son grain compte.
        mesh.uvs.push([(world[0] + world[2]) * 0.5, world[1] * 0.5]);
        // La couche de peinture est presque blanche : on éclaircit la teinte
        // pour retrouver la couleur modélisée.
        mesh.colors.push([c[0] * 1.15, c[1] * 1.15, c[2] * 1.15, alpha]);
    }
    mesh.indices.extend(model.indices.iter().map(|i| i + base));
}

/// Grue à tour : mât de `height` mètres (arrondi aux éléments), tête
/// orientée de `yaw`.
pub fn push_crane(mesh: &mut UrbanMesh, origin: [f32; 3], height: f32, yaw: f32) {
    let models = SiteModels::get();
    let sections = (height / CRANE_MAST_SECTION_M).ceil().max(2.0) as u32;
    for k in 0..sections {
        let y = origin[1] + k as f32 * CRANE_MAST_SECTION_M;
        push_prop(mesh, &models.crane_mast, [origin[0], y, origin[2]], 0.0);
    }
    let top = origin[1] + sections as f32 * CRANE_MAST_SECTION_M;
    push_prop(mesh, &models.crane_top, [origin[0], top, origin[2]], yaw);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_models_load_within_budget() {
        let m = SiteModels::get();
        for (name, model, max_tris) in [
            ("mât", &m.crane_mast, 400),
            ("tête", &m.crane_top, 3000),
            ("pelleteuse", &m.excavator, 600),
            ("bungalow", &m.cabin, 300),
        ] {
            let tris = model.indices.len() / 3;
            assert!(tris > 10 && tris <= max_tris, "{name} : {tris} triangles");
            assert!(model.positions.iter().flatten().all(|v| v.is_finite()));
        }
        // Le mât fait un élément de haut ; la tête porte une flèche de ~30 m.
        assert!((m.crane_mast.height() - CRANE_MAST_SECTION_M).abs() < 0.2);
        let reach = m.crane_top.positions.iter().map(|p| p[0].abs().max(p[2].abs())).fold(0.0, f32::max);
        assert!(reach > 28.0, "flèche de {reach} m");
    }

    #[test]
    fn cranes_stack_mast_sections() {
        let mut mesh = UrbanMesh::default();
        push_crane(&mut mesh, [10.0, 5.0, 10.0], 20.0, 0.7);
        assert!(mesh.is_sane());
        let top = mesh.positions.iter().map(|p| p[1]).fold(0.0, f32::max);
        assert!(top > 5.0 + 20.0 + 5.0, "sommet {top}");
    }
}
