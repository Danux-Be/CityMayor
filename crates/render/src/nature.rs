//! Le paysage : sol texturé, forêts, eau, ciel, et les règles qui rendent la
//! carte « infinie » à l'œil tout en gardant le joueur près de sa ville.
//!
//! # Une carte sans bord visible
//! La simulation est bornée (la zone jouable). Le paysage, lui, est la même
//! fonction de relief (`sim_core::WorldGen`) échantillonnée bien au-delà :
//! `DECOR_EXTENT_M` autour de la zone jouable. Trois garde-fous rendent la fin
//! du décor invisible, et chacun est testé :
//! 1. la caméra ne peut pas s'éloigner (`CameraBounds`) ;
//! 2. le brouillard atmosphérique a la couleur de l'horizon et efface tout ce
//!    qui est plus loin que `FOG_VISIBILITY_M` ;
//! 3. le décor s'étend plus loin que ce que la caméra la plus excentrée peut
//!    voir avant que le brouillard ne soit total.
//!
//! # Aucun Bevy ici
//! Comme `scene.rs`, ce module produit des tableaux (positions, normales,
//! poids de textures) et se teste sans GPU. Les flottants sont légitimes : on
//! est du côté présentation, rien ne retourne dans la simulation.

use sim_core::{World, ZoneType};

/// Étendue du décor au-delà de la zone jouable, en mètres.
pub const DECOR_EXTENT_M: f32 = 24_000.0;
/// Distance de visibilité du brouillard (contraste résiduel ~2 %).
pub const FOG_VISIBILITY_M: f32 = 13_000.0;
/// Couleur de l'horizon : brouillard, ciel bas et fond d'écran sont la même
/// couleur, sinon la ligne d'horizon trahirait la fin du décor.
pub const HORIZON_COLOR: [f32; 3] = [0.72, 0.80, 0.88];
/// Couleur du zénith (haut du dôme de ciel).
pub const ZENITH_COLOR: [f32; 3] = [0.30, 0.50, 0.80];
/// Rayon du dôme de ciel (suit la caméra).
pub const SKY_RADIUS_M: f32 = 60_000.0;

/// Les six couches du sol, dans l'ordre du tableau de textures.
pub const LAYER_COUNT: usize = 6;
/// Taille d'une répétition de texture au sol, en mètres, par couche.
pub const LAYER_SCALE_M: [f32; LAYER_COUNT] = [55.0, 38.0, 16.0, 42.0, 22.0, 70.0];
/// Noms des couches (même ordre que les textures embarquées).
pub const LAYER_NAMES: [&str; LAYER_COUNT] = ["prairie", "herbe sèche", "sous-bois", "roche", "sable", "neige"];

// ------------------------------------------------------------- relief

/// Altitude du sol (mètres) n'importe où : bilinéaire sur les centres de
/// tuiles dans la zone jouable (exactement le maillage), relief généré
/// au-delà. Continue à la frontière, puisque la zone jouable est elle-même un
/// échantillonnage du générateur.
#[must_use]
pub fn ground_altitude_m(world: &World, x: f32, z: f32) -> f32 {
    let tile = world.config().tile_size_m as f32;
    let terrain = world.terrain();
    let (w, h) = (terrain.width(), terrain.height());
    let fx = x / tile - 0.5;
    let fz = z / tile - 0.5;
    let inside = fx >= 0.0 && fz >= 0.0 && fx <= (w - 1) as f32 && fz <= (h - 1) as f32;
    if !inside {
        if let Some(generator) = world.worldgen() {
            return generator.altitude_cm((x * 100.0) as i64, (z * 100.0) as i64) as f32 / 100.0;
        }
    }
    let fx = fx.clamp(0.0, (w - 1) as f32);
    let fz = fz.clamp(0.0, (h - 1) as f32);
    let (x0, z0) = (fx.floor() as u32, fz.floor() as u32);
    let (x1, z1) = ((x0 + 1).min(w - 1), (z0 + 1).min(h - 1));
    let (tx, tz) = (fx - x0 as f32, fz - z0 as f32);
    let at = |x: u32, z: u32| -> f32 {
        let grid = terrain.heights();
        grid.get_xy(x, z).map_or(0.0, |cm| *cm as f32 / 100.0)
    };
    let top = at(x0, z0) + (at(x1, z0) - at(x0, z0)) * tx;
    let bottom = at(x0, z1) + (at(x1, z1) - at(x0, z1)) * tx;
    top + (bottom - top) * tz
}

/// Densité de forêt `0..1` (0 sans générateur).
#[must_use]
pub fn forest_density(world: &World, x: f32, z: f32, altitude_m: f32) -> f32 {
    world.worldgen().map_or(0.0, |g| {
        g.forest_density_q16((x * 100.0) as i64, (z * 100.0) as i64, (altitude_m * 100.0) as i32) as f32
            / 65_536.0
    })
}

/// Taille de la zone jouable en mètres `(x, z)`.
#[must_use]
pub fn play_size_m(world: &World) -> (f32, f32) {
    let tile = world.config().tile_size_m as f32;
    (world.terrain().width() as f32 * tile, world.terrain().height() as f32 * tile)
}

// ------------------------------------------------------------- caméra

/// Contraintes de la caméra : on regarde la ville, pas le bout du monde.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraBounds {
    /// Rectangle (x, z) où le point visé peut se trouver.
    pub focus_min: [f32; 2],
    pub focus_max: [f32; 2],
    pub min_distance: f32,
    pub max_distance: f32,
    /// Inclinaison (radians au-dessus de l'horizon).
    pub min_pitch: f32,
    pub max_pitch: f32,
    /// Hauteur minimale de l'œil au-dessus du sol.
    pub ground_clearance: f32,
}

impl CameraBounds {
    /// Ramène un point visé dans le rectangle autorisé.
    #[must_use]
    pub fn clamp_focus(&self, x: f32, z: f32) -> (f32, f32) {
        (x.clamp(self.focus_min[0], self.focus_max[0]), z.clamp(self.focus_min[1], self.focus_max[1]))
    }

