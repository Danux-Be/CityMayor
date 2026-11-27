//! La ville construite : bâtiments modelés, parcelles, chaussées et trottoirs.
//!
//! # Principe
//! Chaque bâtiment de la simulation (une tuile de 8 m, un type, une capacité)
//! devient une petite construction assemblée ici, sans Bevy : maison à toit
//! à deux pans ou en croupe, immeuble à balcons, commerce avec vitrine et
//! étages, usine en bardage métallique avec cheminée. Les surfaces prennent
//! leurs motifs dans un **tableau de textures** répétables rendues par
//! Blender (`tools/blender/make_buildings.py`) : briques, enduits, bardage,
//! fenêtres en relief, tuiles, ardoises, asphalte marqué, pavés…
//!
//! Un sommet porte : position, normale, coordonnées de texture **en
//! répétitions** (mètres ÷ taille de la couche, `LAYER_SIZE_M`), et une
//! couleur dont RGB est la teinte (variété des façades) et A l'indice de la
//! couche (`layer / LAYER_ENCODING`). Le shader `urban.wesl` lit l'une et
//! l'autre : un seul matériau, un seul appel de dessin par morceau de ville.
//!
//! # Déterminisme
//! Les variantes (matériau, toit, teinte, cheminée…) dérivent d'un hachage de
//! l'indice de tuile : la même ville produit toujours les mêmes maisons, et
//! une maison ne change pas d'aspect quand ses voisines poussent.
//!
//! # Orientation
//! La façade avant regarde la route la plus proche (axe de tuile le plus
//! proche de la direction de la route). La parcelle s'arrête au trottoir ;
//! si la tuile de devant est libre (accotement entre la route et le premier
//! rang de zonage), la parcelle s'y prolonge en jardin de devant.

use std::collections::HashMap;

use sim_core::{BuildingKind, ConstructionStage, TileId, World, ZoneType};

use crate::nature::{ground_altitude_m, Species, TreeSpot};
use crate::scene::RoadStrip;

/// Nombre de couches du tableau de textures urbaines.
pub const LAYER_COUNT: usize = 26;

/// Fichiers des couches (`assets/urban/<nom>.jpg`), dans l'ordre du tableau.
pub const LAYER_NAMES: [&str; LAYER_COUNT] = [
    "00_brique_fenetres",
    "01_brique",
    "02_enduit_fenetres",
    "03_enduit",
    "04_bardage_fenetres",
    "05_bardage",
    "06_immeuble",
    "07_vitrine",
    "08_bureaux",
    "09_industrie",
    "10_tuiles",
    "11_ardoises",
    "12_toit_tole",
    "13_toit_plat",
    "14_asphalte",
    "15_marquage",
    "16_trottoir",
    "17_beton",
    "18_pelouse",
    "19_porte",
    "20_chemin",
    "21_chantier",
    "22_fleche",
    "23_glissiere",
    "24_balise",
    "25_palissade",
];

/// Taille d'une répétition de chaque couche (largeur, hauteur) en mètres :
/// celle de la cellule rendue par Blender.
pub const LAYER_SIZE_M: [[f32; 2]; LAYER_COUNT] = [
    [6.0, 6.0],
    [6.0, 6.0],
    [6.0, 6.0],
    [6.0, 6.0],
    [6.0, 6.0],
    [6.0, 6.0],
    [6.0, 6.0],
    [6.0, 4.5],
    [6.0, 6.0],
    [8.0, 8.0],
    [4.0, 4.0],
    [4.0, 4.0],
    [4.0, 4.0],
    [8.0, 8.0],
    [8.0, 8.0],
    [2.0, 2.0],
    [4.0, 4.0],
    [4.0, 4.0],
    [6.0, 6.0],
    [1.6, 2.8],
    [6.0, 6.0],
    [6.0, 6.0],
    [3.5, 7.0],
    [4.0, 0.32],
    [2.0, 1.0],
    [2.5, 2.0],
];

/// L'indice de couche est codé dans l'alpha de la couleur : `a = couche / 32`.
pub const LAYER_ENCODING: f32 = 32.0;

/// Couches du tableau de textures urbaines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    BrickWindows = 0,
    Brick,
    PlasterWindows,
    Plaster,
    SidingWindows,
    Siding,
    Apartment,
    Shopfront,
    Office,
    Industrial,
    ClayTiles,
    Slate,
    MetalRoof,
    FlatRoof,
    Asphalt,
    Marking,
    Sidewalk,
    Concrete,
    Lawn,
    Door,
    Gravel,
    Works,
    Arrow,
    Guardrail,
    Beacon,
    Hoarding,
}

impl Layer {
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    #[must_use]
    pub const fn size_m(self) -> [f32; 2] {
        LAYER_SIZE_M[self as usize]
    }

    pub(crate) fn alpha(self) -> f32 {
        self as usize as f32 / LAYER_ENCODING
    }
}

/// Hauteur d'un étage (m) : celle des cellules de façade (deux étages par 6 m).
pub const FLOOR_M: f32 = 3.0;
/// Hauteur du rez-de-chaussée commercial (cellule « vitrine »).
pub const SHOP_FLOOR_M: f32 = 4.5;
/// Trottoir : surélévation au-dessus de la chaussée.
pub const CURB_M: f32 = 0.15;


/// Géométrie urbaine prête à téléverser.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UrbanMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

impl UrbanMesh {
    #[must_use]
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// Toutes les valeurs finies, indices dans les bornes, couches valides.
    #[must_use]
    pub fn is_sane(&self) -> bool {
        let n = self.positions.len();
        self.normals.len() == n
            && self.uvs.len() == n
            && self.colors.len() == n
            && self.indices.len() % 3 == 0
            && self.indices.iter().all(|&i| (i as usize) < n)
            && self.positions.iter().flatten().all(|v| v.is_finite())
            && self.uvs.iter().flatten().all(|v| v.is_finite())
            && self.colors.iter().all(|c| {
                let layer = (c[3] * LAYER_ENCODING).round();
                (c[3] * LAYER_ENCODING - layer).abs() < 1e-3 && (layer as usize) < LAYER_COUNT
            })
    }

    /// Ajoute les sommets d'un autre maillage.
    pub fn append(&mut self, other: &Self) {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(&other.positions);
        self.normals.extend_from_slice(&other.normals);
        self.uvs.extend_from_slice(&other.uvs);
        self.colors.extend_from_slice(&other.colors);
        self.indices.extend(other.indices.iter().map(|i| i + base));
    }

    /// Polygone convexe plan (éventail). La face regarde du côté de `facing`
    /// (l'ordre des sommets est inversé si besoin) : le culling et l'éclairage
    /// voient toujours l'extérieur.
    pub(crate) fn polygon(&mut self, points: &[[f32; 3]], uvs: &[[f32; 2]], layer: Layer, tint: [f32; 3], facing: [f32; 3]) {
        debug_assert_eq!(points.len(), uvs.len());
        if points.len() < 3 {
            return;
        }
        let mut normal = [0.0f32; 3];
        for i in 1..points.len() - 1 {
            let c = cross(sub(points[i], points[0]), sub(points[i + 1], points[0]));
            normal = add(normal, c);
        }
        let len = dot(normal, normal).sqrt();
        if len < 1e-6 {
            return;
        }
        let mut normal = scale(normal, 1.0 / len);
        let flip = dot(normal, facing) < 0.0;
        if flip {
            normal = scale(normal, -1.0);
        }
        let base = self.positions.len() as u32;
        let color = [tint[0], tint[1], tint[2], layer.alpha()];
        for (p, uv) in points.iter().zip(uvs) {
            self.positions.push(*p);
            self.normals.push(normal);
            self.uvs.push(*uv);
            self.colors.push(color);
        }
        for i in 1..points.len() as u32 - 1 {
            if flip {
                self.indices.extend_from_slice(&[base, base + i + 1, base + i]);
            } else {
                self.indices.extend_from_slice(&[base, base + i, base + i + 1]);
            }
        }
    }

    /// Mur vertical de `a` à `b` (plan XZ), de `y0` à `y1`, tourné vers
    /// `outward`. Les motifs sont centrés sur le mur (fenêtres symétriques)
    /// et les étages calés sur `base_y`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn wall(&mut self, a: [f32; 2], b: [f32; 2], y0: f32, y1: f32, base_y: f32, layer: Layer, tint: [f32; 3], outward: [f32; 2]) {
        if y1 - y0 < 1e-3 {
            return;
        }
        let [um, vm] = layer.size_m();
        let length = dist2(a, b);
        let offset = um * 0.5 - length * 0.5;
        let (u0, u1) = (offset / um, (length + offset) / um);
        let v = |y: f32| -(y - base_y) / vm;
        self.polygon(
            &[[a[0], y0, a[1]], [b[0], y0, b[1]], [b[0], y1, b[1]], [a[0], y1, a[1]]],
            &[[u0, v(y0)], [u1, v(y0)], [u1, v(y1)], [u0, v(y1)]],
            layer,
            tint,
            [outward[0], 0.0, outward[1]],
        );
    }

    /// Surface horizontale ou inclinée, texture projetée selon deux axes
    /// du plan (`u_axis`, `v_axis` : vecteurs unitaires XZ).
    pub(crate) fn surface(&mut self, points: &[[f32; 3]], u_axis: [f32; 3], v_axis: [f32; 3], layer: Layer, tint: [f32; 3], facing: [f32; 3]) {
        let [um, vm] = layer.size_m();
        let uvs: Vec<[f32; 2]> = points.iter().map(|p| [dot(*p, u_axis) / um, dot(*p, v_axis) / vm]).collect();
        self.polygon(points, &uvs, layer, tint, facing);
    }
}

// ------------------------------------------------------------------ vecteurs

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Hachage déterministe (splitmix64).
fn hash(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Tirages successifs à partir d'une graine.
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        self.0 = hash(self.0);
        self.0
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[(self.next() % items.len() as u64) as usize]
    }
    fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }
}

