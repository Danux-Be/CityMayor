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
    tile_center_m, BuildingKind, Command, Fixed, Point, Rect, RoadSpec, SimConfig, World,
    WorldGenParams, ZoneType,
};
#[cfg(test)]
use sim_core::Terrain;

/// Taille de la carte de démonstration (tuiles de 8 m → 2 048 m de côté,
/// l'ordre de grandeur d'une case de *Cities: Skylines*).
pub const DEMO_MAP_TILES: u32 = 256;

/// Construit un monde de démonstration sur un relief généré (collines,
/// fleuve, lacs, montagnes à l'horizon) : des avenues tracées seulement sur la
/// terre ferme, de part et d'autre du fleuve, un boulevard courbe et un
/// zonage mixte. Vit ici (sans Bevy) pour être testé.
#[must_use]
pub fn demo_city() -> World {
    let mut world = World::generated(
        SimConfig::default(),
        0x0C17_CAFE,
        WorldGenParams::default(),
        DEMO_MAP_TILES,
        DEMO_MAP_TILES,
    );
    let max_slope = world.config().max_slope_cm;
    let dry = |world: &World, x: u32, y: u32| -> bool {
        world
            .zoning()
            .grid()
            .tile_at(x, y)
            .is_some_and(|tile| world.terrain().is_buildable(tile, max_slope))
    };

    let mut west_ends = Vec::new();
    for row in 0..17u32 {
        let y = 24 + row * 12;
        // Tronçons secs d'au moins 16 tuiles : le fleuve coupe une avenue en
        // deux tronçons (pas encore de ponts).
        let mut runs = Vec::new();
        let mut start = None;
        for x in 6..DEMO_MAP_TILES - 6 {
            match (dry(&world, x, y), start) {
                (true, None) => start = Some(x),
                (false, Some(s)) => {
                    runs.push((s, x - 1));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            runs.push((s, DEMO_MAP_TILES - 7));
        }
        for (x0, x1) in runs.into_iter().filter(|(a, b)| b - a >= 16) {
            let (x0, x1) = (x0 + 1, x1 - 1);
            world.queue(Command::BuildRoad {
                from: Point::new(tile_center_m(x0 as i32), tile_center_m(y as i32)),
                to: Point::new(tile_center_m(x1 as i32), tile_center_m(y as i32)),
                spec: RoadSpec::AVENUE,
            });
            if x0 < 40 {
                west_ends.push((x0, y));
            }
            for (band, zone) in [
                (y + 1, ZoneType::Residential),
                (y + 2, ZoneType::Residential),
                (y + 3, ZoneType::Residential),
                (y - 1, ZoneType::Commercial),
                (y - 2, ZoneType::Industrial),
                (y - 3, ZoneType::Industrial),
            ] {
                world.queue(Command::ZoneRect {
                    rect: Rect::new(x0, band, x1, band),
                    zone,
                });
            }
        }
    }

    // Boulevard courbe entre l'extrémité ouest de deux avenues. Ces nœuds
    // existent déjà (poignée nulle) et une poignée n'est appliquée qu'à un nœud
    // *créé* par la commande (invariant 11) : un seul segment serait une
    // droite. On passe par un nœud intermédiaire neuf, porteur de la tangente :
    // les deux demi-segments sont courbes et partagent cette tangente (G1).
    if let (Some(&(ax, ay)), Some(&(bx, by))) = (west_ends.first(), west_ends.get(4)) {
        let (mx, my) = ((ax + bx) / 2 + 30, (ay + by) / 2);
        if dry(&world, mx, my) {
            let middle = Point::new(tile_center_m(mx as i32), tile_center_m(my as i32));
            world.queue(Command::BuildCurve {
                from: Point::new(tile_center_m(ax as i32), tile_center_m(ay as i32)),
                to: middle,
                from_handle: Point::default(),
                to_handle: Point::new(Fixed::ZERO, Fixed::from_int(110)),
                spec: RoadSpec::TWO_WAY,
            });
            world.queue(Command::BuildCurve {
                from: middle,
                to: Point::new(tile_center_m(bx as i32), tile_center_m(by as i32)),
                from_handle: Point::default(),
                to_handle: Point::default(),
                spec: RoadSpec::TWO_WAY,
            });
        }
    }
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
        // Fondations : sur une pente, la boîte descend sous le sol au lieu de
        // flotter au-dessus du côté aval.
        let foundation = FOUNDATION_DEPTH_M;
        let cx = (x as f32 + 0.5) * tile_size;
        let cz = (y as f32 + 0.5) * tile_size;
        blocks.push(BuildingBlock {
            min: [cx - half, ground - foundation, cz - half],
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

/// Profondeur des fondations sous l'altitude de la tuile (pentes ≤ 4 m/tuile).
const FOUNDATION_DEPTH_M: f32 = 3.0;

/// Surépaisseur de la chaussée au-dessus du terrain (évite le z-fighting).
const ROAD_LIFT_M: f32 = 0.45;

/// Altitude du sol, en mètres, à une position monde (mètres) : la même
/// surface que le maillage du sol (bilinéaire), prolongée hors carte.
pub(crate) fn altitude_at(world: &World, x: f32, z: f32) -> f32 {
    crate::nature::ground_altitude_m(world, x, z)
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
    fn demo_city_builds_on_both_banks_with_a_curved_boulevard() {
        let mut world = demo_city();
        world.tick();
        let roads = world.roads();
        assert!(roads.live_segments() >= 20, "{} segments", roads.live_segments());
        let curved = roads.segments().filter(|(_, seg)| !seg.curve.is_straight()).count();
        assert_eq!(curved, 2, "le boulevard doit être courbe");
        // Aucune route ne part d'une tuile noyée.
        let tile = world.config().tile_size_m as f32;
        for (_, node) in roads.nodes() {
            let (x, z) = (node.pos.x.to_f32() / tile, node.pos.y.to_f32() / tile);
            let id = world.zoning().grid().tile_at(x as u32, z as u32).unwrap();
            assert!(world.terrain().altitude_cm(id).unwrap() > 0, "nœud dans l'eau");
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