    /// Distance horizontale maximale entre l'œil et la zone jouable.
    #[must_use]
    pub fn max_eye_offset_from_play_area(&self, play: (f32, f32)) -> f32 {
        let margin_x = (-self.focus_min[0]).max(self.focus_max[0] - play.0);
        let margin_z = (-self.focus_min[1]).max(self.focus_max[1] - play.1);
        margin_x.hypot(margin_z) + self.max_distance
    }
}

/// Visibilité du brouillard selon l'inclinaison de la caméra.
///
/// Le brouillard sert à cacher la fin du décor, donc seulement quand
/// l'horizon est dans le champ. Caméra plongeante (le haut de l'image passe
/// sous l'horizon), on voit loin et net : la visibilité s'allonge, sans que
/// le bord du décor puisse entrer dans le champ (testé).
#[must_use]
pub fn fog_visibility_m(pitch: f32, vertical_fov: f32) -> f32 {
    let top_ray_below_horizon = pitch - vertical_fov * 0.5;
    FOG_VISIBILITY_M * (1.0 + 3.0 * smoothstep(0.22, 0.65, top_ray_below_horizon))
}

/// Champ de vision vertical de la caméra (radians), partagé avec le viewer.
pub const CAMERA_FOV: f32 = 0.78;

/// Bornes pour une carte donnée : le point visé peut déborder de 12 % de la
/// zone jouable (au moins 200 m), pas plus.
#[must_use]
pub fn camera_bounds(world: &World) -> CameraBounds {
    let (w, h) = play_size_m(world);
    let margin = (w.max(h) * 0.12).max(200.0);
    CameraBounds {
        focus_min: [-margin, -margin],
        focus_max: [w + margin, h + margin],
        min_distance: 25.0,
        max_distance: (w.max(h) * 1.4).clamp(800.0, 3_200.0),
        min_pitch: 0.10,
        max_pitch: 1.45,
        ground_clearance: 6.0,
    }
}

// ------------------------------------------------------------- sol

/// Un morceau de sol prêt à téléverser (un mesh par morceau : culling).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GroundChunk {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Poids des couches 0..4 (prairie, herbe sèche, sous-bois, roche).
    pub colors: Vec<[f32; 4]>,
    /// Poids des couches 4..6 (sable, neige).
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// Coordonnées d'un axe : un sommet par centre de tuile dans la zone jouable,
/// puis un pas qui grandit avec la distance (le brouillard et la perspective
/// rendent le détail lointain inutile).
#[must_use]
pub fn axis_coordinates(tiles: u32, tile: f32, extent: f32) -> Vec<f32> {
    let first = tile * 0.5;
    let last = (tiles as f32 - 0.5) * tile;
    let outward = |start: f32, sign: f32| -> Vec<f32> {
        let mut out = Vec::new();
        let (mut offset, mut step) = (0.0f32, tile);
        while offset < extent {
            let cap = if offset < 2_500.0 {
                40.0
            } else if offset < 9_000.0 {
                110.0
            } else {
                260.0
            };
            step = (step * 1.12).min(cap);
            offset += step;
            out.push(start + sign * offset);
        }
        out
    };
    let mut coords: Vec<f32> = outward(first, -1.0).into_iter().rev().collect();
    coords.extend((0..tiles).map(|i| first + i as f32 * tile));
    coords.extend(outward(last, 1.0));
    coords
}

/// Poids des six couches pour un point du sol.
#[must_use]
pub fn splat_weights(altitude_m: f32, normal_y: f32, forest: f32, x: f32, z: f32) -> [f32; LAYER_COUNT] {
    let slope = 1.0 - normal_y.clamp(0.0, 1.0); // 0 plat, ~0,29 à 45°
    let n1 = noise2(x * 0.004, z * 0.004);
    let n2 = noise2(x * 0.0011 + 17.0, z * 0.0011 - 5.0);

    let snow_line = 1_420.0 + n1 * 260.0;
    let snow = smoothstep(snow_line, snow_line + 160.0, altitude_m) * (1.0 - smoothstep(0.30, 0.50, slope));
    let rock = smoothstep(0.16, 0.34, slope).max(smoothstep(1_250.0, 1_650.0, altitude_m) * 0.75);
    // Sable : berges et fonds seulement (la ville borde le fleuve à 1–3 m).
    let sand = 1.0 - smoothstep(0.35, 1.2, altitude_m);
    let dry = (smoothstep(0.60, 0.85, n2) * 0.7 + smoothstep(300.0, 1_000.0, altitude_m) * 0.5).min(1.0);

    let mut rest = 1.0;
    let w_snow = snow;
    rest *= 1.0 - snow;
    let w_rock = rock * rest;
    rest *= 1.0 - rock;
    let w_sand = sand * rest;
    rest *= 1.0 - sand;
    let w_forest = forest * 0.9 * rest;
    rest -= w_forest;
    let w_dry = dry * rest;
    let w_lush = rest - w_dry;
    [w_lush.max(0.0), w_dry, w_forest, w_rock, w_sand, w_snow]
}