// ------------------------------------------------------------- index routier

/// Tronçon de chaussée (deux points consécutifs d'une polyligne).
#[derive(Clone, Copy, Debug)]
struct Piece {
    a: [f32; 2],
    b: [f32; 2],
    /// Demi-largeur chaussée + trottoir.
    reach: f32,
    /// La route dessert-elle les parcelles (pas une voie rapide) ?
    serves: bool,
    /// Demi-largeur du terre-plein planté (boulevards), 0 sinon.
    median: f32,
    /// Abscisses curvilignes des deux bouts du morceau, et longueur de la
    /// route : le terre-plein s'arrête avant les carrefours.
    s: [f32; 2],
    length: f32,
}

/// Index spatial des chaussées : la route la plus proche d'un point.
#[derive(Clone, Debug, Default)]
pub struct RoadIndex {
    pieces: Vec<Piece>,
    buckets: HashMap<(i32, i32), Vec<u32>>,
}

const BUCKET_M: f32 = 32.0;

/// Pas d'arbre sur le terre-plein à moins de cette distance d'un bout de
/// boulevard (le carrefour et son élargissement).
const MEDIAN_END_M: f32 = 22.0;

/// Résultat d'une requête : point le plus proche de l'axe, distance, portée
/// (demi-chaussée + trottoir).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NearestRoad {
    pub point: [f32; 2],
    pub distance: f32,
    pub reach: f32,
}

impl RoadIndex {
    #[must_use]
    pub fn new(strips: &[RoadStrip]) -> Self {
        let mut index = Self::default();
        for strip in strips {
            let profile = crate::roads::Profile::of(strip.spec);
            let reach = profile.reach();
            let serves = strip.spec.class.serves_lots();
            let median = if profile.planted_median { profile.median * 0.5 } else { 0.0 };
            let length: f32 = strip.points.windows(2).map(|w| dist2([w[0][0], w[0][2]], [w[1][0], w[1][2]])).sum();
            let mut walked = 0.0;
            for pair in strip.points.windows(2) {
                let (a, b) = ([pair[0][0], pair[0][2]], [pair[1][0], pair[1][2]]);
                let step = dist2(a, b);
                let piece = Piece { a, b, reach, serves, median, s: [walked, walked + step], length };
                walked += step;
                let id = index.pieces.len() as u32;
                index.pieces.push(piece);
                let (x0, x1) = (piece.a[0].min(piece.b[0]) - reach, piece.a[0].max(piece.b[0]) + reach);
                let (z0, z1) = (piece.a[1].min(piece.b[1]) - reach, piece.a[1].max(piece.b[1]) + reach);
                for bx in (x0 / BUCKET_M).floor() as i32..=(x1 / BUCKET_M).floor() as i32 {
                    for bz in (z0 / BUCKET_M).floor() as i32..=(z1 / BUCKET_M).floor() as i32 {
                        index.buckets.entry((bx, bz)).or_default().push(id);
                    }
                }
            }
        }
        index
    }

    /// Route la plus proche de `p` à moins de `radius` mètres.
    #[must_use]
    pub fn nearest(&self, p: [f32; 2], radius: f32) -> Option<NearestRoad> {
        let mut best: Option<NearestRoad> = None;
        let (bx0, bx1) = (((p[0] - radius) / BUCKET_M).floor() as i32, ((p[0] + radius) / BUCKET_M).floor() as i32);
        let (bz0, bz1) = (((p[1] - radius) / BUCKET_M).floor() as i32, ((p[1] + radius) / BUCKET_M).floor() as i32);
        for bx in bx0..=bx1 {
            for bz in bz0..=bz1 {
                for &id in self.buckets.get(&(bx, bz)).map_or(&[][..], Vec::as_slice) {
                    let piece = self.pieces[id as usize];
                    if !piece.serves {
                        continue;
                    }
                    let q = closest_on_segment(p, piece.a, piece.b);
                    let d = dist2(p, q);
                    if d <= radius && best.is_none_or(|b| d < b.distance) {
                        best = Some(NearestRoad { point: q, distance: d, reach: piece.reach });
                    }
                }
            }
        }
        best
    }

    /// `p` est-il sur le terre-plein planté d'un boulevard, à plus de
    /// `margin` mètres de ses bordures ?
    #[must_use]
    pub fn in_median(&self, p: [f32; 2], margin: f32) -> bool {
        let (bx, bz) = ((p[0] / BUCKET_M).floor() as i32, (p[1] / BUCKET_M).floor() as i32);
        self.buckets.get(&(bx, bz)).is_some_and(|ids| {
            ids.iter().any(|&id| {
                let piece = self.pieces[id as usize];
                let q = closest_on_segment(p, piece.a, piece.b);
                let t = dist2(piece.a, q) / dist2(piece.a, piece.b).max(1e-6);
                let s = piece.s[0] + (piece.s[1] - piece.s[0]) * t;
                // Loin des bouts : aux carrefours, le terre-plein laisse place à la chaussée.
                let interior = s > MEDIAN_END_M && s < piece.length - MEDIAN_END_M;
                piece.median > margin && interior && dist2(p, q) < piece.median - margin
            })
        })
    }

    /// Distance de `p` au bord extérieur du trottoir le plus proche (négative
    /// si `p` est sur la chaussée ou le trottoir).
    #[must_use]
    pub fn clearance(&self, p: [f32; 2], radius: f32) -> f32 {
        let mut best = radius;
        let (bx0, bx1) = (((p[0] - radius) / BUCKET_M).floor() as i32, ((p[0] + radius) / BUCKET_M).floor() as i32);
        let (bz0, bz1) = (((p[1] - radius) / BUCKET_M).floor() as i32, ((p[1] + radius) / BUCKET_M).floor() as i32);
        for bx in bx0..=bx1 {
            for bz in bz0..=bz1 {
                for &id in self.buckets.get(&(bx, bz)).map_or(&[][..], Vec::as_slice) {
                    let piece = self.pieces[id as usize];
                    let d = dist2(p, closest_on_segment(p, piece.a, piece.b)) - piece.reach;
                    best = best.min(d);
                }
            }
        }
        best
    }
}

fn closest_on_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    if len2 < 1e-9 {
        return a;
    }
    let t = (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0);
    [a[0] + ab[0] * t, a[1] + ab[1] * t]
}

// ------------------------------------------------------------------ parcelles

/// Repère d'une parcelle : centre, axe avant (vers la route), axe droit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lot {
    pub tile: TileId,
    pub center: [f32; 2],
    pub front: [f32; 2],
    pub right: [f32; 2],
    /// Profondeur utilisable derrière le centre (m) et devant (jusqu'au trottoir).
    pub back: f32,
    pub ahead: f32,
    /// Demi-largeur de la parcelle.
    pub half_width: f32,
}

impl Lot {
    /// Point du plan à `r` mètres à droite et `f` mètres devant le centre.
    #[must_use]
    pub fn at(&self, r: f32, f: f32) -> [f32; 2] {
        [
            self.center[0] + self.right[0] * r + self.front[0] * f,
            self.center[1] + self.right[1] * r + self.front[1] * f,
        ]
    }

    #[must_use]
    pub fn depth(&self) -> f32 {
        self.back + self.ahead
    }
}

/// Parcelle d'un bâtiment : orientation vers la route la plus proche et
/// limite avant au trottoir.
#[must_use]
pub fn lot_for(world: &World, roads: &RoadIndex, tile: TileId) -> Option<Lot> {
    let grid = world.zoning().grid();
    let (x, y) = grid.xy(tile)?;
    let size = world.config().tile_size_m as f32;
    let center = [(x as f32 + 0.5) * size, (y as f32 + 0.5) * size];
    let half = size * 0.5;

    let near = roads.nearest(center, 40.0);
    let front = match near {
        Some(road) => {
            let v = [road.point[0] - center[0], road.point[1] - center[1]];
            if v[0].abs() > v[1].abs() { [v[0].signum(), 0.0] } else { [0.0, v[1].signum()] }
        }
        None => [0.0, 1.0],
    };
    let right = [-front[1], front[0]];

    // Jardin de devant : la parcelle s'étend sur la tuile avant si elle est
    // libre (pas de zonage, pas de bâtiment).
    let fx = x as i64 + front[0] as i64;
    let fy = y as i64 + front[1] as i64;
    let free_ahead = u32::try_from(fx)
        .ok()
        .zip(u32::try_from(fy).ok())
        .and_then(|(fx, fy)| grid.tile_at(fx, fy))
        .is_some_and(|t| grid.get(t) == Some(&ZoneType::Unzoned) && world.buildings().at(t).is_none());
    let mut ahead = if free_ahead { half + size } else { half };
    if let Some(road) = near {
        // Distance au trottoir le long de l'axe avant.
        let along = (road.point[0] - center[0]) * front[0] + (road.point[1] - center[1]) * front[1];
        ahead = ahead.min(along - road.reach - 0.2);
    }
    let mut ahead = ahead.max(-half + 3.0);

    // Les autres bords reculent devant toute route qui les longe (boulevard
    // en biais, carrefour) : on échantillonne chaque bord et on le rentre
    // jusqu'à dégager le trottoir. Le centre glisse si un seul côté recule.
    let (mut left, mut right_ext, mut back) = (half - 0.1, half - 0.1, half - 0.1);
    let point = |r: f32, f: f32| {
        [center[0] + right[0] * r + front[0] * f, center[1] + right[1] * r + front[1] * f]
    };
    let margin = 0.3;
    for _ in 0..3 {
        let side_clearance = |pts: [[f32; 2]; 3]| pts.iter().map(|p| roads.clearance(*p, 30.0)).fold(f32::INFINITY, f32::min);
        let mid_f = (ahead - back) * 0.5;
        let mid_r = (right_ext - left) * 0.5;
        let c_left = side_clearance([point(-left, -back), point(-left, mid_f), point(-left, ahead)]);
        let c_right = side_clearance([point(right_ext, -back), point(right_ext, mid_f), point(right_ext, ahead)]);
        let c_back = side_clearance([point(-left, -back), point(mid_r, -back), point(right_ext, -back)]);
        let c_front = side_clearance([point(-left, ahead), point(mid_r, ahead), point(right_ext, ahead)]);
        left -= (margin - c_left).max(0.0);
        right_ext -= (margin - c_right).max(0.0);
        back -= (margin - c_back).max(0.0);
        ahead -= (margin - c_front).max(0.0);
    }
    if left + right_ext < 3.0 || back + ahead < 3.0 {
        return None;
    }
    let shift = (right_ext - left) * 0.5;
    let center = point(shift, 0.0);
    Some(Lot { tile, center, front, right, back, ahead, half_width: (left + right_ext) * 0.5 })
}

