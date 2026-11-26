//! Dérivation des données de **présentation** depuis les vues de simulation.
//!
//! # Pourquoi ce module n'a aucune dépendance à Bevy
//! Il transforme `&World` (lecture seule) en structures triviales
//! (positions, boîtes, polylignes) qui n'ont aucun sens hors du rendu et ne
//! doivent **jamais** revenir dans la simulation. Comme il compile sans moteur,
//! on peut le tester instantanément et vérifier ses invariants numériques
//! (aucun NaN, boîtes non dégénérées, ordre déterministe) sans ouvrir de
//! fenêtre ni de GPU.
//!
//! # Invariants
//! - **Lecture seule** : aucune méthode ne prend `&mut World`.
//! - **Ordre déterministe** : on itère `Buildings::iter()` (ordre des indices),
//!   jamais un `HashMap` ; deux rendus de la même ville produisent la même
//!   géométrie, ce qui permet de comparer des captures en CI.
//! - **Toutes les coordonnées sont finies** : les `f32` ne viennent que de
//!   conversions de valeurs entières bornées (`Fixed::to_f32`, `i32` cm).
//! - Le rendu est en **mètres, Y vers le haut**. La simulation est en 2D
//!   (`Point { x, y }` = plan du sol) : la conversion se fait ici, une fois.

use sim_core::{
    tile_center_m, BuildingKind, Command, Fixed, Point, Rect, RoadSpec, SimConfig, Terrain,
    World, ZoneType,
};

/// Construit un monde de démonstration : un maillage d'avenues, un boulevard
/// courbe pour montrer les splines, et un zonage mixte qui produit une ville
/// lisible en quelques secondes. Vit ici (sans Bevy) pour être testé.
#[must_use]
pub fn demo_city() -> World {
    let mut world = World::new(SimConfig::default(), 0x0C17_CAFE, Terrain::flat(128, 128));
    for row in 0..10i32 {
        let y = 16 + row * 10;
        world.queue(Command::BuildRoad {
            from: Point::new(tile_center_m(8), tile_center_m(y)),
            to: Point::new(tile_center_m(120), tile_center_m(y)),
            spec: RoadSpec::AVENUE,
        });
        for (band, zone) in [
            (y + 1, ZoneType::Residential),
            (y + 2, ZoneType::Residential),
            (y + 3, ZoneType::Residential),
            (y - 1, ZoneType::Commercial),
            (y - 2, ZoneType::Industrial),
            (y - 3, ZoneType::Industrial),
        ] {
            world.queue(Command::ZoneRect {
                rect: Rect::new(8, band as u32, 120, band as u32),
                zone,
            });
        }
    }
    // Boulevard courbe entre l'extrémité ouest de la première et de la dernière
    // avenue. Ces deux nœuds existent déjà (poignée nulle) et une poignée n'est
    // appliquée qu'à un nœud *créé* par la commande (invariant 11) : un seul
    // segment entre eux serait donc une droite. On passe par un nœud
    // intermédiaire neuf, porteur de la tangente : les deux demi-segments sont
    // courbes et partagent cette tangente (G1 au milieu).
    let middle = Point::new(tile_center_m(44), tile_center_m(61));
    world.queue(Command::BuildCurve {
        from: Point::new(tile_center_m(8), tile_center_m(16)),
        to: middle,
        from_handle: Point::default(),
        to_handle: Point::new(Fixed::ZERO, Fixed::from_int(160)),
        spec: RoadSpec::TWO_WAY,
    });
    world.queue(Command::BuildCurve {
        from: middle,
        to: Point::new(tile_center_m(8), tile_center_m(106)),
        from_handle: Point::default(),
        to_handle: Point::default(),
        spec: RoadSpec::TWO_WAY,
    });
    world
}

/// Bloc de bâtiment prêt à être instancié (boîte alignée sur les axes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BuildingBlock {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub kind: BuildingKind,
}

impl BuildingBlock {
    #[must_use]
    pub fn center(&self) -> [f32; 3] {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
            (self.min[2] + self.max[2]) * 0.5,
        ]
    }
}

/// Bande de chaussée (polyligne + demi-largeur) pour générer un ruban.
#[derive(Clone, Debug, PartialEq)]
pub struct RoadStrip {
    /// Points en mètres, Y = altitude du sol (+ surépaisseur anti z-fighting).
    pub points: Vec<[f32; 3]>,
    pub half_width_m: f32,
}

/// Maillage de terrain prêt à téléverser.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerrainMeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

/// Fraction de la tuile réellement occupée par l'emprise d'un bâtiment.
///
/// Laisser un jeu visuel entre bâtiments rend la densité lisible (et évite
/// l'effet "mur continu" quand tout un quartier est bâti).
const FOOTPRINT_RATIO: f32 = 0.72;