/// Sol complet (zone jouable + décor), découpé en morceaux de
/// `chunk_cells × chunk_cells` cellules.
#[must_use]
// Les indices servent aussi à lire la grille de hauteurs voisine (normales).
#[allow(clippy::needless_range_loop)]
pub fn ground_chunks(world: &World, chunk_cells: usize) -> Vec<GroundChunk> {
    let tile = world.config().tile_size_m as f32;
    let terrain = world.terrain();
    let xs = axis_coordinates(terrain.width(), tile, DECOR_EXTENT_M);
    let zs = axis_coordinates(terrain.height(), tile, DECOR_EXTENT_M);
    let (nx, nz) = (xs.len(), zs.len());

    let mut heights = vec![0.0f32; nx * nz];
    for (j, &z) in zs.iter().enumerate() {
        for (i, &x) in xs.iter().enumerate() {
            heights[j * nx + i] = ground_altitude_m(world, x, z);
        }
    }
    let height = |i: usize, j: usize| heights[j * nx + i];
    let normal = |i: usize, j: usize| -> [f32; 3] {
        let (i0, i1) = (i.saturating_sub(1), (i + 1).min(nx - 1));
        let (j0, j1) = (j.saturating_sub(1), (j + 1).min(nz - 1));
        let dx = (height(i1, j) - height(i0, j)) / (xs[i1] - xs[i0]).max(1e-3);
        let dz = (height(i, j1) - height(i, j0)) / (zs[j1] - zs[j0]).max(1e-3);
        let len = (dx * dx + dz * dz + 1.0).sqrt();
        [-dx / len, 1.0 / len, -dz / len]
    };

    let step = chunk_cells.max(1);
    let mut chunks = Vec::new();
    let mut j0 = 0;
    while j0 + 1 < nz {
        let j1 = (j0 + step).min(nz - 1);
        let mut i0 = 0;
        while i0 + 1 < nx {
            let i1 = (i0 + step).min(nx - 1);
            let mut chunk = GroundChunk::default();
            let width = i1 - i0 + 1;
            for j in j0..=j1 {
                for i in i0..=i1 {
                    let (x, z, y) = (xs[i], zs[j], height(i, j));
                    let n = normal(i, j);
                    let forest = forest_density(world, x, z, y) * smoothstep(0.86, 0.94, n[1]);
                    let w = splat_weights(y, n[1], forest, x, z);
                    chunk.positions.push([x, y, z]);
                    chunk.normals.push(n);
                    chunk.colors.push([w[0], w[1], w[2], w[3]]);
                    chunk.uvs.push([w[4], w[5]]);
                }
            }
            for j in 0..(j1 - j0) {
                for i in 0..(i1 - i0) {
                    let a = (j * width + i) as u32;
                    let b = a + 1;
                    let c = a + width as u32;
                    let d = c + 1;
                    chunk.indices.extend_from_slice(&[a, c, b, b, c, d]);
                }
            }
            chunks.push(chunk);
            i0 = i1;
        }
        j0 = j1;
    }
    chunks
}

// ------------------------------------------------------------- forêts

/// Essences d'arbres (modélisées dans Blender : `tools/blender/make_trees.py`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Species {
    Spruce,
    Pine,
    Oak,
    Birch,
}

impl Species {
    pub const ALL: [Self; 4] = [Self::Spruce, Self::Pine, Self::Oak, Self::Birch];

    const fn index(self) -> usize {
        match self {
            Self::Spruce => 0,
            Self::Pine => 1,
            Self::Oak => 2,
            Self::Birch => 3,
        }
    }

    const fn is_conifer(self) -> bool {
        matches!(self, Self::Spruce | Self::Pine)
    }
}

/// Niveau de détail d'un arbre.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeLod {
    /// Modèle complet (~400–750 triangles) : vu de près.
    Model,
    /// Modèle décimé (~45 triangles) : même silhouette, à mi-distance.
    Reduced,
    /// Silhouette procédurale (5–8 triangles) : jusqu'à l'horizon.
    Silhouette,
}

/// Géométrie d'un modèle d'arbre (mètres, pied à l'origine, Y vers le haut).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeModel {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

impl TreeModel {
    /// Lit la première primitive d'un `.glb` (positions, normales, `COLOR_0`).
    ///
    /// # Errors
    /// Fichier invalide ou attribut manquant.
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
        let colors: Vec<[f32; 4]> = reader
            .read_colors(0)
            .map_or_else(|| vec![[0.2, 0.35, 0.15, 1.0]; positions.len()], |c| c.into_rgba_f32().collect());
        let indices: Vec<u32> = reader
            .read_indices()
            .map_or_else(|| (0..positions.len() as u32).collect(), |i| i.into_u32().collect());
        if normals.len() != positions.len() || colors.len() != positions.len() {
            return Err("attributs de longueurs différentes".into());
        }
        Ok(Self { positions, normals, colors, indices })
    }
}

/// Les modèles des quatre essences, en deux niveaux de détail.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeModels {
    pub model: [TreeModel; 4],
    pub reduced: [TreeModel; 4],
}

impl TreeModels {
    /// Modèles embarqués dans le binaire (`assets/trees/*.glb`).
    ///
    /// # Panics
    /// Si un `.glb` embarqué est illisible (erreur de build, pas d'exécution).
    #[must_use]
    pub fn embedded() -> Self {
        let load = |bytes: &[u8], name: &str| {
            TreeModel::from_glb(bytes).unwrap_or_else(|e| panic!("arbre {name} illisible : {e}"))
        };
        Self {
            model: [
                load(include_bytes!("../assets/trees/epicea_lod0.glb"), "epicea"),
                load(include_bytes!("../assets/trees/pin_lod0.glb"), "pin"),
                load(include_bytes!("../assets/trees/chene_lod0.glb"), "chene"),
                load(include_bytes!("../assets/trees/bouleau_lod0.glb"), "bouleau"),
            ],
            reduced: [
                load(include_bytes!("../assets/trees/epicea_lod1.glb"), "epicea"),
                load(include_bytes!("../assets/trees/pin_lod1.glb"), "pin"),
                load(include_bytes!("../assets/trees/chene_lod1.glb"), "chene"),
                load(include_bytes!("../assets/trees/bouleau_lod1.glb"), "bouleau"),
            ],
        }
    }
}

/// Un arbre planté : où, quelle essence, quelle taille. Calculé une fois ;
/// la géométrie en est dérivée à chaque niveau de détail.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TreeSpot {
    pub base: [f32; 3],
    pub species: Species,
    pub scale: f32,
    /// Variation de teinte `0..1`.
    pub tint: f32,
    /// Rotation autour de la verticale (radians).
    pub angle: f32,
}

/// Morceau de forêt : géométrie fusionnée (positions, normales, couleurs).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeChunk {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub trees: usize,
}

/// Région de plantation (mètres, rectangle `x0..x1 × z0..z1`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Region {
    x0: f32,
    z0: f32,
    x1: f32,
    z1: f32,
}