// ------------------------------------------------------------------ bâtiments

/// Archétype visuel d'un bâtiment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Archetype {
    /// Pavillon d'un ou deux niveaux, toit en pente, jardin et haies.
    House,
    /// Maison de ville de trois niveaux.
    Townhouse,
    /// Immeuble d'habitation à balcons, toit plat.
    Apartments,
    /// Commerce : vitrine au rez-de-chaussée, logements ou bureaux au-dessus.
    Shop,
    /// Immeuble de bureaux vitré sur rez commercial.
    Offices,
    /// Atelier ou usine en bardage métallique.
    Factory,
}

/// Archétype et nombre d'étages (hors toit) d'après le type et la capacité.
#[must_use]
pub fn archetype(kind: BuildingKind, capacity: u32) -> (Archetype, u32) {
    match kind {
        BuildingKind::Household if capacity <= 14 => (Archetype::House, 1),
        BuildingKind::Household if capacity <= 26 => (Archetype::House, 2),
        BuildingKind::Household if capacity <= 36 => (Archetype::Townhouse, 3),
        BuildingKind::Household => (Archetype::Apartments, (3 + (capacity - 36) / 4).min(6)),
        BuildingKind::Shop if capacity <= 22 => (Archetype::Shop, (capacity.saturating_sub(4) / 6).min(3)),
        BuildingKind::Shop => (Archetype::Offices, (3 + (capacity - 22) / 2).min(8)),
        BuildingKind::Factory => (Archetype::Factory, 1),
    }
}

/// Hauteur totale approximative (m), toit compris — pour les tests et le cadrage.
#[must_use]
pub fn archetype_height_m(archetype: Archetype, floors: u32) -> f32 {
    let f = floors as f32;
    match archetype {
        Archetype::House | Archetype::Townhouse => f * FLOOR_M + 3.5,
        Archetype::Apartments => f * FLOOR_M + 1.0,
        Archetype::Shop | Archetype::Offices => SHOP_FLOOR_M + f * FLOOR_M + 0.8,
        Archetype::Factory => 8.0 + 1.0,
    }
}

const PLASTER_TINTS: [[f32; 3]; 6] = [
    [1.0, 0.97, 0.9],
    [1.0, 0.95, 0.82],
    [0.98, 0.9, 0.84],
    [0.92, 0.95, 0.98],
    [1.0, 1.0, 1.0],
    [0.95, 0.92, 0.86],
];
const SIDING_TINTS: [[f32; 3]; 5] = [
    [1.0, 1.0, 1.0],
    [0.85, 0.92, 1.0],
    [0.88, 0.96, 0.88],
    [1.0, 0.94, 0.84],
    [0.8, 0.8, 0.82],
];
const ROOF_TINTS: [[f32; 3]; 4] = [[1.0, 1.0, 1.0], [0.85, 0.75, 0.72], [0.7, 0.62, 0.6], [1.05, 0.95, 0.85]];
const WHITE: [f32; 3] = [1.0, 1.0, 1.0];

/// Rectangle d'emprise dans le repère de la parcelle : de `r0` à `r1` (droite),
/// de `f0` à `f1` (avant).
#[derive(Clone, Copy, Debug)]
struct Footprint {
    r0: f32,
    r1: f32,
    f0: f32,
    f1: f32,
}

impl Footprint {
    fn corners(&self, lot: &Lot) -> [[f32; 2]; 4] {
        // Sens trigonométrique vu de dessus : arrière-gauche, arrière-droit,
        // avant-droit, avant-gauche.
        [lot.at(self.r0, self.f0), lot.at(self.r1, self.f0), lot.at(self.r1, self.f1), lot.at(self.r0, self.f1)]
    }
    fn center(&self, lot: &Lot) -> [f32; 2] {
        lot.at((self.r0 + self.r1) * 0.5, (self.f0 + self.f1) * 0.5)
    }
    fn width(&self) -> f32 {
        self.r1 - self.r0
    }
    fn depth(&self) -> f32 {
        self.f1 - self.f0
    }
}

/// Murs extérieurs d'une emprise, de `y0` à `y1`. `front_layer` habille la
/// façade avant, `side_layer` les trois autres.
#[allow(clippy::too_many_arguments)]
fn walls(mesh: &mut UrbanMesh, lot: &Lot, fp: &Footprint, y0: f32, y1: f32, base: f32, front_layer: Layer, side_layer: Layer, tint: [f32; 3]) {
    let c = fp.center(lot);
    let corners = fp.corners(lot);
    for i in 0..4 {
        let (a, b) = (corners[i], corners[(i + 1) % 4]);
        let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
        let out = [mid[0] - c[0], mid[1] - c[1]];
        let layer = if i == 2 { front_layer } else { side_layer };
        mesh.wall(a, b, y0, y1, base, layer, tint, out);
    }
}

/// Toit plat à acrotère : dalle, relevés intérieurs et extérieurs, couvertine.
fn flat_roof(mesh: &mut UrbanMesh, lot: &Lot, fp: &Footprint, top: f32, dice: &mut Dice) {
    let parapet = 0.7;
    let inset = 0.25;
    let outer = fp.corners(lot);
    let inner_fp = Footprint { r0: fp.r0 + inset, r1: fp.r1 - inset, f0: fp.f0 + inset, f1: fp.f1 - inset };
    let inner = inner_fp.corners(lot);
    let up = [0.0, 1.0, 0.0];
    let slab: Vec<[f32; 3]> = inner.iter().map(|p| [p[0], top, p[1]]).collect();
    mesh.surface(&slab, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], Layer::FlatRoof, WHITE, up);
    let c = fp.center(lot);
    for i in 0..4 {
        let (a, b) = (outer[i], outer[(i + 1) % 4]);
        let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
        let out = [mid[0] - c[0], mid[1] - c[1]];
        mesh.wall(a, b, top, top + parapet, top, Layer::Concrete, WHITE, out);
        let (ia, ib) = (inner[i], inner[(i + 1) % 4]);
        mesh.wall(ia, ib, top, top + parapet, top, Layer::Concrete, WHITE, [-out[0], -out[1]]);
        let cap = [[a[0], top + parapet, a[1]], [b[0], top + parapet, b[1]], [ib[0], top + parapet, ib[1]], [ia[0], top + parapet, ia[1]]];
        mesh.surface(&cap, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], Layer::Concrete, WHITE, up);
    }
    // Édicule technique (cage d'escalier, machinerie) sur les grands toits.
    if fp.width() > 6.0 && dice.chance(0.3) {
        let w = dice.range(1.6, 2.6);
        let r = dice.range(fp.r0 + 1.0, fp.r1 - 1.0 - w);
        let f = dice.range(fp.f0 + 1.0, fp.f1 - 3.0);
        let block = Footprint { r0: r, r1: r + w, f0: f, f1: f + 2.0 };
        cuboid(mesh, lot, &block, top, top + dice.range(1.6, 2.6), Layer::Concrete, Layer::FlatRoof, [0.92, 0.92, 0.9]);
    }
}

/// Pavé : quatre murs et un dessus.
#[allow(clippy::too_many_arguments)]
fn cuboid(mesh: &mut UrbanMesh, lot: &Lot, fp: &Footprint, y0: f32, y1: f32, side: Layer, top_layer: Layer, tint: [f32; 3]) {
    walls(mesh, lot, fp, y0, y1, y0, side, side, tint);
    let top: Vec<[f32; 3]> = fp.corners(lot).iter().map(|p| [p[0], y1, p[1]]).collect();
    mesh.surface(&top, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], top_layer, tint, [0.0, 1.0, 0.0]);
}