/// Hauteur d'un bâtiment, en mètres.
///
/// Dérivée de la capacité (habitants ou emplois) : c'est le seul signal dont
/// dispose le rendu, et il est suffisant pour que la silhouette de la ville
/// raconte la densité. Les niveaux 2/3 (itération 4) affineront cette formule.
#[must_use]
pub fn building_height_m(kind: BuildingKind, capacity: u32) -> f32 {
    let base = match kind {
        BuildingKind::Household => 4.0,
        BuildingKind::Shop => 6.0,
        BuildingKind::Factory => 8.0,
    };
    let per_unit = match kind {
        BuildingKind::Household => 0.55,
        BuildingKind::Shop => 0.70,
        BuildingKind::Factory => 0.50,
    };
    (base + capacity as f32 * per_unit).clamp(6.0, 90.0)
}

/// Boîtes de tous les bâtiments, dans l'ordre des indices de tuiles.
#[must_use]
pub fn buildings(world: &World) -> Vec<BuildingBlock> {
    let tile_size = world.config().tile_size_m as f32;
    let half = tile_size * FOOTPRINT_RATIO * 0.5;
    let grid = world.zoning().grid();

    let mut blocks = Vec::with_capacity(world.buildings().live_count() as usize);
    for (_, building) in world.buildings().iter() {
        let Some((x, y)) = grid.xy(building.tile) else {
            continue;
        };
        let ground = altitude_m(world, building.tile);
        let height = building_height_m(building.kind, building.capacity);
        let cx = (x as f32 + 0.5) * tile_size;
        let cz = (y as f32 + 0.5) * tile_size;
        blocks.push(BuildingBlock {
            min: [cx - half, ground, cz - half],
            max: [cx + half, ground + height, cz + half],
            kind: building.kind,
        });
    }
    blocks
}

/// Rubans de chaussée, un par segment routier.
#[must_use]
pub fn roads(world: &World) -> Vec<RoadStrip> {
    let mut strips = Vec::with_capacity(world.roads().live_segments() as usize);
    for (id, segment) in world.roads().segments() {
        let Some(polyline) = world.roads().segment_polyline(id, segment.curve.suggested_steps())
        else {
            continue;
        };
        let lanes = segment.spec.total_lanes().max(1) as f32;
        let points = polyline
            .iter()
            .map(|p| {
                let x = p.x.to_f32();
                let z = p.y.to_f32();
                [x, altitude_at(world, x, z) + ROAD_LIFT_M, z]
            })
            .collect();
        strips.push(RoadStrip {
            points,
            half_width_m: lanes * 3.5 * 0.5,
        });
    }
    strips
}

/// Maillage du terrain : un sommet par tuile, deux triangles par cellule.
#[must_use]
pub fn terrain(world: &World) -> TerrainMeshData {
    let terrain = world.terrain();
    let (width, height) = (terrain.width(), terrain.height());
    let tile_size = world.config().tile_size_m as f32;
    let count = (width as usize) * (height as usize);

    let mut positions = Vec::with_capacity(count);
    let mut normals = Vec::with_capacity(count);
    for y in 0..height {
        for x in 0..width {
            let Some(tile) = world.zoning().grid().tile_at(x, y) else {
                continue;
            };
            positions.push([
                (x as f32 + 0.5) * tile_size,
                altitude_m(world, tile),
                (y as f32 + 0.5) * tile_size,
            ]);
            normals.push([0.0, 1.0, 0.0]);
        }
    }

    let mut indices = Vec::with_capacity(count * 6);
    for y in 0..height.saturating_sub(1) {
        for x in 0..width.saturating_sub(1) {
            let i = y * width + x;
            let right = i + 1;
            let down = i + width;
            let down_right = down + 1;
            indices.extend_from_slice(&[i, down, right, right, down, down_right]);
        }
    }

    TerrainMeshData {
        positions,
        normals,
        indices,
    }
}

/// Surépaisseur de la chaussée au-dessus du terrain (évite le z-fighting).
const ROAD_LIFT_M: f32 = 0.25;

/// Altitude du sol, en mètres, à une position monde (mètres).
fn altitude_at(world: &World, x: f32, z: f32) -> f32 {
    let tile_size = world.config().tile_size_m as f32;
    let grid = world.zoning().grid();
    let tx = ((x / tile_size).floor().max(0.0)) as u32;
    let tz = ((z / tile_size).floor().max(0.0)) as u32;
    match grid.tile_at(tx.min(grid.width() - 1), tz.min(grid.height() - 1)) {
        Some(tile) => altitude_m(world, tile),
        None => 0.0,
    }
}

/// Altitude en mètres d'une tuile (`Fixed` centimètres → f32 mètres).
fn altitude_m(world: &World, tile: sim_core::TileId) -> f32 {
    world
        .terrain()
        .altitude_cm(tile)
        .map_or(0.0, |cm| cm as f32 / 100.0)
}

