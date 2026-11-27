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
/// fleuve, lacs, montagnes à l'horizon), avec tout le catalogue routier :
/// rangées d'avenues, de rues, de boulevards et de sens uniques sur la terre
/// ferme, une autoroute au nord reliée à la ville par un rond-point, un
/// chemin vers la campagne et un boulevard courbe. Zonage mixte. Le réseau
/// initial est livré ouvert ; [`demo_road_works`] ouvre ensuite un chantier.
/// Vit ici (sans Bevy) pour être testé.
#[must_use]
pub fn demo_city() -> World {
    // Rayon d'accès de 3 tuiles : le premier rang de zonage est à deux tuiles
    // de l'axe de la route, la tuile entre les deux sert d'accotement et de
    // jardin de devant.
    let config = SimConfig { road_access_radius_tiles: 3, road_construction: false, ..SimConfig::default() };
    let mut world = World::generated(config, 0x0C17_CAFE, WorldGenParams::default(), DEMO_MAP_TILES, DEMO_MAP_TILES);
    let max_slope = world.config().max_slope_cm;
    let dry = |world: &World, x: u32, y: u32| -> bool {
        world.zoning().grid().tile_at(x, y).is_some_and(|tile| world.terrain().is_buildable(tile, max_slope))
    };
    // Tronçons secs d'au moins 16 tuiles sur une rangée : le fleuve coupe les
    // routes est-ouest (pas encore de ponts).
    let dry_runs = |world: &World, y: u32| -> Vec<(u32, u32)> {
        let mut runs = Vec::new();
        let mut start = None;
        for x in 6..DEMO_MAP_TILES - 6 {
            match (dry(world, x, y), start) {
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
        runs.into_iter().filter(|(a, b)| b - a >= 16).map(|(a, b)| (a + 1, b - 1)).collect()
    };
    let at = |x: u32, y: u32| Point::new(tile_center_m(x as i32), tile_center_m(y as i32));
    let road = |world: &mut World, from: Point, to: Point, spec: RoadSpec| world.queue(Command::BuildRoad { from, to, spec });
    let zone = |world: &mut World, x0: u32, x1: u32, y: u32| {
        for (band, zone) in [
            (y + 2, ZoneType::Residential),
            (y + 3, ZoneType::Residential),
            (y - 2, ZoneType::Commercial),
            (y - 3, ZoneType::Industrial),
        ] {
            world.queue(Command::ZoneRect { rect: Rect::new(x0, band, x1, band), zone });
        }
    };

    // --- rangées urbaines : le type de route varie d'une rangée à l'autre.
    let row_spec = |row: u32| match row % 5 {
        0 => RoadSpec::AVENUE,
        1 | 4 => RoadSpec::TWO_WAY,
        2 => RoadSpec::BOULEVARD,
        _ => RoadSpec::ONE_WAY_SINGLE,
    };
    let mut west_ends = Vec::new();
    let mut ring: Option<(u32, u32)> = None;
    let mut last_east = None;
    for row in 0..21u32 {
        let y = 24 + row * 10;
        let spec = row_spec(row);
        for (x0, x1) in dry_runs(&world, y) {
            // Sens uniques : un sens sur deux, d'est en ouest.
            let (from, to) = if spec.is_one_way() && row % 2 == 1 { (at(x1, y), at(x0, y)) } else { (at(x0, y), at(x1, y)) };
            if row == 0 && ring.is_none() && x1 - x0 > 60 {
                // Premier tronçon de la première avenue : coupé par le rond-point.
                let xr = x0 + 30;
                ring = Some((xr, y));
                let c = at(xr, y);
                let r = Fixed::from_int(DEMO_ROUNDABOUT_RADIUS_M);
                world.queue(Command::BuildRoundabout { center: c, radius_m: r, spec: RoadSpec::ONE_WAY });
                road(&mut world, from, Point::new(c.x - r, c.y), spec);
                road(&mut world, Point::new(c.x + r, c.y), to, spec);
                zone(&mut world, x0, xr - 5, y);
                zone(&mut world, xr + 5, x1, y);
            } else {
                road(&mut world, from, to, spec);
                zone(&mut world, x0, x1, y);
            }
            if x0 < 40 {
                west_ends.push((x0, y));
            }
            last_east = Some((x1, y));
        }
    }

    // --- autoroute au nord, reliée au rond-point par une rue. Le dernier
    // tronçon à l'est est laissé au chantier (`demo_road_works`).
    let hy = DEMO_HIGHWAY_ROW;
    for (x0, x1) in dry_runs(&world, hy) {
        let east = x1.saturating_sub(DEMO_HIGHWAY_WORKS_TILES).max(x0 + 8);
        match ring {
            Some((xr, _)) if xr > x0 && xr < east => {
                road(&mut world, at(x0, hy), at(xr, hy), RoadSpec::HIGHWAY);
                road(&mut world, at(xr, hy), at(east, hy), RoadSpec::HIGHWAY);
            }
            _ => road(&mut world, at(x0, hy), at(east, hy), RoadSpec::HIGHWAY),
        }
    }
    if let Some((xr, yr)) = ring {
        let c = at(xr, yr);
        let r = Fixed::from_int(DEMO_ROUNDABOUT_RADIUS_M);
        road(&mut world, at(xr, hy), Point::new(c.x, c.y - r), RoadSpec::TWO_WAY);
    }

    // --- chemin vers la campagne, au sud-est.
    if let Some((x1, y)) = last_east {
        let end_y = (y + 26).min(DEMO_MAP_TILES - 4);
        if dry(&world, x1 - 12, end_y) {
            road(&mut world, at(x1, y), at(x1 - 12, end_y), RoadSpec::GRAVEL);
        }
    }

    // Boulevard courbe entre l'extrémité ouest de deux rangées. Ces nœuds
    // existent déjà (poignée nulle) et une poignée n'est appliquée qu'à un nœud
    // *créé* par la commande (invariant 11) : un seul segment serait une
    // droite. On passe par un nœud intermédiaire neuf, porteur de la tangente :
    // les deux demi-segments sont courbes et partagent cette tangente (G1).
    if let (Some(&(ax, ay)), Some(&(bx, by))) = (west_ends.first(), west_ends.get(4)) {
        let (mx, my) = ((ax + bx) / 2 + 30, (ay + by) / 2);
        if dry(&world, mx, my) {
            let middle = at(mx, my);
            world.queue(Command::BuildCurve {
                from: at(ax, ay),
                to: middle,
                from_handle: Point::default(),
                to_handle: Point::new(Fixed::ZERO, Fixed::from_int(110)),
                spec: RoadSpec::TWO_WAY,
            });
            world.queue(Command::BuildCurve {
                from: middle,
                to: at(bx, by),
                from_handle: Point::default(),
                to_handle: Point::default(),
                spec: RoadSpec::TWO_WAY,
            });
        }
    }
    world
}

/// Carte vierge d'une nouvelle partie : le relief généré (collines,
/// fleuve, lacs, forêts, montagnes), sans zone ni bâtiment. Seule une
/// entrée d'autoroute arrive de l'ouest et se prolonge par une courte rue :
/// c'est le raccordement au reste du pays, d'où le joueur trace sa ville.
/// Les routes du joueur passent ensuite par un chantier.
#[must_use]
pub fn blank_map() -> World {
    let config = SimConfig { road_access_radius_tiles: 3, road_construction: false, ..SimConfig::default() };
    let mut world = World::generated(config, 0x0C17_CAFE, WorldGenParams::default(), DEMO_MAP_TILES, DEMO_MAP_TILES);
    let max_slope = world.config().max_slope_cm;
    let dry = |world: &World, x: u32, y: u32| {
        world.zoning().grid().tile_at(x, y).is_some_and(|tile| world.terrain().is_buildable(tile, max_slope))
    };
    let at = |x: u32, y: u32| Point::new(tile_center_m(x as i32), tile_center_m(y as i32));
    // Premier tronçon sec de la rangée de l'autoroute, depuis le bord ouest.
    let hy = DEMO_HIGHWAY_ROW;
    let x0 = (2..DEMO_MAP_TILES / 2).find(|&x| dry(&world, x, hy)).unwrap_or(2);
    let mut xe = x0;
    while xe < x0 + 70 && dry(&world, xe + 1, hy) {
        xe += 1;
    }
    world.queue(Command::BuildRoad { from: at(x0, hy), to: at(xe, hy), spec: RoadSpec::HIGHWAY });
    // Bretelle de sortie : une rue vers le sud, sur la terre ferme.
    let mut ye = hy;
    while ye < hy + 18 && dry(&world, xe, ye + 1) {
        ye += 1;
    }
    if ye > hy + 3 {
        world.queue(Command::BuildRoad { from: at(xe, hy), to: at(xe, ye), spec: RoadSpec::TWO_WAY });
    }
    world.queue(Command::SetRoadConstruction { enabled: true });
    world
}

/// Point de départ de la caméra : le bout de la route de départ sur une
/// carte vierge, le centre de la carte sinon.
#[must_use]
pub fn start_focus(world: &World) -> [f32; 3] {
    let roads = world.roads();
    if world.buildings().live_count() == 0 && roads.live_segments() > 0 && roads.live_segments() <= 4 {
        if let Some((_, seg)) = roads.segments().last() {
            if let Some(node) = roads.node(seg.b) {
                let (x, z) = (node.pos.x.to_f32(), node.pos.y.to_f32());
                return [x, altitude_at(world, x, z), z];
            }
        }
    }
    map_center(world)
}

/// Petit pâté de maisons pour les tests d'outils : carte de 64 × 64 tuiles
/// (512 m), deux rues, un îlot zoné, et de la place libre au sud.
#[must_use]
pub fn small_block() -> World {
    let config = SimConfig { road_access_radius_tiles: 3, road_construction: false, ..SimConfig::default() };
    // Sans fleuve : le pâté et la place libre autour restent au sec.
    let params = WorldGenParams { river: None, ..WorldGenParams::default() };
    let mut world = World::generated(config, 0x0C17_CAFE, params, 64, 64);
    let at = |x: i32, y: i32| Point::new(tile_center_m(x), tile_center_m(y));
    for (from, to) in [((8, 20), (56, 20)), ((8, 20), (8, 32)), ((56, 20), (56, 32))] {
        world.queue(Command::BuildRoad { from: at(from.0, from.1), to: at(to.0, to.1), spec: RoadSpec::TWO_WAY });
    }
    world.queue(Command::ZoneRect { rect: Rect::new(10, 22, 54, 23), zone: ZoneType::Residential });
    world.queue(Command::ZoneRect { rect: Rect::new(10, 17, 54, 18), zone: ZoneType::Commercial });
    world
}

/// Rangée (tuiles) de l'autoroute de la démo.
pub const DEMO_HIGHWAY_ROW: u32 = 9;
/// Rayon de l'anneau du rond-point de la démo (m).
pub const DEMO_ROUNDABOUT_RADIUS_M: i32 = 20;
/// Longueur (tuiles) du tronçon d'autoroute laissé au chantier.
const DEMO_HIGHWAY_WORKS_TILES: u32 = 36;

/// Ouvre le chantier de la démo : le dernier tronçon de l'autoroute, avec
/// la durée de travaux normale (la route ouvrira en cours de partie).
pub fn demo_road_works(world: &mut World) {
    world.queue(Command::SetRoadConstruction { enabled: true });
    let hy = DEMO_HIGHWAY_ROW;
    let at = |x: u32| Point::new(tile_center_m(x as i32), tile_center_m(hy as i32));
    // Extrémité est actuelle de l'autoroute : le nœud le plus à l'est sur sa rangée.
    let east = world
        .roads()
        .segments()
        .filter(|(_, s)| s.spec == RoadSpec::HIGHWAY)
        .flat_map(|(_, s)| [s.a, s.b])
        .filter_map(|n| world.roads().node(n).map(|node| node.pos))
        .max_by_key(|p| p.x);
    if let Some(from) = east {
        let x0 = (from.x.to_f32() / world.config().tile_size_m as f32) as u32;
        let x1 = (x0 + DEMO_HIGHWAY_WORKS_TILES).min(DEMO_MAP_TILES - 7);
        if x1 > x0 + 4 {
            world.queue(Command::BuildRoad { from, to: at(x1), spec: RoadSpec::HIGHWAY });
        }
    }
}

/// Repères de la démo (pour le tour de captures) : nom et point visé (m).
#[must_use]
pub fn demo_landmarks(world: &World) -> Vec<(&'static str, [f32; 3])> {
    let mut marks = Vec::new();
    let mid = |s: &sim_core::RoadSegment| {
        let p = s.curve.eval(Fixed::HALF);
        let (x, z) = (p.x.to_f32(), p.y.to_f32());
        [x, altitude_at(world, x, z), z]
    };
    if let Some(r) = world.roads().roundabouts().first() {
        let (x, z) = (r.center.x.to_f32(), r.center.y.to_f32());
        marks.push(("rond-point", [x, altitude_at(world, x, z), z]));
    }
    let now = world.current_tick();
    if let Some((_, s)) = world.roads().segments().find(|(_, s)| !s.is_open(now)) {
        marks.push(("chantier", mid(s)));
    }
    if let Some((_, s)) = world.roads().segments().find(|(_, s)| s.spec == RoadSpec::HIGHWAY && s.is_open(now)) {
        marks.push(("autoroute", mid(s)));
    }
    if let Some((_, s)) = world.roads().segments().find(|(_, s)| s.spec == RoadSpec::BOULEVARD) {
        marks.push(("boulevard", mid(s)));
    }
    if let Some((_, s)) = world.roads().segments().find(|(_, s)| s.spec == RoadSpec::ONE_WAY_SINGLE) {
        marks.push(("sens-unique", mid(s)));
    }
    marks
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
    /// Demi-largeur revêtue (`roads::Profile::half_width`).
    pub half_width_m: f32,
    pub spec: RoadSpec,
    /// Nœuds de départ et d'arrivée (indices) : carrefours et continuités.
    pub nodes: [u32; 2],
    /// Avancement du chantier `[0, 1]` (1 = route ouverte).
    pub progress: f32,
    /// Centre du rond-point si ce segment appartient à un anneau.
    pub ring_center: Option<[f32; 2]>,
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
        let points = polyline
            .iter()
            .map(|p| {
                let x = p.x.to_f32();
                let z = p.y.to_f32();
                [x, altitude_at(world, x, z) + ROAD_LIFT_M, z]
            })
            .collect();
        let ring_center = world
            .roads()
            .roundabouts()
            .iter()
            .find(|r| r.nodes.contains(&segment.a) && r.nodes.contains(&segment.b))
            .map(|r| [r.center.x.to_f32(), r.center.y.to_f32()]);
        strips.push(RoadStrip {
            points,
            half_width_m: crate::roads::Profile::of(segment.spec).half_width(),
            spec: segment.spec,
            nodes: [segment.a.raw(), segment.b.raw()],
            progress: segment.construction_progress(world.current_tick()).to_f32(),
            ring_center,
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
    fn blank_map_has_only_the_highway_entry() {
        let mut world = blank_map();
        for _ in 0..3 {
            world.tick();
        }
        let roads = world.roads();
        assert_eq!(roads.live_segments(), 2, "autoroute d'entrée + rue de départ");
        assert!(roads.segments().any(|(_, s)| s.spec == RoadSpec::HIGHWAY));
        // Livrées ouvertes ; les routes du joueur, elles, passeront par un chantier.
        let now = world.current_tick();
        assert!(roads.segments().all(|(_, s)| s.is_open(now)));
        assert!(world.config().road_construction);
        assert_eq!(world.zoning().count(ZoneType::Residential), 0);
        assert_eq!(world.buildings().live_count(), 0);
        // La caméra démarre au bout de la rue, sur la carte.
        let focus = start_focus(&world);
        assert_ne!(focus, map_center(&world));
        assert!(focus[0] > 0.0 && focus[2] > 0.0);
    }

    #[test]
    fn small_block_is_dry_land_with_room_to_build() {
        let world = small_block();
        let max_slope = world.config().max_slope_cm;
        let grid = world.zoning().grid();
        let dry = (12..52).flat_map(|y| (6..58).map(move |x| (x, y)));
        let wet: Vec<_> = dry.filter(|&(x, y)| !grid.tile_at(x, y).is_some_and(|t| world.terrain().is_buildable(t, max_slope))).collect();
        assert!(wet.len() < 40, "{} tuiles non constructibles", wet.len());
    }

    #[test]
    fn demo_city_builds_on_both_banks_with_a_curved_boulevard() {
        let mut world = demo_city();
        world.tick();
        let roads = world.roads();
        assert!(roads.live_segments() >= 20, "{} segments", roads.live_segments());
        let curved = roads.segments().filter(|(_, seg)| !seg.curve.is_straight() && !seg.fixed_curve).count();
        assert_eq!(curved, 2, "le boulevard doit être courbe");
        // Tout le catalogue de la démo est présent.
        for spec in [RoadSpec::AVENUE, RoadSpec::TWO_WAY, RoadSpec::BOULEVARD, RoadSpec::ONE_WAY_SINGLE, RoadSpec::HIGHWAY, RoadSpec::GRAVEL] {
            assert!(roads.segments().any(|(_, s)| s.spec == spec), "{spec:?} absent de la démo");
        }
        assert_eq!(roads.roundabouts().len(), 1);
        // Aucune route ne part d'une tuile noyée.
        let tile = world.config().tile_size_m as f32;
        for (_, node) in roads.nodes() {
            let (x, z) = (node.pos.x.to_f32() / tile, node.pos.y.to_f32() / tile);
            let id = world.zoning().grid().tile_at(x as u32, z as u32).unwrap();
            assert!(world.terrain().altitude_cm(id).unwrap() > 0, "nœud dans l'eau");
        }
    }

    fn demo() -> World {
        let config = SimConfig { road_construction: false, ..SimConfig::default() };
        let mut world = World::new(config, 21, Terrain::flat(32, 32));
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