/// Arbres du décor (hors zone jouable), groupés par cellule de `cell_m`
/// mètres. Plus denses près de la ville, plus clairsemés au loin.
#[must_use]
pub fn decor_tree_cells(world: &World, cell_m: f32) -> Vec<Vec<TreeSpot>> {
    let (w, h) = play_size_m(world);
    let reach = 5_000.0;
    let mut cells = Vec::new();
    let mut z = -reach;
    while z < h + reach {
        let mut x = -reach;
        while x < w + reach {
            let region = Region { x0: x, z0: z, x1: x + cell_m, z1: z + cell_m };
            let spots = plant(world, region, |px, pz| {
                // Hors zone jouable uniquement : la ville a sa propre forêt.
                if px >= 0.0 && pz >= 0.0 && px < w && pz < h {
                    return None;
                }
                let gap = (-px).max(px - w).max(-pz).max(pz - h).max(0.0);
                Some(if gap < 1_200.0 { 8.0 } else { 18.0 })
            });
            if !spots.is_empty() {
                cells.push(spots);
            }
            x += cell_m;
        }
        z += cell_m;
    }
    cells
}

/// Arbres possibles de la zone jouable, avec leur tuile, groupés par cellule
/// de `cell_m` mètres. Calculé une fois : à chaque changement de la ville, on
/// ne fait que filtrer (`city_trees`), sans réévaluer le relief.
#[must_use]
pub fn play_area_tree_cells(world: &World, cell_m: f32) -> Vec<Vec<(TreeSpot, sim_core::TileId)>> {
    let (w, h) = play_size_m(world);
    let tile = world.config().tile_size_m as f32;
    let grid = world.zoning().grid();
    let mut cells = Vec::new();
    let mut z = 0.0;
    while z < h {
        let mut x = 0.0;
        while x < w {
            let region = Region { x0: x, z0: z, x1: (x + cell_m).min(w), z1: (z + cell_m).min(h) };
            let spots = plant(world, region, |_, _| Some(8.0));
            let with_tiles: Vec<_> = spots
                .into_iter()
                .filter_map(|s| {
                    let id = grid.tile_at((s.base[0] / tile) as u32, (s.base[2] / tile) as u32)?;
                    Some((s, id))
                })
                .collect();
            cells.push(with_tiles);
            x += cell_m;
        }
        z += cell_m;
    }
    cells
}

/// Arbres d'une cellule de la ville qui ont encore leur place : tuile non
/// zonée, sans bâtiment, hors desserte routière (les arbres cèdent à la ville).
#[must_use]
pub fn city_trees(world: &World, cell: &[(TreeSpot, sim_core::TileId)]) -> Vec<TreeSpot> {
    let grid = world.zoning().grid();
    let access = world.road_access_view();
    cell.iter()
        .filter(|(_, id)| {
            grid.get(*id) == Some(&ZoneType::Unzoned)
                && world.buildings().at(*id).is_none()
                && !access.get(id.index()).copied().unwrap_or(false)
        })
        .map(|(spot, _)| *spot)
        .collect()
}

/// Géométrie fusionnée d'un ensemble d'arbres, relative à `origin`.
#[must_use]
pub fn build_trees(spots: &[TreeSpot], lod: TreeLod, models: &TreeModels, origin: [f32; 3]) -> TreeChunk {
    let mut chunk = TreeChunk::default();
    for spot in spots {
        let base = [spot.base[0] - origin[0], spot.base[1] - origin[1], spot.base[2] - origin[2]];
        match lod {
            TreeLod::Model => push_model(&mut chunk, &models.model[spot.species.index()], base, spot),
            TreeLod::Reduced => push_model(&mut chunk, &models.reduced[spot.species.index()], base, spot),
            TreeLod::Silhouette => push_silhouette(&mut chunk, base, spot),
        }
        chunk.trees += 1;
    }
    chunk
}

/// Distances de fondu `(apparition, disparition)` d'un niveau de détail, en
/// mètres de la caméra à l'origine de la cellule.
#[must_use]
pub const fn tree_lod_distances(lod: TreeLod) -> ((f32, f32), (f32, f32)) {
    match lod {
        TreeLod::Model => ((0.0, 0.0), (220.0, 350.0)),
        TreeLod::Reduced => ((220.0, 350.0), (1_150.0, 1_450.0)),
        TreeLod::Silhouette => ((1_150.0, 1_450.0), (7_000.0, 8_000.0)),
    }
}

/// Marge (m) entre la fin de visibilité d'un niveau et le moment où on le
/// construit : la cellule est prête avant d'entrer dans le champ.
pub const STREAM_MARGIN_M: f32 = 120.0;
/// Hystérésis (m) avant de libérer un niveau : pas de construction/libération
/// en boucle quand la caméra oscille autour d'une limite.
pub const STREAM_HYSTERESIS_M: f32 = 150.0;

/// Les niveaux « modèle » et « allégé » d'une cellule doivent-ils exister,
/// pour une caméra à `distance` mètres de son origine ? `built` : état actuel
/// (pour l'hystérésis). Les silhouettes, elles, existent toujours.
///
/// Pourquoi du streaming : construire le modèle complet pour tous les arbres
/// (300 000 dans le décor) demandait des Go et a fait tuer le terminal par
/// oomd. Seules les cellules proches de la caméra en ont besoin.
#[must_use]
pub fn streamed_lod_wanted(lod: TreeLod, distance: f32, built: bool) -> bool {
    let reach = tree_lod_distances(lod).1 .1 + STREAM_MARGIN_M;
    match lod {
        TreeLod::Silhouette => true,
        _ if built => distance < reach + STREAM_HYSTERESIS_M,
        _ => distance < reach,
    }
}

/// Centre (moyenne des pieds) d'un ensemble d'arbres : origine commune des
/// trois niveaux de détail, pour que leurs fondus coïncident exactement.
#[must_use]
pub fn spots_center(spots: &[TreeSpot]) -> [f32; 3] {
    let n = spots.len().max(1) as f32;
    let sum = spots.iter().fold([0.0f32; 3], |acc, s| [acc[0] + s.base[0], acc[1] + s.base[1], acc[2] + s.base[2]]);
    [sum[0] / n, sum[1] / n, sum[2] / n]
}