/// Toit en pente au-dessus d'une emprise : à deux pans (faîtage parallèle à
/// la rue, pignons sur les côtés) ou en croupe (quatre pans).
#[allow(clippy::too_many_arguments)]
fn pitched_roof(
    mesh: &mut UrbanMesh,
    lot: &Lot,
    fp: &Footprint,
    eave: f32,
    pitch: f32,
    hip: bool,
    roof: Layer,
    roof_tint: [f32; 3],
    gable: Layer,
    wall_tint: [f32; 3],
    base: f32,
) {
    let overhang = 0.4;
    let slope = pitch.tan();
    let half_depth = fp.depth() * 0.5;
    let rise = half_depth * slope;
    let ridge_y = eave + rise;
    let fm = (fp.f0 + fp.f1) * 0.5;
    let low = eave - overhang * slope;
    let (r0, r1) = (fp.r0 - overhang, fp.r1 + overhang);
    let (f0, f1) = (fp.f0 - overhang, fp.f1 + overhang);
    // Croupe : le faîtage est raccourci de la demi-profondeur à chaque bout.
    let (k0, k1) = if hip {
        let inset = (half_depth + overhang).min(fp.width() * 0.5 + overhang - 0.05);
        (r0 + inset, r1 - inset)
    } else {
        (r0, r1)
    };
    let p = |r: f32, f: f32, y: f32| {
        let q = lot.at(r, f);
        [q[0], y, q[1]]
    };
    let dir3 = |v: [f32; 2]| [v[0], 0.0, v[1]];
    let along = dir3(lot.right);
    let slope_len = (rise + overhang * slope).hypot(half_depth + overhang);
    let roof_uv = |pts: &[[f32; 3]], down: [f32; 2]| -> Vec<[f32; 2]> {
        let [um, vm] = roof.size_m();
        pts.iter()
            .map(|q| {
                let ridge_point = lot.at(0.0, fm);
                let horizontal = (q[0] - ridge_point[0]) * down[0] + (q[2] - ridge_point[1]) * down[1];
                let s = horizontal / (half_depth + overhang) * slope_len;
                [dot(*q, along) / um, s / vm]
            })
            .collect()
    };
    // Pan avant et pan arrière.
    for (edge, sign) in [(f1, 1.0f32), (f0, -1.0)] {
        let pts = [p(r0, edge, low), p(r1, edge, low), p(k1, fm, ridge_y), p(k0, fm, ridge_y)];
        let down = [lot.front[0] * sign, lot.front[1] * sign];
        let uv = roof_uv(&pts, down);
        let facing = [down[0] * slope, 1.0, down[1] * slope];
        mesh.polygon(&pts, &uv, roof, roof_tint, facing);
    }
    if hip {
        // Croupes latérales (triangles).
        for (edge, k, sign) in [(r0, k0, -1.0f32), (r1, k1, 1.0)] {
            let pts = [p(edge, f0, low), p(edge, f1, low), p(k, fm, ridge_y)];
            let side = [lot.right[0] * sign, lot.right[1] * sign];
            let [um, vm] = roof.size_m();
            let along_f = dir3(lot.front);
            let uv: Vec<[f32; 2]> = pts
                .iter()
                .map(|q| {
                    let s = (q[0] * side[0] + q[2] * side[1]) / um;
                    [dot(*q, along_f) / um, s * um / vm * slope_len / (half_depth + overhang)]
                })
                .collect();
            mesh.polygon(&pts, &uv, roof, roof_tint, [side[0] * slope, 1.0, side[1] * slope]);
        }
    } else {
        // Pignons : triangles de mur (sans fenêtres) sous les rives.
        for (edge, sign) in [(fp.r0, -1.0f32), (fp.r1, 1.0)] {
            let pts = [p(edge, fp.f0, eave), p(edge, fp.f1, eave), p(edge, fm, ridge_y)];
            let [um, vm] = gable.size_m();
            let along_f = dir3(lot.front);
            let uv: Vec<[f32; 2]> = pts.iter().map(|q| [dot(*q, along_f) / um, -(q[1] - base) / vm]).collect();
            let out = [lot.right[0] * sign, 0.0, lot.right[1] * sign];
            mesh.polygon(&pts, &uv, gable, wall_tint, out);
        }
    }
}

/// Socle : du terrain (le plus bas de l'emprise, un peu enterré) au plancher.
fn plinth(world: &World, mesh: &mut UrbanMesh, lot: &Lot, fp: &Footprint, floor: f32) {
    let low = fp
        .corners(lot)
        .iter()
        .map(|c| ground_altitude_m(world, c[0], c[1]))
        .fold(f32::INFINITY, f32::min);
    walls(mesh, lot, fp, low - 1.0, floor, floor, Layer::Concrete, Layer::Concrete, [0.85, 0.84, 0.82]);
}

/// Altitude du plancher : le point le plus haut de l'emprise + une marche.
fn floor_level(world: &World, lot: &Lot, fp: &Footprint) -> f32 {
    fp.corners(lot)
        .iter()
        .map(|c| ground_altitude_m(world, c[0], c[1]))
        .fold(f32::NEG_INFINITY, f32::max)
        + 0.3
}

/// Sol d'une zone de la parcelle, épousant le terrain (grille 3×3).
fn ground_patch(world: &World, mesh: &mut UrbanMesh, lot: &Lot, area: &Footprint, layer: Layer, tint: [f32; 3], lift: f32) {
    if area.width() <= 0.05 || area.depth() <= 0.05 {
        return;
    }
    let rs = [area.r0, (area.r0 + area.r1) * 0.5, area.r1];
    let fs = [area.f0, (area.f0 + area.f1) * 0.5, area.f1];
    let point = |i: usize, j: usize| {
        let q = lot.at(rs[i], fs[j]);
        [q[0], ground_altitude_m(world, q[0], q[1]) + lift, q[1]]
    };
    for i in 0..2 {
        for j in 0..2 {
            let pts = [point(i, j), point(i + 1, j), point(i + 1, j + 1), point(i, j + 1)];
            mesh.surface(&pts, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], layer, tint, [0.0, 1.0, 0.0]);
        }
    }
}

/// Haie (pavé feuillu) le long d'un côté de parcelle, posée sur le terrain.
fn hedge(world: &World, mesh: &mut UrbanMesh, lot: &Lot, area: Footprint, height: f32) {
    let corners = area.corners(lot);
    let low = corners.iter().map(|c| ground_altitude_m(world, c[0], c[1])).fold(f32::INFINITY, f32::min);
    let high = corners.iter().map(|c| ground_altitude_m(world, c[0], c[1])).fold(f32::NEG_INFINITY, f32::max);
    let tint = [0.42, 0.58, 0.34];
    walls(mesh, lot, &area, low - 0.3, high + height, 0.0, Layer::Lawn, Layer::Lawn, tint);
    let top: Vec<[f32; 3]> = corners.iter().map(|p| [p[0], high + height, p[1]]).collect();
    mesh.surface(&top, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], Layer::Lawn, tint, [0.0, 1.0, 0.0]);
}