/// Couleur d'accent par type de bâtiment (l'overlay de zonage s'y accroche).
#[must_use]
pub fn kind_color(kind: BuildingKind) -> [f32; 3] {
    match kind {
        BuildingKind::Household => [0.78, 0.62, 0.42],
        BuildingKind::Shop => [0.30, 0.55, 0.80],
        BuildingKind::Factory => [0.55, 0.52, 0.48],
    }
}

/// Centre de la carte en mètres, pour cadrer la caméra au démarrage.
#[must_use]
pub fn map_center(world: &World) -> [f32; 3] {
    let tile_size = world.config().tile_size_m as f32;
    let grid = world.zoning().grid();
    [
        grid.width() as f32 * tile_size * 0.5,
        0.0,
        grid.height() as f32 * tile_size * 0.5,
    ]
}

/// Vrai si toutes les coordonnées d'un bloc sont finies et non dégénérées.
#[must_use]
pub fn block_is_sane(block: &BuildingBlock) -> bool {
    block.min.iter().chain(block.max.iter()).all(|v| v.is_finite())
        && block.min[0] < block.max[0]
        && block.min[1] < block.max[1]
        && block.min[2] < block.max[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_boulevard_is_actually_curved() {
        let mut world = demo_city();
        world.tick();
        let roads = world.roads();
        // 10 avenues droites + 2 demi-boulevards courbes.
        assert_eq!(roads.live_segments(), 12);
        let curved: Vec<_> = roads
            .segments()
            .filter(|(_, seg)| !seg.curve.is_straight())
            .collect();
        assert_eq!(curved.len(), 2, "le boulevard doit être courbe");
        for (_, seg) in &curved {
            assert!(seg.length_m > seg.curve.p0.distance(seg.curve.p1));
        }
    }

    fn demo() -> World {
        let mut world = World::new(SimConfig::default(), 21, Terrain::flat(32, 32));
        world.queue(Command::BuildRoad {
            from: Point::new(tile_center_m(2), tile_center_m(8)),
            to: Point::new(tile_center_m(28), tile_center_m(8)),
            spec: RoadSpec::TWO_WAY,
        });
        world.queue(Command::BuildCurve {
            from: Point::new(tile_center_m(8), tile_center_m(8)),
            to: Point::new(tile_center_m(8), tile_center_m(28)),
            from_handle: Point::new(Fixed::from_int(12), Fixed::from_int(6)),
            to_handle: Point::new(Fixed::from_int(-6), Fixed::from_int(-12)),
            spec: RoadSpec::TWO_WAY,
        });
        world.queue(Command::ZoneRect {
            rect: Rect::new(3, 9, 26, 11),
            zone: ZoneType::Residential,
        });
        world.queue(Command::ZoneRect {
            rect: Rect::new(3, 6, 26, 7),
            zone: ZoneType::Commercial,
        });
        for _ in 0..400 {
            world.tick();
        }
        world
    }

    #[test]
    fn blocks_match_the_simulation_one_for_one() {
        let world = demo();
        let blocks = buildings(&world);
        assert_eq!(blocks.len() as u32, world.buildings().live_count());
        assert!(!blocks.is_empty(), "la démo doit produire des bâtiments");
        for block in &blocks {
            assert!(block_is_sane(block), "bloc invalide: {block:?}");
        }
    }

    #[test]
    fn heights_grow_with_capacity_and_stay_bounded() {
        let small = building_height_m(BuildingKind::Household, 6);
        let big = building_height_m(BuildingKind::Household, 48);
        assert!(big > small);
        assert!(building_height_m(BuildingKind::Factory, 0) >= 6.0);
        assert!(building_height_m(BuildingKind::Shop, 10_000) <= 90.0);
    }

    #[test]
    fn roads_are_curved_polylines_not_chords() {
        let world = demo();
        let strips = roads(&world);
        assert_eq!(strips.len(), world.roads().live_segments() as usize);
        let curved = strips
            .iter()
            .find(|s| s.points.len() > 16)
            .expect("un segment courbe doit produire une longue polyligne");
        assert!(curved.points.iter().flatten().all(|v| v.is_finite()));
        assert!(curved.half_width_m > 0.0);
    }

    #[test]
    fn terrain_mesh_is_well_formed() {
        let world = demo();
        let mesh = terrain(&world);
        let count = (world.terrain().width() * world.terrain().height()) as usize;
        assert_eq!(mesh.positions.len(), count);
        assert_eq!(mesh.normals.len(), count);
        assert!(mesh.positions.iter().flatten().all(|v| v.is_finite()));
        let limit = mesh.positions.len() as u32;
        assert!(mesh.indices.iter().all(|i| *i < limit));
        assert_eq!(mesh.indices.len() % 3, 0);
    }

    #[test]
    fn derivation_is_deterministic() {
        let world = demo();
        assert_eq!(buildings(&world), buildings(&world));
        assert_eq!(roads(&world), roads(&world));
        assert_eq!(terrain(&world), terrain(&world));
    }
}