/// Plante des arbres dans une région : grille jitterée, probabilité = densité
/// de forêt. `spacing_at` renvoie l'espacement local (ou `None` : interdit).
fn plant(world: &World, region: Region, spacing_at: impl Fn(f32, f32) -> Option<f32>) -> Vec<TreeSpot> {
    let mut spots = Vec::new();
    let base = 8.0f32;
    let mut gz = (region.z0 / base).floor() as i64;
    while (gz as f32) * base < region.z1 {
        let mut gx = (region.x0 / base).floor() as i64;
        while (gx as f32) * base < region.x1 {
            let (cx, cz) = (gx as f32 * base, gz as f32 * base);
            gx += 1;
            if cx < region.x0 || cz < region.z0 {
                continue;
            }
            let Some(spacing) = spacing_at(cx, cz) else {
                continue;
            };
            // Espacement plus large : on ne garde qu'une cellule sur k².
            let k = (spacing / base).round().max(1.0) as i64;
            let (ix, iz) = (gx - 1, gz);
            if ix.rem_euclid(k) != 0 || iz.rem_euclid(k) != 0 {
                continue;
            }
            let r = hash3(ix, iz, 1);
            let px = cx + (hash3(ix, iz, 2) - 0.5) * spacing * 0.9;
            let pz = cz + (hash3(ix, iz, 3) - 0.5) * spacing * 0.9;
            // Le décalage peut faire changer de tuile : on revalide le point final.
            if spacing_at(px, pz).is_none() {
                continue;
            }
            let y = ground_altitude_m(world, px, pz);
            let density = forest_density(world, px, pz, y);
            if r >= density * 0.92 {
                continue;
            }
            // Pas d'arbre sur les pentes fortes (on y voit la roche).
            let slope = (ground_altitude_m(world, px + 3.0, pz) - y)
                .abs()
                .max((ground_altitude_m(world, px, pz + 3.0) - y).abs())
                / 3.0;
            if slope > 0.7 {
                continue;
            }
            // Essence : les résineux gagnent avec l'altitude, les bouleaux
            // aiment les berges, les chênes la plaine.
            let pick = hash3(ix, iz, 5);
            let conifer_share = 0.30 + smoothstep(250.0, 900.0, y) * 0.65;
            let species = if pick < conifer_share {
                if hash3(ix, iz, 8) < 0.6 { Species::Spruce } else { Species::Pine }
            } else if hash3(ix, iz, 8) < 0.25 + (1.0 - smoothstep(2.0, 12.0, y)) * 0.5 {
                Species::Birch
            } else {
                Species::Oak
            };
            let size = (spacing / 8.0).sqrt().min(1.5);
            spots.push(TreeSpot {
                base: [px, y, pz],
                species,
                scale: size * (0.62 + hash3(ix, iz, 4) * 0.55),
                tint: hash3(ix, iz, 6),
                angle: hash3(ix, iz, 7) * std::f32::consts::TAU,
            });
        }
        gz += 1;
    }
    spots
}

/// Ajoute un modèle transformé (rotation verticale, échelle, teinte).
fn push_model(chunk: &mut TreeChunk, model: &TreeModel, base: [f32; 3], spot: &TreeSpot) {
    let start = chunk.positions.len() as u32;
    let (sin, cos) = spot.angle.sin_cos();
    // Légère variation d'élancement : pas deux arbres identiques.
    let stretch = 0.9 + spot.tint * 0.25;
    let tone = 0.82 + spot.tint * 0.32;
    let autumn = if spot.species.is_conifer() { 0.0 } else { (spot.tint - 0.8).max(0.0) * 2.5 };
    for ((p, n), c) in model.positions.iter().zip(&model.normals).zip(&model.colors) {
        let (x, z) = (p[0] * cos - p[2] * sin, p[0] * sin + p[2] * cos);
        chunk.positions.push([
            base[0] + x * spot.scale,
            base[1] + p[1] * spot.scale * stretch,
            base[2] + z * spot.scale,
        ]);
        chunk.normals.push([n[0] * cos - n[2] * sin, n[1], n[0] * sin + n[2] * cos]);
        chunk.colors.push([
            (c[0] * tone + autumn * 0.06).min(1.0),
            c[1] * tone,
            c[2] * tone * (1.0 - autumn * 0.3),
            1.0,
        ]);
    }
    chunk.indices.extend(model.indices.iter().map(|i| start + i));
}

/// Silhouette lointaine : un cône (résineux) ou une double pyramide (feuillu),
/// aux proportions des modèles.
fn push_silhouette(chunk: &mut TreeChunk, base: [f32; 3], spot: &TreeSpot) {
    let s = spot.scale;
    let t = spot.tint;
    match spot.species {
        Species::Spruce => {
            let color = [0.05 + t * 0.02, 0.12 + t * 0.03, 0.06, 1.0];
            push_cone(chunk, [base[0], base[1] + 1.0 * s, base[2]], 3.6 * s, 0.0, 20.0 * s, 5, spot.angle, color);
        }
        Species::Pine => {
            let color = [0.07 + t * 0.02, 0.14 + t * 0.03, 0.06, 1.0];
            push_blob(chunk, [base[0], base[1] + 13.5 * s, base[2]], 3.2 * s, 2.2 * s, spot.angle, color, 4);
        }
        Species::Oak => {
            let color = [0.10 + t * 0.04, 0.19 + t * 0.04, 0.05, 1.0];
            push_blob(chunk, [base[0], base[1] + 8.5 * s, base[2]], 5.5 * s, 4.0 * s, spot.angle, color, 4);
        }
        Species::Birch => {
            let color = [0.17 + t * 0.04, 0.28 + t * 0.04, 0.08, 1.0];
            push_blob(chunk, [base[0], base[1] + 10.0 * s, base[2]], 2.6 * s, 4.0 * s, spot.angle, color, 4);
        }
    }
}