/// Porte d'entrée plaquée sur la façade avant, centrée en `r`.
fn front_door(mesh: &mut UrbanMesh, lot: &Lot, r: f32, f: f32, floor: f32) {
    let [w, h] = Layer::Door.size_m();
    let a = lot.at(r - w * 0.5, f + 0.03);
    let b = lot.at(r + w * 0.5, f + 0.03);
    mesh.polygon(
        &[[a[0], floor, a[1]], [b[0], floor, b[1]], [b[0], floor + h, b[1]], [a[0], floor + h, a[1]]],
        &[[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
        Layer::Door,
        WHITE,
        [lot.front[0], 0.0, lot.front[1]],
    );
}

/// Construit un bâtiment complet (parcelle comprise) dans `mesh`.
///
/// Un bâtiment en chantier (`stage` autre que `Complete`) montre son stade :
/// palissade et engins, dalle, structure qui monte sous échafaudages et grue,
/// puis bâtiment fini encore échafaudé. `within` : avancement dans le stade.
pub fn push_building(world: &World, mesh: &mut UrbanMesh, lot: &Lot, kind: BuildingKind, capacity: u32, stage: ConstructionStage, within: f32) {
    if stage != ConstructionStage::Complete {
        construction_site(world, mesh, lot, kind, capacity, stage, within);
        return;
    }
    finished_building(world, mesh, lot, kind, capacity);
}

fn finished_building(world: &World, mesh: &mut UrbanMesh, lot: &Lot, kind: BuildingKind, capacity: u32) {
    let (arch, floors) = archetype(kind, capacity);
    let mut dice = building_dice(lot.tile);
    match arch {
        Archetype::House | Archetype::Townhouse => house(world, mesh, lot, arch, floors, &mut dice),
        Archetype::Apartments => apartments(world, mesh, lot, floors, &mut dice),
        Archetype::Shop | Archetype::Offices => shop(world, mesh, lot, arch, floors, &mut dice),
        Archetype::Factory => factory(world, mesh, lot, &mut dice),
    }
}

/// Emprise, plancher et hauteur de l'enveloppe d'un bâtiment : mêmes tirages
/// que sa construction finale (le chantier montre le bon volume).
fn shell(world: &World, lot: &Lot, arch: Archetype, floors: u32) -> (Footprint, f32, f32) {
    let mut dice = building_dice(lot.tile);
    let fp = match arch {
        Archetype::House | Archetype::Townhouse => house_plan(lot, arch == Archetype::Townhouse, &mut dice),
        Archetype::Apartments => {
            let depth = 7.2f32.min(lot.depth() - 0.5);
            let front = lot.ahead - dice.range(0.3, 1.0);
            Footprint { r0: -3.7, r1: 3.7, f0: front - depth, f1: front }
        }
        Archetype::Shop | Archetype::Offices => {
            let depth = 7.4f32.min(lot.depth() - 0.3);
            let front = lot.ahead - 0.2;
            Footprint { r0: -3.8, r1: 3.8, f0: front - depth, f1: front }
        }
        Archetype::Factory => {
            let depth = 7.6f32.min(lot.depth() - 0.3);
            let front = lot.ahead - dice.range(0.2, 0.8);
            Footprint { r0: -3.85, r1: 3.85, f0: front - depth, f1: front }
        }
    };
    let floor = floor_level(world, lot, &fp);
    let height = match arch {
        Archetype::House | Archetype::Townhouse | Archetype::Apartments => floors as f32 * FLOOR_M,
        Archetype::Shop | Archetype::Offices => SHOP_FLOOR_M + floors as f32 * FLOOR_M,
        Archetype::Factory => 8.0,
    };
    (fp, floor, floor + height)
}

/// Tube d'échafaudage (ou poteau) de section carrée `2r`, de `a` à `b`.
fn tube(mesh: &mut UrbanMesh, a: [f32; 3], b: [f32; 3], r: f32, layer: Layer, tint: [f32; 3]) {
    let axis = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len = dot(axis, axis).sqrt();
    if len < 1e-4 {
        return;
    }
    let ax = scale(axis, 1.0 / len);
    let helper = if ax[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
    let u = cross(ax, helper);
    let u = scale(u, r / dot(u, u).sqrt());
    let w = cross(ax, u);
    let w = scale(w, r / dot(w, w).sqrt());
    let corners = [add(u, w), add(scale(u, -1.0), w), add(scale(u, -1.0), scale(w, -1.0)), add(u, scale(w, -1.0))];
    for i in 0..4 {
        let (c0, c1) = (corners[i], corners[(i + 1) % 4]);
        let quad = [add(a, c0), add(b, c0), add(b, c1), add(a, c1)];
        let out = add(c0, c1);
        let [um, vm] = layer.size_m();
        let uv = [[0.0, 0.0], [len / um, 0.0], [len / um, r / vm], [0.0, r / vm]];
        mesh.polygon(&quad, &uv, layer, tint, out);
    }
}

/// Échafaudage de façade autour d'une emprise, jusqu'à `height` au-dessus
/// du plancher : deux files de montants, moises à chaque niveau (2 m),
/// planchers de travail.
fn scaffolding(mesh: &mut UrbanMesh, lot: &Lot, fp: &Footprint, floor: f32, height: f32) {
    let (gap, depth) = (0.35, 0.85);
    let steel = [0.55, 0.57, 0.6];
    let lifts = (height / 2.0).ceil().max(1.0) as usize;
    for side in 0..4 {
        // Côté de l'emprise (repère parcelle), prolongé aux angles.
        let (r0, r1, f0, f1, normal) = match side {
            0 => (fp.r0 - gap - depth, fp.r1 + gap + depth, fp.f0, fp.f0, [0.0, -1.0]),
            1 => (fp.r1, fp.r1, fp.f0 - gap - depth, fp.f1 + gap + depth, [1.0, 0.0]),
            2 => (fp.r0 - gap - depth, fp.r1 + gap + depth, fp.f1, fp.f1, [0.0, 1.0]),
            _ => (fp.r0, fp.r0, fp.f0 - gap - depth, fp.f1 + gap + depth, [-1.0, 0.0]),
        };
        let at = |t: f32, off: f32, y: f32| {
            let r = r0 + (r1 - r0) * t + normal[0] * off;
            let f = f0 + (f1 - f0) * t + normal[1] * off;
            let q = lot.at(r, f);
            [q[0], y, q[1]]
        };
        let length = (r1 - r0).abs().max((f1 - f0).abs());
        let bays = (length / 2.5).ceil().max(1.0) as usize;
        for off in [gap, gap + depth] {
            for k in 0..=bays {
                let t = k as f32 / bays as f32;
                tube(mesh, at(t, off, floor - 0.3), at(t, off, floor + height + 1.0), 0.03, Layer::Concrete, steel);
            }
            for l in 1..=lifts {
                let y = floor + l as f32 * 2.0;
                tube(mesh, at(0.0, off, y), at(1.0, off, y), 0.025, Layer::Concrete, steel);
                // Garde-corps au-dessus du plancher, côté extérieur.
                if off > gap {
                    tube(mesh, at(0.0, off, y + 1.0), at(1.0, off, y + 1.0), 0.02, Layer::Concrete, steel);
                }
            }
        }
        for l in 1..=lifts {
            let y = floor + l as f32 * 2.0 + 0.03;
            let deck = [at(0.0, gap, y), at(1.0, gap, y), at(1.0, gap + depth, y), at(0.0, gap + depth, y)];
            mesh.surface(&deck, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], Layer::Siding, [0.95, 0.72, 0.45], [0.0, 1.0, 0.0]);
        }
    }
}

/// Palissade le long du pourtour de la parcelle, avec un portail à l'avant.
fn hoarding(world: &World, mesh: &mut UrbanMesh, lot: &Lot) {
    let inset = 0.15;
    let (r0, r1) = (-lot.half_width + inset, lot.half_width - inset);
    let (f0, f1) = (-lot.back + inset, lot.ahead - inset);
    let [panel, height] = Layer::Hoarding.size_m();
    // (début, fin, normale extérieure) de chaque côté, en repère parcelle.
    let sides = [
        ([r0, f0], [r1, f0], [0.0, -1.0]),
        ([r1, f0], [r1, f1], [1.0, 0.0]),
        ([r1, f1], [r0, f1], [0.0, 1.0]),
        ([r0, f1], [r0, f0], [-1.0, 0.0]),
    ];
    for (k, (a, b, n)) in sides.iter().enumerate() {
        let length = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let count = (length / panel).ceil().max(1.0) as usize;
        for i in 0..count {
            let (t0, t1) = (i as f32 / count as f32, (i + 1) as f32 / count as f32);
            // Portail : deux panneaux ouverts au milieu du côté avant.
            if k == 2 && count >= 3 && (i == count / 2 || (count % 2 == 0 && i + 1 == count / 2)) {
                continue;
            }
            let p0 = lot.at(a[0] + (b[0] - a[0]) * t0, a[1] + (b[1] - a[1]) * t0);
            let p1 = lot.at(a[0] + (b[0] - a[0]) * t1, a[1] + (b[1] - a[1]) * t1);
            let g0 = ground_altitude_m(world, p0[0], p0[1]);
            let g1 = ground_altitude_m(world, p1[0], p1[1]);
            let low = g0.min(g1) - 0.2;
            let top = g0.max(g1) + height;
            let out = [lot.right[0] * n[0] + lot.front[0] * n[1], lot.right[1] * n[0] + lot.front[1] * n[1]];
            mesh.wall(p0, p1, low, top, top - height, Layer::Hoarding, WHITE, out);
            mesh.wall(p1, p0, low, top, top - height, Layer::Hoarding, WHITE, [-out[0], -out[1]]);
        }
    }
}

/// Chantier : sol de terre, palissade, et ce que montre le stade.
#[allow(clippy::too_many_arguments)]
fn construction_site(world: &World, mesh: &mut UrbanMesh, lot: &Lot, kind: BuildingKind, capacity: u32, stage: ConstructionStage, within: f32) {
    let (arch, floors) = archetype(kind, capacity);
    let (fp, floor, top) = shell(world, lot, arch, floors);
    let mut dice = Dice(hash(u64::from(lot.tile.raw()) ^ 0x517E_C0DE));
    let whole = Footprint { r0: -lot.half_width, r1: lot.half_width, f0: -lot.back, f1: lot.ahead };
    ground_patch(world, mesh, lot, &whole, Layer::Works, WHITE, 0.09);
    hoarding(world, mesh, lot);
    let models = crate::site::SiteModels::get();
    let big = top - floor > 7.0;
    // Une grue dessert plusieurs parcelles : une sur six environ en a une.
    let crane = big && dice.chance(0.17);
    let excavator = dice.chance(0.5);
    let cabin = big && dice.chance(0.3);
    let ground_at = |r: f32, f: f32| {
        let q = lot.at(r, f);
        [q[0], ground_altitude_m(world, q[0], q[1]), q[1]]
    };
    // Orientation des engins : le long de la façade, en radians autour de Y.
    let along = lot.right[1].atan2(lot.right[0]);
    let crane_spot = || {
        let q = lot.at(fp.r0 + 1.3, fp.f0 + 1.3);
        [q[0], floor - 0.3, q[1]]
    };
    match stage {
        ConstructionStage::SitePreparation | ConstructionStage::Foundations => {
            // Terrassement : la pelleteuse travaille, le bungalow de chantier
            // attend sur les grands chantiers.
            // Le bras (6 m vers +X du modèle) travaille vers le fond de la
            // parcelle : la machine reste chez elle.
            let back_yaw = (-lot.front[1]).atan2(-lot.front[0]);
            let ef = (lot.ahead - 3.1).min(fp.f1);
            if excavator {
                push_excavator(mesh, ground_at(dice.range(-0.8, 0.8), ef), back_yaw + dice.range(-0.15, 0.15));
            }
            if cabin {
                let q = ground_at(0.0, -lot.back + 1.4);
                crate::site::push_prop(mesh, &models.cabin, q, -along);
            }
            if stage == ConstructionStage::Foundations {
                // Dalle coulée progressivement depuis l'arrière, coffrage autour.
                let poured = Footprint { f1: fp.f0 + (fp.f1 - fp.f0) * within.max(0.15), ..fp };
                cuboid(mesh, lot, &poured, floor - 0.8, floor, Layer::Concrete, Layer::Concrete, [0.78, 0.77, 0.74]);
                walls(mesh, lot, &Footprint { r0: fp.r0 - 0.1, r1: fp.r1 + 0.1, f0: fp.f0 - 0.1, f1: fp.f1 + 0.1 }, floor - 0.8, floor + 0.25, floor, Layer::Siding, Layer::Siding, [0.85, 0.65, 0.4]);
            }
        }
        ConstructionStage::Structure => {
            let built = floor + (top - floor) * within.clamp(0.05, 1.0);
            cuboid(mesh, lot, &fp, floor - 0.8, floor, Layer::Concrete, Layer::Concrete, [0.78, 0.77, 0.74]);
            // Murs montés jusqu'au niveau sous la dalle en cours.
            let level_h = if matches!(arch, Archetype::Shop | Archetype::Offices) { SHOP_FLOOR_M } else { FLOOR_M };
            let walls_to = (built - FLOOR_M).max(floor);
            let block = if matches!(arch, Archetype::House | Archetype::Townhouse) { (Layer::Brick, [0.95, 0.9, 0.85]) } else { (Layer::Concrete, [0.86, 0.85, 0.82]) };
            if walls_to > floor + 0.5 {
                walls(mesh, lot, &fp, floor, walls_to, floor, block.0, block.0, block.1);
            }
            // Dalles des niveaux atteints et poteaux jusqu'au front de pose.
            let mut y = floor + level_h;
            while y <= built + 0.01 {
                cuboid(mesh, lot, &fp, y - 0.25, y, Layer::Concrete, Layer::Concrete, [0.8, 0.79, 0.76]);
                y += FLOOR_M;
            }
            for (r, f) in [(fp.r0, fp.f0), (fp.r1, fp.f0), (fp.r1, fp.f1), (fp.r0, fp.f1), ((fp.r0 + fp.r1) * 0.5, fp.f0), ((fp.r0 + fp.r1) * 0.5, fp.f1)] {
                let col = Footprint { r0: r - 0.18, r1: r + 0.18, f0: f - 0.18, f1: f + 0.18 };
                let col = Footprint { r0: col.r0.max(fp.r0), r1: col.r1.min(fp.r1), f0: col.f0.max(fp.f0), f1: col.f1.min(fp.f1) };
                cuboid(mesh, lot, &col, floor, built, Layer::Concrete, Layer::Concrete, [0.8, 0.79, 0.76]);
            }
            scaffolding(mesh, lot, &fp, floor, built - floor);
            if crane {
                crate::site::push_crane(mesh, crane_spot(), top - floor + 9.0, dice.range(0.0, std::f32::consts::TAU));
            }
        }
        ConstructionStage::Finishing | ConstructionStage::Complete => {
            finished_building(world, mesh, lot, kind, capacity);
            // Les échafaudages descendent à mesure que les façades sont finies.
            if within < 0.75 {
                scaffolding(mesh, lot, &fp, floor, top - floor);
            }
            if crane && within < 0.35 {
                crate::site::push_crane(mesh, crane_spot(), top - floor + 9.0, dice.range(0.0, std::f32::consts::TAU));
            }
        }
    }
}

fn push_excavator(mesh: &mut UrbanMesh, at: [f32; 3], yaw: f32) {
    crate::site::push_prop(mesh, &crate::site::SiteModels::get().excavator, at, yaw);
}

/// Emprise d'une maison : premiers tirages de sa graine (partagés avec
/// `tree_allowed`, qui doit savoir où commence le jardin de derrière).
fn house_plan(lot: &Lot, townhouse: bool, dice: &mut Dice) -> Footprint {
    let width = if townhouse { 7.0 } else { dice.pick(&[6.0, 6.0, 6.6]) };
    let depth = (if townhouse { 7.0 } else { dice.range(5.6, 6.8) }).min(lot.depth() - 1.0);
    // Recul : un jardin de devant si la parcelle le permet.
    let front = (lot.ahead - dice.range(HOUSE_SETBACK_M, HOUSE_SETBACK_M + 1.3)).max(-lot.back + depth + 0.3).min(lot.ahead - 0.3);
    Footprint { r0: -width * 0.5, r1: width * 0.5, f0: front - depth, f1: front }
}

/// Bande des arbres d'alignement : distance au bord du trottoir (m).
const STREET_TREE_CLEARANCE: (f32, f32) = (0.6, 1.3);

/// Recul minimal d'une maison derrière le trottoir : laisse la place aux
/// arbres d'alignement.
const HOUSE_SETBACK_M: f32 = 2.2;

fn building_dice(tile: TileId) -> Dice {
    Dice(hash(u64::from(tile.raw()) ^ 0xC17E_B11D))
}

/// Un arbre de la zone jouable a-t-il sa place en `(x, z)` (tuile `tile`) ?
///
/// - loin des routes, sur une tuile libre : oui (bosquets entre les îlots) ;
/// - près d'une route, sur une tuile libre : seulement en alignement, juste
///   derrière le trottoir — l'accotement reste un jardin de devant dégagé ;
/// - dans le jardin de derrière d'un pavillon : oui, à distance de la maison ;
/// - sur une tuile zonée encore vide ou un autre bâtiment : non.
#[must_use]
pub fn tree_allowed(world: &World, roads: &RoadIndex, x: f32, z: f32, tile: TileId) -> bool {
    let grid = world.zoning().grid();
    if roads.in_median([x, z], 0.8) {
        return true;
    }
    let clearance = roads.clearance([x, z], 12.0);
    if clearance < STREET_TREE_CLEARANCE.0 {
        return false;
    }
    let Some(id) = world.buildings().at(tile) else {
        if grid.get(tile) != Some(&ZoneType::Unzoned) {
            return false;
        }
        // Voisines bâties : pas d'arbre à moins de 2,5 m de leur limite ; et
        // pas d'alignement devant un commerce ou une usine (ils avancent
        // jusqu'au trottoir).
        let Some((tx, ty)) = grid.xy(tile) else { return false };
        let size = world.config().tile_size_m as f32;
        let (cx, cz) = ((tx as f32 + 0.5) * size, (ty as f32 + 0.5) * size);
        let mut beside_shop = false;
        for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
            let (nx, ny) = (tx as i64 + dx, ty as i64 + dy);
            let Some(n) = u32::try_from(nx).ok().zip(u32::try_from(ny).ok()).and_then(|(nx, ny)| grid.tile_at(nx, ny)) else {
                continue;
            };
            let Some(b) = world.buildings().at(n).and_then(|id| world.buildings().get(id)) else { continue };
            let house = matches!(archetype(b.kind, b.capacity).0, Archetype::House | Archetype::Townhouse);
            beside_shop |= !house;
            // Une maison qui regarde cette tuile en a fait son jardin de
            // devant : son recul (`HOUSE_SETBACK_M`) laisse la place à
            // l'alignement. Sinon, distance à la limite commune.
            let faces_us = house
                && lot_for(world, roads, n).is_some_and(|lot| lot.front == [-dx as f32, -dy as f32]);
            let to_edge = size * 0.5 - ((x - cx) * dx as f32 + (z - cz) * dy as f32);
            if to_edge < 2.5 && !faces_us {
                return false;
            }
        }
        let served = world.road_access_view().get(tile.index()).copied().unwrap_or(false);
        if !served {
            return true;
        }
        return clearance < STREET_TREE_CLEARANCE.1 && !beside_shop;
    };
    let Some(building) = world.buildings().get(id) else { return false };
    let (arch, _) = archetype(building.kind, building.capacity);
    if !matches!(arch, Archetype::House | Archetype::Townhouse) {
        return false;
    }
    let Some(lot) = lot_for(world, roads, tile) else { return false };
    let fp = house_plan(&lot, arch == Archetype::Townhouse, &mut building_dice(tile));
    let d = [x - lot.center[0], z - lot.center[1]];
    let f = d[0] * lot.front[0] + d[1] * lot.front[1];
    let r = d[0] * lot.right[0] + d[1] * lot.right[1];
    f < fp.f0 - 2.2 && f > -lot.back + 0.6 && r.abs() < lot.half_width - 0.8
}

fn house(world: &World, mesh: &mut UrbanMesh, lot: &Lot, arch: Archetype, floors: u32, dice: &mut Dice) {
    let townhouse = arch == Archetype::Townhouse;
    let fp = house_plan(lot, townhouse, dice);
    let (width, depth, front) = (fp.width(), fp.depth(), fp.f1);

    let (windows, plain, tint) = match dice.next() % 3 {
        0 => (Layer::BrickWindows, Layer::Brick, [dice.range(0.92, 1.05), dice.range(0.9, 1.0), dice.range(0.9, 1.0)]),
        1 => (Layer::PlasterWindows, Layer::Plaster, dice.pick(&PLASTER_TINTS)),
        _ => (Layer::SidingWindows, Layer::Siding, dice.pick(&SIDING_TINTS)),
    };
    let floor = floor_level(world, lot, &fp);
    let eave = floor + floors as f32 * FLOOR_M;

    // Jardin : pelouse sur toute la parcelle, allée jusqu'à la porte, haies.
    let whole = Footprint { r0: -lot.half_width, r1: lot.half_width, f0: -lot.back, f1: lot.ahead };
    ground_patch(world, mesh, lot, &whole, Layer::Lawn, [dice.range(0.85, 1.0), 1.0, dice.range(0.8, 0.95)], 0.08);
    let door_r = dice.pick(&[-1.5, 1.5]) * (width / 6.0);
    let path = Footprint { r0: door_r - 0.6, r1: door_r + 0.6, f0: front, f1: lot.ahead };
    ground_patch(world, mesh, lot, &path, Layer::Sidewalk, WHITE, 0.12);
    if dice.chance(0.75) {
        let t = 0.45;
        for (r0, r1) in [(-lot.half_width, -lot.half_width + t), (lot.half_width - t, lot.half_width)] {
            hedge(world, mesh, lot, Footprint { r0, r1, f0: -lot.back, f1: front - depth - 0.5 }, dice.range(0.8, 1.3));
        }
        hedge(world, mesh, lot, Footprint { r0: -lot.half_width + t, r1: lot.half_width - t, f0: -lot.back, f1: -lot.back + t }, 1.0);
    }

    plinth(world, mesh, lot, &fp, floor);
    walls(mesh, lot, &fp, floor, eave, floor, windows, windows, tint);
    front_door(mesh, lot, door_r, front, floor);
    let roof = dice.pick(&[Layer::ClayTiles, Layer::ClayTiles, Layer::Slate]);
    let roof_tint = dice.pick(&ROOF_TINTS);
    let pitch = dice.range(32.0, 42.0).to_radians();
    let hip = dice.chance(0.4);
    pitched_roof(mesh, lot, &fp, eave, pitch, hip, roof, roof_tint, plain, tint, floor);
    // Cheminée qui dépasse du faîtage.
    if dice.chance(0.6) {
        let r = dice.range(fp.r0 + 0.8, fp.r1 - 1.4);
        let f = (fp.f0 + fp.f1) * 0.5 - dice.range(0.2, 1.2);
        let ridge = eave + fp.depth() * 0.5 * pitch.tan();
        let stack = Footprint { r0: r, r1: r + 0.6, f0: f, f1: f + 0.6 };
        cuboid(mesh, lot, &stack, eave, ridge + 0.9, plain, Layer::Concrete, tint);
    }
}

fn apartments(world: &World, mesh: &mut UrbanMesh, lot: &Lot, floors: u32, dice: &mut Dice) {
    let depth = 7.2f32.min(lot.depth() - 0.5);
    let front = lot.ahead - dice.range(0.3, 1.0);
    let fp = Footprint { r0: -3.7, r1: 3.7, f0: front - depth, f1: front };
    let floor = floor_level(world, lot, &fp);
    let top = floor + floors as f32 * FLOOR_M;
    let tint = dice.pick(&PLASTER_TINTS);
    let forecourt = Footprint { r0: -lot.half_width, r1: lot.half_width, f0: -lot.back, f1: lot.ahead };
    ground_patch(world, mesh, lot, &forecourt, Layer::Lawn, [0.9, 1.0, 0.9], 0.08);
    let path = Footprint { r0: -0.8, r1: 0.8, f0: front, f1: lot.ahead };
    ground_patch(world, mesh, lot, &path, Layer::Sidewalk, WHITE, 0.12);
    plinth(world, mesh, lot, &fp, floor);
    walls(mesh, lot, &fp, floor, top, floor, Layer::Apartment, Layer::Apartment, tint);
    front_door(mesh, lot, 0.0, front, floor);
    flat_roof(mesh, lot, &fp, top, dice);
}

fn shop(world: &World, mesh: &mut UrbanMesh, lot: &Lot, arch: Archetype, floors: u32, dice: &mut Dice) {
    let depth = 7.4f32.min(lot.depth() - 0.3);
    let front = lot.ahead - 0.2;
    let fp = Footprint { r0: -3.8, r1: 3.8, f0: front - depth, f1: front };
    let floor = floor_level(world, lot, &fp);
    let ground_top = floor + SHOP_FLOOR_M;
    let top = ground_top + floors as f32 * FLOOR_M;
    // Parvis pavé devant la vitrine.
    let forecourt = Footprint { r0: -lot.half_width, r1: lot.half_width, f0: -lot.back, f1: lot.ahead };
    ground_patch(world, mesh, lot, &forecourt, Layer::Sidewalk, WHITE, 0.1);

    let (upper, upper_tint) = if arch == Archetype::Offices {
        (Layer::Office, WHITE)
    } else {
        match dice.next() % 3 {
            0 => (Layer::BrickWindows, WHITE),
            1 => (Layer::PlasterWindows, dice.pick(&PLASTER_TINTS)),
            _ => (Layer::Apartment, dice.pick(&PLASTER_TINTS)),
        }
    };
    let side = if arch == Archetype::Offices { Layer::Office } else { Layer::Plaster };
    plinth(world, mesh, lot, &fp, floor);
    // Rez : vitrine devant, enduit sur les autres côtés. Teinte de store variée.
    let awning = dice.pick(&[[1.0, 1.0, 1.0], [0.92, 0.96, 1.0], [0.95, 1.0, 0.94], [1.0, 0.97, 0.9]]);
    walls(mesh, lot, &fp, floor, ground_top, floor, Layer::Shopfront, side, awning);
    if floors > 0 {
        walls(mesh, lot, &fp, ground_top, top, ground_top, upper, upper, upper_tint);
    }
    flat_roof(mesh, lot, &fp, top, dice);
}

fn factory(world: &World, mesh: &mut UrbanMesh, lot: &Lot, dice: &mut Dice) {
    let depth = 7.6f32.min(lot.depth() - 0.3);
    let front = lot.ahead - dice.range(0.2, 0.8);
    let fp = Footprint { r0: -3.85, r1: 3.85, f0: front - depth, f1: front };
    let floor = floor_level(world, lot, &fp);
    let eave = floor + 8.0;
    let tint = dice.pick(&[[1.0, 1.0, 1.0], [0.85, 0.92, 1.0], [1.0, 0.95, 0.85], [0.82, 0.9, 0.82]]);
    let yard = Footprint { r0: -lot.half_width, r1: lot.half_width, f0: -lot.back, f1: lot.ahead };
    ground_patch(world, mesh, lot, &yard, Layer::Concrete, [0.8, 0.8, 0.8], 0.1);
    plinth(world, mesh, lot, &fp, floor);
    walls(mesh, lot, &fp, floor, eave, floor, Layer::Industrial, Layer::Industrial, tint);
    if dice.chance(0.6) {
        pitched_roof(mesh, lot, &fp, eave, 12f32.to_radians(), false, Layer::MetalRoof, [0.9, 0.92, 0.95], Layer::Industrial, tint, floor);
    } else {
        flat_roof(mesh, lot, &fp, eave, dice);
    }
    // Cheminée de brique.
    if dice.chance(0.12) {
        let r = dice.range(fp.r0 + 0.5, fp.r1 - 1.7);
        let f = fp.f0 + dice.range(0.5, 2.0);
        let stack = Footprint { r0: r, r1: r + 1.2, f0: f, f1: f + 1.2 };
        cuboid(mesh, lot, &stack, eave - 1.0, eave + dice.range(4.0, 8.0), Layer::Brick, Layer::Concrete, [0.9, 0.85, 0.8]);
    }
}

/// Tous les bâtiments dont la tuile est dans `[x0, x1) × [y0, y1)`.
#[must_use]
pub fn buildings_in(world: &World, roads: &RoadIndex, x0: u32, y0: u32, x1: u32, y1: u32) -> UrbanMesh {
    let grid = world.zoning().grid();
    let mut mesh = UrbanMesh::default();
    for y in y0..y1.min(grid.height()) {
        for x in x0..x1.min(grid.width()) {
            let Some(tile) = grid.tile_at(x, y) else { continue };
            let Some(building) = world.buildings().at(tile).and_then(|id| world.buildings().get(id)) else {
                continue;
            };
            if let Some(lot) = lot_for(world, roads, tile) {
                let (stage, within) = building.stage(world.current_tick());
                push_building(world, &mut mesh, &lot, building.kind, building.capacity, stage, within.to_f32());
            }
        }
    }
    mesh
}

/// Pas d'avancement *visible* d'un chantier : la géométrie ne change qu'à
/// ces paliers, inutile de reconstruire entre deux.
fn visual_step(stage: ConstructionStage, within: f32) -> u64 {
    match stage {
        ConstructionStage::Foundations => (within * 3.0) as u64,
        ConstructionStage::Structure => (within * 4.0) as u64,
        ConstructionStage::Finishing => u64::from(within >= 0.35) + u64::from(within >= 0.75),
        _ => 0,
    }
}

/// Empreintes des bâtiments d'un carré de tuiles : `(visuelle, occupation)`.
///
/// - visuelle : change dès qu'un bâtiment apparaît, disparaît, change de
///   type/capacité ou franchit un palier de chantier (géométrie à refaire) ;
/// - occupation : seulement à l'apparition ou la disparition d'un bâtiment
///   (les arbres de la ville n'en dépendent pas davantage).
#[must_use]
pub fn buildings_signature(world: &World, x0: u32, y0: u32, x1: u32, y1: u32) -> (u64, u64) {
    let grid = world.zoning().grid();
    let now = world.current_tick();
    let mut signature = 0xC17E_u64;
    let mut occupancy = 0x0CC0_u64;
    for y in y0..y1.min(grid.height()) {
        for x in x0..x1.min(grid.width()) {
            let Some(tile) = grid.tile_at(x, y) else { continue };
            if let Some(building) = world.buildings().at(tile).and_then(|id| world.buildings().get(id)) {
                let kind = building.kind as u64;
                // Chantier : stade, et quart d'avancement (la structure monte).
                let (stage, within) = building.stage(now);
                let step = (stage as u64) << 8 | visual_step(stage, within.to_f32());
                let base = (u64::from(tile.raw()) << 20) ^ (kind << 16) ^ u64::from(building.capacity);
                signature = hash(signature ^ base ^ (step << 40));
                occupancy = hash(occupancy ^ base);
            }
        }
    }
    (signature, occupancy)
}

// ------------------------------------------------------------------ voirie

/// Arbres d'alignement : un tous les `STREET_TREE_SPACING_M` de chaque côté
/// des routes, juste derrière le trottoir ; une essence par rue. Le filtre
/// `tree_allowed` écarte ensuite ceux qui tombent devant un commerce, sur
/// une parcelle ou dans un carrefour.
#[must_use]
pub fn street_tree_spots(world: &World, strips: &[RoadStrip]) -> Vec<(TreeSpot, TileId)> {
    const STREET_TREE_SPACING_M: f32 = 11.0;
    let grid = world.zoning().grid();
    let size = world.config().tile_size_m as f32;
    let mut spots = Vec::new();
    for strip in strips {
        let mut dice = Dice(hash(u64::from(strip.nodes[0]) << 32 | u64::from(strip.nodes[1])));
        let species = dice.pick(&[Species::Oak, Species::Oak, Species::Birch]);
        let profile = crate::roads::Profile::of(strip.spec);
        // Alignements derrière les trottoirs, et sur le terre-plein planté.
        let mut offsets: Vec<f32> = Vec::new();
        if profile.sidewalk > 0.0 {
            offsets.push(profile.reach() + 0.95);
        }
        let median_trees = profile.planted_median;
        let mut next = STREET_TREE_SPACING_M * 0.5;
        let mut walked = 0.0;
        for pair in strip.points.windows(2) {
            let (a, b) = ([pair[0][0], pair[0][2]], [pair[1][0], pair[1][2]]);
            let len = dist2(a, b);
            if len < 1e-4 {
                continue;
            }
            let dir = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
            while next <= walked + len {
                let t = next - walked;
                let p = [a[0] + dir[0] * t, a[1] + dir[1] * t];
                let mut placements: Vec<f32> = offsets.iter().flat_map(|o| [-*o, *o]).collect();
                if median_trees {
                    placements.push(0.0);
                }
                for offset in placements {
                    let (x, z) = (p[0] - dir[1] * offset, p[1] + dir[0] * offset);
                    if x < 0.0 || z < 0.0 {
                        continue;
                    }
                    let Some(tile) = grid.tile_at((x / size) as u32, (z / size) as u32) else { continue };
                    let spot = TreeSpot {
                        base: [x, ground_altitude_m(world, x, z), z],
                        species,
                        scale: dice.range(0.68, 0.88),
                        tint: dice.range(0.0, 0.8),
                        angle: dice.range(0.0, std::f32::consts::TAU),
                    };
                    spots.push((spot, tile));
                }
                next += STREET_TREE_SPACING_M;
            }
            walked += len;
        }
    }
    spots
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene;

    fn city() -> World {
        // Même pré-simulation que le viewer : chantiers livrés et en cours.
        let mut world = scene::demo_city();
        for _ in 0..2_400 {
            world.tick();
        }
        world
    }

    #[test]
    fn layer_table_matches_the_blender_script() {
        let script = include_str!("../../../tools/blender/make_buildings.py");
        for (i, name) in LAYER_NAMES.iter().enumerate() {
            let [w, h] = LAYER_SIZE_M[i];
            let entry = format!("(\"{name}\", {w:?}, {h:?})");
            assert!(script.contains(&entry), "couche {name} absente ou de taille différente : {entry}");
        }
        assert_eq!(Layer::Hoarding.index(), LAYER_COUNT - 1);
    }

    #[test]
    fn archetypes_follow_capacity() {
        assert_eq!(archetype(BuildingKind::Household, 6).0, Archetype::House);
        assert_eq!(archetype(BuildingKind::Household, 30).0, Archetype::Townhouse);
        let (arch, floors) = archetype(BuildingKind::Household, 48);
        assert_eq!(arch, Archetype::Apartments);
        assert!((3..=9).contains(&floors));
        assert_eq!(archetype(BuildingKind::Shop, 4), (Archetype::Shop, 0));
        assert_eq!(archetype(BuildingKind::Factory, 20).0, Archetype::Factory);
        for cap in 1..60 {
            for kind in [BuildingKind::Household, BuildingKind::Shop, BuildingKind::Factory] {
                let (arch, floors) = archetype(kind, cap);
                let h = archetype_height_m(arch, floors);
                assert!((4.0..45.0).contains(&h), "{kind:?} {cap} -> {h} m");
            }
        }
    }

    #[test]
    fn every_building_is_sane_and_stays_off_the_road() {
        let world = city();
        let strips = scene::roads(&world);
        let roads = RoadIndex::new(&strips);
        assert!(world.buildings().live_count() > 200, "la démo doit avoir poussé");
        let mut kinds = std::collections::HashSet::new();
        let mut squeezed = 0;
        for (_, building) in world.buildings().iter() {
            let Some(lot) = lot_for(&world, &roads, building.tile) else {
                squeezed += 1;
                continue;
            };
            let mut mesh = UrbanMesh::default();
            let (stage, within) = building.stage(world.current_tick());
            push_building(&world, &mut mesh, &lot, building.kind, building.capacity, stage, within.to_f32());
            assert!(!mesh.is_empty() && mesh.is_sane());
            let budget = if building.complete { 600 } else { 12_000 };
            assert!(mesh.vertex_count() < budget, "{:?} ({stage:?}) trop lourd : {}", building.kind, mesh.vertex_count());
            kinds.insert(archetype(building.kind, building.capacity).0);
            // Le bâti (hors parcelle) ne mord ni sur la chaussée ni sur le
            // trottoir. Sur un chantier, seule la flèche de grue survole la
            // rue : on vérifie ce qui est au sol (palissade, engins).
            let ground = ground_altitude_m(&world, lot.center[0], lot.center[1]);
            let band = if building.complete { (ground + 2.5, f32::INFINITY) } else { (ground - 5.0, ground + 2.5) };
            for p in mesh.positions.iter().filter(|p| p[1] > band.0 && p[1] < band.1) {
                let clearance = roads.clearance([p[0], p[2]], 30.0);
                // Un échafaudage peut empiéter d'un mètre sur le trottoir
                // (jamais sur la chaussée : trottoirs de 2 m et plus).
                let limit = if building.complete { -0.5 } else { -1.5 };
                assert!(clearance > limit, "{stage:?} sur la route en {p:?} ({clearance})");
            }
        }
        assert!(kinds.len() >= 3, "trop peu de variété : {kinds:?}");
        // Une parcelle coincée entre deux routes n'a pas de place pour bâtir :
        // ce doit rester rare.
        assert!(squeezed * 20 < world.buildings().live_count(), "{squeezed} parcelles sans place");
    }

    #[test]
    fn facades_face_the_road() {
        let world = city();
        let roads = RoadIndex::new(&scene::roads(&world));
        for (_, building) in world.buildings().iter().take(300) {
            let Some(lot) = lot_for(&world, &roads, building.tile) else { continue };
            let road = roads.nearest(lot.center, 40.0).expect("route proche");
            let to_road = [road.point[0] - lot.center[0], road.point[1] - lot.center[1]];
            assert!(to_road[0] * lot.front[0] + to_road[1] * lot.front[1] > 0.0);
        }
    }

    #[test]
    fn city_trees_avoid_houses_and_roads() {
        let world = city();
        let roads = RoadIndex::new(&scene::roads(&world));
        let grid = world.zoning().grid();
        // Murs et toits de chaque bâtiment, par tuile.
        let mut walls: HashMap<(u32, u32), Vec<[f32; 3]>> = HashMap::new();
        for (_, building) in world.buildings().iter().filter(|(_, b)| b.complete) {
            let Some(lot) = lot_for(&world, &roads, building.tile) else { continue };
            let mut mesh = UrbanMesh::default();
            let (stage, within) = building.stage(world.current_tick());
            push_building(&world, &mut mesh, &lot, building.kind, building.capacity, stage, within.to_f32());
            let ground = ground_altitude_m(&world, lot.center[0], lot.center[1]);
            let high: Vec<_> = mesh.positions.into_iter().filter(|p| p[1] > ground + 2.0).collect();
            walls.insert(grid.xy(building.tile).unwrap(), high);
        }
        let size = world.config().tile_size_m as f32;
        let mut street = 0;
        let strips = scene::roads(&world);
        let mut candidates: Vec<_> = crate::nature::play_area_tree_cells(&world, 256.0).into_iter().flatten().collect();
        candidates.extend(street_tree_spots(&world, &strips));
        for (spot, tile) in &candidates {
            let (x, z) = (spot.base[0], spot.base[2]);
            if !tree_allowed(&world, &roads, x, z, *tile) {
                continue;
            }
            let clearance = roads.clearance([x, z], 12.0);
            assert!(clearance >= 0.6 || roads.in_median([x, z], 0.8), "arbre sur la route en {x} {z}");
            if clearance < 2.0 {
                street += 1;
            }
            let (tx, ty) = ((x / size) as i64, (z / size) as i64);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let key = ((tx + dx).max(0) as u32, (ty + dy).max(0) as u32);
                    for p in walls.get(&key).map_or(&[][..], Vec::as_slice) {
                        // Sommets des murs : distance au tronc dans le plan,
                        // mesurée aux coins (les murs sont des quads).
                        let d = ((p[0] - x).powi(2) + (p[2] - z).powi(2)).sqrt();
                        assert!(d > 0.5, "arbre collé à un bâtiment en {x} {z}");
                    }
                }
            }
        }
        assert!(street > 200, "trop peu d'arbres d'alignement : {street}");
    }

    #[test]
    fn every_construction_stage_shows_its_site() {
        let world = city();
        let roads = RoadIndex::new(&scene::roads(&world));
        let lot = world.buildings().iter().find_map(|(_, b)| lot_for(&world, &roads, b.tile)).unwrap();
        let tall = BuildingKind::Household;
        let mut previous_height = 0.0f32;
        for (stage, within) in [
            (ConstructionStage::SitePreparation, 0.5),
            (ConstructionStage::Foundations, 0.5),
            (ConstructionStage::Structure, 0.2),
            (ConstructionStage::Structure, 0.9),
            (ConstructionStage::Finishing, 0.9),
        ] {
            let mut mesh = UrbanMesh::default();
            push_building(&world, &mut mesh, &lot, tall, 48, stage, within);
            assert!(mesh.is_sane(), "{stage:?}");
            let layers: std::collections::HashSet<usize> =
                mesh.colors.iter().map(|c| (c[3] * LAYER_ENCODING).round() as usize).collect();
            assert!(layers.contains(&Layer::Hoarding.index()), "{stage:?} : palissade");
            assert!(layers.contains(&Layer::Works.index()), "{stage:?} : terre");
            // La structure monte d'un stade à l'autre (hors grue : sous 25 m).
            let ground = ground_altitude_m(&world, lot.center[0], lot.center[1]);
            let height = mesh
                .positions
                .iter()
                .filter(|p| p[1] < ground + 25.0)
                .map(|p| p[1] - ground)
                .fold(0.0, f32::max);
            if stage == ConstructionStage::Structure {
                assert!(height >= previous_height - 0.01, "{stage:?} {within} : {height} < {previous_height}");
            }
            previous_height = height;
        }
        // Certains immeubles en gros œuvre ont une grue (bien au-dessus du
        // bâtiment), pas tous : une grue dessert plusieurs parcelles.
        let lots: Vec<Lot> = world.buildings().iter().filter_map(|(_, b)| lot_for(&world, &roads, b.tile)).take(200).collect();
        let cranes = lots
            .iter()
            .filter(|lot| {
                let mut mesh = UrbanMesh::default();
                push_building(&world, &mut mesh, lot, tall, 48, ConstructionStage::Structure, 0.5);
                let ground = ground_altitude_m(&world, lot.center[0], lot.center[1]);
                mesh.positions.iter().any(|p| p[1] - ground > 25.0)
            })
            .count();
        assert!(cranes > 10 && cranes < 80, "{cranes} grues sur {} chantiers", lots.len());
    }

    #[test]
    fn same_city_same_houses() {
        let world = city();
        let roads = RoadIndex::new(&scene::roads(&world));
        let a = buildings_in(&world, &roads, 0, 0, 128, 128);
        let b = buildings_in(&world, &roads, 0, 0, 128, 128);
        assert_eq!(a, b);
        assert_eq!(buildings_signature(&world, 0, 0, 128, 128), buildings_signature(&world, 0, 0, 128, 128));
    }

}