/// Tronc de cône (`top_r = 0` : cône) à `sides` faces, normales lissées.
#[allow(clippy::too_many_arguments)]
fn push_cone(chunk: &mut TreeChunk, base: [f32; 3], r0: f32, r1: f32, height: f32, sides: u32, rot: f32, color: [f32; 4]) {
    let start = chunk.positions.len() as u32;
    let slope = (r0 - r1) / height.max(1e-3);
    for s in 0..sides {
        let a = rot + s as f32 / sides as f32 * std::f32::consts::TAU;
        let (c, sn) = (a.cos(), a.sin());
        let len = (1.0 + slope * slope).sqrt();
        let n = [c / len, slope / len, sn / len];
        chunk.positions.push([base[0] + c * r0, base[1], base[2] + sn * r0]);
        chunk.positions.push([base[0] + c * r1, base[1] + height, base[2] + sn * r1]);
        chunk.normals.push(n);
        chunk.normals.push(n);
        chunk.colors.push(color);
        // Sommet plus clair : lumière du ciel sur le haut du feuillage.
        chunk.colors.push([color[0] * 1.25, color[1] * 1.25, color[2] * 1.2, 1.0]);
    }
    for s in 0..sides {
        let a = start + s * 2;
        let b = start + ((s + 1) % sides) * 2;
        if r1 > 0.0 {
            chunk.indices.extend_from_slice(&[a, a + 1, b, b, a + 1, b + 1]);
        } else {
            // Cône pointu : un seul triangle par face (l'autre serait dégénéré).
            chunk.indices.extend_from_slice(&[a, a + 1, b]);
        }
    }
}

/// Houppier de feuillu : double pyramide à 6 côtés.
fn push_blob(chunk: &mut TreeChunk, center: [f32; 3], radius: f32, half_height: f32, rot: f32, color: [f32; 4], sides: u32) {
    let start = chunk.positions.len() as u32;
    let top = [center[0], center[1] + half_height, center[2]];
    let bottom = [center[0], center[1] - half_height * 0.8, center[2]];
    chunk.positions.push(top);
    chunk.normals.push([0.0, 1.0, 0.0]);
    chunk.colors.push([color[0] * 1.3, color[1] * 1.3, color[2] * 1.2, 1.0]);
    chunk.positions.push(bottom);
    chunk.normals.push([0.0, -1.0, 0.0]);
    chunk.colors.push([color[0] * 0.7, color[1] * 0.7, color[2] * 0.7, 1.0]);
    for s in 0..sides {
        let a = rot + s as f32 / sides as f32 * std::f32::consts::TAU;
        let wobble = 0.85 + 0.3 * ((s * 7 + 3) % 5) as f32 / 4.0;
        chunk.positions.push([center[0] + a.cos() * radius * wobble, center[1], center[2] + a.sin() * radius * wobble]);
        chunk.normals.push([a.cos() * 0.9, 0.35, a.sin() * 0.9]);
        chunk.colors.push(color);
    }
    for s in 0..sides {
        let a = start + 2 + s;
        let b = start + 2 + (s + 1) % sides;
        chunk.indices.extend_from_slice(&[start, a, b, start + 1, b, a]);
    }
}

// ------------------------------------------------------------- eau, ciel

/// Plan d'eau unique au niveau de la mer, couvrant tout le décor. Le relief
/// passe dessous partout où il est sec : c'est la forme du terrain qui
/// dessine le fleuve et les lacs.
#[must_use]
pub fn water_plane(world: &World, cells: u32) -> (Vec<[f32; 3]>, Vec<u32>) {
    let (w, h) = play_size_m(world);
    let level = sim_core::terrain::WATER_LEVEL_CM as f32 / 100.0;
    let (x0, z0) = (-DECOR_EXTENT_M, -DECOR_EXTENT_M);
    let (sx, sz) = ((w + 2.0 * DECOR_EXTENT_M) / cells as f32, (h + 2.0 * DECOR_EXTENT_M) / cells as f32);
    let mut positions = Vec::new();
    for j in 0..=cells {
        for i in 0..=cells {
            positions.push([x0 + i as f32 * sx, level, z0 + j as f32 * sz]);
        }
    }
    let mut indices = Vec::new();
    let row = cells + 1;
    for j in 0..cells {
        for i in 0..cells {
            let a = j * row + i;
            indices.extend_from_slice(&[a, a + row, a + 1, a + 1, a + row, a + row + 1]);
        }
    }
    (positions, indices)
}

/// Dôme de ciel (sphère unité, à mettre à l'échelle) avec dégradé
/// horizon → zénith en couleur de sommet. Sous l'horizon : couleur d'horizon.
#[must_use]
pub fn sky_dome(rings: u32, segments: u32) -> (Vec<[f32; 3]>, Vec<[f32; 4]>, Vec<u32>) {
    let mut positions = Vec::new();
    let mut colors = Vec::new();
    for r in 0..=rings {
        let v = r as f32 / rings as f32; // 0 = zénith, 1 = nadir
        let phi = v * std::f32::consts::PI;
        let y = phi.cos();
        let t = smoothstep(0.0, 0.55, y.max(0.0)).powf(0.8);
        let color = [
            HORIZON_COLOR[0] + (ZENITH_COLOR[0] - HORIZON_COLOR[0]) * t,
            HORIZON_COLOR[1] + (ZENITH_COLOR[1] - HORIZON_COLOR[1]) * t,
            HORIZON_COLOR[2] + (ZENITH_COLOR[2] - HORIZON_COLOR[2]) * t,
            1.0,
        ];
        for s in 0..=segments {
            let theta = s as f32 / segments as f32 * std::f32::consts::TAU;
            positions.push([phi.sin() * theta.cos(), y, phi.sin() * theta.sin()]);
            colors.push(color);
        }
    }
    let mut indices = Vec::new();
    let row = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let a = r * row + s;
            indices.extend_from_slice(&[a, a + 1, a + row, a + 1, a + row + 1, a + row]);
        }
    }
    (positions, colors, indices)
}

// ------------------------------------------------------------- utilitaires

fn smoothstep(e0: f32, e1: f32, v: f32) -> f32 {
    let t = ((v - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Hachage → `0..1`, pour le placement des arbres (rendu uniquement).
fn hash3(x: i64, z: i64, salt: u64) -> f32 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (z as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ salt.wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 31;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 29;
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Bruit de valeur lissé `0..1` (variation des textures).
fn noise2(x: f32, z: f32) -> f32 {
    let (ix, iz) = (x.floor(), z.floor());
    let (fx, fz) = (x - ix, z - iz);
    let (sx, sz) = (fx * fx * (3.0 - 2.0 * fx), fz * fz * (3.0 - 2.0 * fz));
    let (ix, iz) = (ix as i64, iz as i64);
    let a = hash3(ix, iz, 11);
    let b = hash3(ix + 1, iz, 11);
    let c = hash3(ix, iz + 1, 11);
    let d = hash3(ix + 1, iz + 1, 11);
    let top = a + (b - a) * sx;
    let bottom = c + (d - c) * sx;
    top + (bottom - top) * sz
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::demo_city;

    #[test]
    fn decor_edge_is_hidden_by_fog_from_every_allowed_camera() {
        let world = demo_city();
        let bounds = camera_bounds(&world);
        let play = play_size_m(&world);
        // Pire cas : point visé dans un coin autorisé, caméra reculée au maximum
        // vers l'extérieur. Le bord du décor doit rester au-delà de 1,5 × la
        // visibilité (contraste résiduel < 0,3 %).
        let nearest_edge = DECOR_EXTENT_M - bounds.max_eye_offset_from_play_area(play);
        assert!(
            nearest_edge > FOG_VISIBILITY_M * 1.5,
            "bord du décor à {nearest_edge} m, visibilité {FOG_VISIBILITY_M} m"
        );
        // Le ciel englobe tout le décor.
        const { assert!(SKY_RADIUS_M > DECOR_EXTENT_M * 2.0) };
    }

    #[test]
    fn plunging_views_never_reach_the_decor_edge() {
        // Pour chaque inclinaison : ou bien l'horizon est visible et le
        // brouillard reste à la visibilité de base, ou bien le rayon le plus
        // haut touche le sol avant le bord du décor.
        let world = demo_city();
        let bounds = camera_bounds(&world);
        let play = play_size_m(&world);
        let edge = DECOR_EXTENT_M - bounds.max_eye_offset_from_play_area(play);
        let mut pitch = bounds.min_pitch;
        while pitch <= bounds.max_pitch {
            let visibility = fog_visibility_m(pitch, CAMERA_FOV);
            let top = pitch - CAMERA_FOV * 0.5;
            if top <= 0.0 {
                assert!(visibility <= FOG_VISIBILITY_M * 1.0001, "horizon visible, brouillard allongé à {pitch}");
            } else {
                let eye_height = bounds.max_distance * pitch.sin() + 2_000.0; // + sommets
                let reach = eye_height / top.tan();
                assert!(
                    reach < edge || visibility * 1.5 < edge,
                    "pitch {pitch} : sol visible jusqu'à {reach} m, bord à {edge} m"
                );
            }
            pitch += 0.01;
        }
    }

    #[test]
    fn camera_focus_is_clamped_near_the_play_area() {
        let world = demo_city();
        let bounds = camera_bounds(&world);
        let (w, h) = play_size_m(&world);
        let (x, z) = bounds.clamp_focus(-50_000.0, 90_000.0);
        assert!(x >= -0.2 * w && z <= 1.2 * h);
        let (x, z) = bounds.clamp_focus(w * 0.5, h * 0.5);
        assert_eq!((x, z), (w * 0.5, h * 0.5));
    }

    #[test]
    fn ground_is_continuous_at_the_play_area_border() {
        let world = demo_city();
        let (w, _) = play_size_m(&world);
        let tile = world.config().tile_size_m as f32;
        for k in 0..64 {
            let z = 40.0 + k as f32 * 29.0;
            // De part et d'autre de la limite du maillage jouable (centre de la
            // dernière tuile) : pas de marche.
            let inside = ground_altitude_m(&world, w - tile * 0.5 - 0.01, z);
            let outside = ground_altitude_m(&world, w - tile * 0.5 + 0.01, z);
            assert!((inside - outside).abs() < 0.05, "marche de {} m à z={z}", inside - outside);
        }
    }

    #[test]
    fn axis_covers_the_decor_with_play_tiles_exact() {
        let coords = axis_coordinates(256, 8.0, DECOR_EXTENT_M);
        assert!(coords.windows(2).all(|p| p[1] > p[0]), "axe non trié");
        assert!(coords[0] <= -DECOR_EXTENT_M + 4.0);
        assert!(*coords.last().unwrap() >= 2048.0 + DECOR_EXTENT_M - 4.0);
        for i in 0..256 {
            let c = 4.0 + i as f32 * 8.0;
            assert!(coords.iter().any(|&v| (v - c).abs() < 1e-3), "centre de tuile {i} absent");
        }
        assert!(coords.len() < 800, "trop de sommets : {}", coords.len());
    }

    #[test]
    fn splat_weights_sum_to_one_and_follow_nature() {
        for (alt, ny, forest) in [(0.0, 1.0, 0.0), (5.0, 1.0, 0.8), (40.0, 0.6, 0.0), (2_000.0, 0.98, 0.0), (300.0, 0.97, 0.0)] {
            let w = splat_weights(alt, ny, forest, 123.0, 456.0);
            let sum: f32 = w.iter().sum();
            assert!((sum - 1.0).abs() < 1e-4, "somme {sum} pour {alt} m");
            assert!(w.iter().all(|v| *v >= 0.0));
        }
        assert!(splat_weights(0.0, 1.0, 0.0, 0.0, 0.0)[4] > 0.9, "rivage en sable");
        assert!(splat_weights(40.0, 0.6, 0.0, 0.0, 0.0)[3] > 0.9, "falaise en roche");
        assert!(splat_weights(2_000.0, 0.98, 0.0, 0.0, 0.0)[5] > 0.9, "sommet enneigé");
        assert!(splat_weights(5.0, 1.0, 0.8, 0.0, 0.0)[2] > 0.5, "sous-bois en forêt");
    }

    #[test]
    fn ground_chunks_are_well_formed() {
        let world = demo_city();
        let chunks = ground_chunks(&world, 128);
        assert!(chunks.len() > 4);
        for chunk in &chunks {
            let n = chunk.positions.len();
            assert_eq!(chunk.normals.len(), n);
            assert_eq!(chunk.colors.len(), n);
            assert_eq!(chunk.uvs.len(), n);
            assert!(chunk.indices.iter().all(|&i| (i as usize) < n));
            assert!(chunk.positions.iter().flatten().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn trees_never_grow_on_the_city() {
        let mut world = demo_city();
        let cells = play_area_tree_cells(&world, 256.0);
        for _ in 0..600 {
            world.tick();
        }
        let tile = world.config().tile_size_m as f32;
        let grid = world.zoning().grid();
        let mut kept = 0;
        for cell in &cells {
            for spot in city_trees(&world, cell) {
                let id = grid.tile_at((spot.base[0] / tile) as u32, (spot.base[2] / tile) as u32).unwrap();
                assert!(world.buildings().at(id).is_none(), "arbre sur un bâtiment");
                assert_eq!(grid.get(id), Some(&ZoneType::Unzoned), "arbre sur une zone");
                kept += 1;
            }
        }
        assert!(kept > 1_000, "la ville n'a presque plus d'arbres : {kept}");
    }

    #[test]
    fn blender_models_load_within_budget() {
        let models = TreeModels::embedded();
        for species in Species::ALL {
            let full = &models.model[species.index()];
            let reduced = &models.reduced[species.index()];
            let (tf, tr) = (full.indices.len() / 3, reduced.indices.len() / 3);
            assert!((150..=900).contains(&tf), "{species:?} : {tf} triangles");
            assert!(tr <= 64 && tr * 5 < tf, "{species:?} réduit : {tr} triangles");
            for model in [full, reduced] {
                assert!(model.indices.iter().all(|&i| (i as usize) < model.positions.len()));
                // Pied à l'origine, debout (Y vers le haut), hauteur d'arbre.
                let min_y = model.positions.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
                let max_y = model.positions.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
                assert!(min_y > -1.5 && min_y < 1.0, "{species:?} : pied à {min_y}");
                assert!((8.0..30.0).contains(&max_y), "{species:?} : hauteur {max_y}");
                assert!(model.normals.iter().all(|n| ((n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() - 1.0).abs() < 0.02));
            }
        }
    }

    #[test]
    fn forest_geometry_fits_in_memory() {
        // Garde-fou : la version qui construisait le modèle complet partout
        // dépassait 170 M de sommets et faisait tuer le terminal par oomd.
        // Pire cas : caméra posée sur la cellule la plus boisée.
        let world = demo_city();
        let models = TreeModels::embedded();
        let vertices = |lod: TreeLod, species: Species| match lod {
            TreeLod::Model => models.model[species.index()].positions.len(),
            TreeLod::Reduced => models.reduced[species.index()].positions.len(),
            TreeLod::Silhouette => {
                let spot = TreeSpot { base: [0.0; 3], species, scale: 1.0, tint: 0.5, angle: 0.0 };
                build_trees(&[spot], TreeLod::Silhouette, &models, [0.0; 3]).positions.len()
            }
        };
        let mut cells: Vec<Vec<TreeSpot>> = decor_tree_cells(&world, 500.0);
        cells.extend(play_area_tree_cells(&world, 256.0).into_iter().map(|c| c.into_iter().map(|(s, _)| s).collect()));
        let centers: Vec<[f32; 3]> = cells.iter().map(|c| spots_center(c)).collect();
        let silhouettes: usize = cells.iter().flatten().map(|s| vertices(TreeLod::Silhouette, s.species)).sum();
        let mut worst = 0usize;
        for eye in &centers {
            let mut streamed = 0usize;
            for (cell, center) in cells.iter().zip(&centers) {
                let d = ((eye[0] - center[0]).powi(2) + (eye[2] - center[2]).powi(2)).sqrt();
                for lod in [TreeLod::Model, TreeLod::Reduced] {
                    if streamed_lod_wanted(lod, d, true) {
                        streamed += cell.iter().map(|s| vertices(lod, s.species)).sum::<usize>();
                    }
                }
            }
            worst = worst.max(streamed);
        }
        let total = silhouettes + worst;
        // 40 octets par sommet (position, normale, couleur) : < 400 Mo.
        assert!(total < 10_000_000, "{total} sommets (silhouettes {silhouettes}, streaming {worst})");
    }

    #[test]
    fn streaming_has_hysteresis() {
        let reach = tree_lod_distances(TreeLod::Model).1 .1 + STREAM_MARGIN_M;
        assert!(streamed_lod_wanted(TreeLod::Model, reach - 1.0, false));
        assert!(!streamed_lod_wanted(TreeLod::Model, reach + 1.0, false));
        // Déjà construit : on le garde un peu au-delà.
        assert!(streamed_lod_wanted(TreeLod::Model, reach + 1.0, true));
        assert!(!streamed_lod_wanted(TreeLod::Model, reach + STREAM_HYSTERESIS_M + 1.0, true));
        assert!(streamed_lod_wanted(TreeLod::Silhouette, 1e9, false));
    }

    #[test]
    fn tree_lods_share_their_placement() {
        let world = demo_city();
        let models = TreeModels::embedded();
        let cells = decor_tree_cells(&world, 1_000.0);
        let spots = &cells[cells.len() / 2];
        let origin = spots_center(spots);
        let lods = [TreeLod::Model, TreeLod::Reduced, TreeLod::Silhouette].map(|lod| build_trees(spots, lod, &models, origin));
        assert!(lods.iter().all(|c| c.trees == spots.len()));
        // Chaque niveau coûte nettement moins que le précédent.
        assert!(lods[1].indices.len() * 5 < lods[0].indices.len());
        assert!(lods[2].indices.len() * 3 < lods[1].indices.len());
    }
}
