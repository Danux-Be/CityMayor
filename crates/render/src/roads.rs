//! Voirie : profils en travers, marquages, chantiers, carrefours, ronds-points.
//!
//! # Profil en travers
//! Chaque classe de route (`RoadClass`) a son profil (`Profile`) : largeur
//! de voie, terre-plein central (planté pour les boulevards, séparateur en
//! béton pour les autoroutes), bandes d'arrêt d'urgence, trottoirs,
//! glissières. Le décalage latéral `o` se mesure depuis l'axe, positif à
//! **droite** du sens `a → b` : en circulation à droite, les voies « aller »
//! (`forward_lanes`) sont du côté positif.
//!
//! # Marquages
//! Les lignes ne sont pas peintes dans la texture de chaussée : ce sont des
//! bandes de géométrie (couche `Marking`) posées juste au-dessus de
//! l'asphalte. Un seul asphalte sert donc à toutes les configurations
//! (1 à 6 voies, sens unique, autoroute), et les flèches suivent les voies.
//!
//! # Chantier
//! Une route en travaux (`RoadSegment::opens_at` dans le futur) montre son
//! avancement : la partie déjà revêtue est en asphalte nu, le reste en terre
//! compactée, avec des balises rayées aux deux bouts et au front de pose.
//! Marquages, trottoirs et glissières n'apparaissent qu'à l'ouverture.

use std::collections::HashMap;

use sim_core::{RoadClass, RoadSpec, World};

use crate::nature::ground_altitude_m;
use crate::scene::RoadStrip;
use crate::urban::{Layer, UrbanMesh};

const WHITE: [f32; 3] = [1.0, 1.0, 1.0];
const UP: [f32; 3] = [0.0, 1.0, 0.0];
/// Hauteur des bordures (trottoirs, terre-plein, îlot).
pub const CURB_M: f32 = 0.15;
/// Surélévation des marquages au-dessus de l'asphalte.
const PAINT_LIFT_M: f32 = 0.03;
/// Largeur d'une ligne peinte.
const LINE_M: f32 = 0.15;

/// Profil en travers d'une route.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Profile {
    pub lane: f32,
    /// Largeur du terre-plein central (0 si aucun).
    pub median: f32,
    /// Terre-plein planté (boulevard) plutôt qu'un séparateur en béton.
    pub planted_median: bool,
    /// Bande dérasée côté terre-plein et côté extérieur.
    pub inner_shoulder: f32,
    pub outer_shoulder: f32,
    pub sidewalk: f32,
    pub surface: Layer,
    pub guardrails: bool,
    pub forward: u8,
    pub backward: u8,
}

impl Profile {
    #[must_use]
    pub fn of(spec: RoadSpec) -> Self {
        let (f, b) = (spec.forward_lanes, spec.backward_lanes);
        let base = Self {
            lane: 3.25,
            median: 0.0,
            planted_median: false,
            inner_shoulder: 0.0,
            outer_shoulder: 0.0,
            sidewalk: 2.0,
            surface: Layer::Asphalt,
            guardrails: false,
            forward: f,
            backward: b,
        };
        match spec.class {
            RoadClass::Gravel => Self { lane: 2.75, sidewalk: 0.0, outer_shoulder: 0.4, surface: Layer::Gravel, ..base },
            RoadClass::Street => base,
            RoadClass::Avenue => Self { lane: 3.5, sidewalk: 2.5, ..base },
            RoadClass::Boulevard => Self { lane: 3.5, median: 4.0, planted_median: true, inner_shoulder: 0.3, sidewalk: 3.0, ..base },
            RoadClass::Highway => Self {
                lane: 3.75,
                median: if f > 0 && b > 0 { 1.2 } else { 0.0 },
                inner_shoulder: if f > 0 && b > 0 { 0.75 } else { 1.0 },
                outer_shoulder: 2.5,
                sidewalk: 0.0,
                guardrails: true,
                ..base
            },
        }
    }

    #[must_use]
    pub fn two_way(&self) -> bool {
        self.forward > 0 && self.backward > 0
    }

    /// Demi-largeur revêtue (chaussées + terre-plein + bandes), sans trottoir.
    #[must_use]
    pub fn half_width(&self) -> f32 {
        if self.two_way() {
            let side = f32::from(self.forward.max(self.backward));
            self.median * 0.5 + self.inner_shoulder + side * self.lane + self.outer_shoulder
        } else {
            // Sens unique : voies centrées sur l'axe, bande la plus large des
            // deux côtés (profil symétrique).
            let lanes = f32::from(self.forward + self.backward);
            lanes * self.lane * 0.5 + self.inner_shoulder.max(self.outer_shoulder)
        }
    }

    /// Demi-largeur totale (trottoir compris) : ce qu'aucune parcelle ne doit
    /// toucher.
    #[must_use]
    pub fn reach(&self) -> f32 {
        self.half_width() + self.sidewalk
    }

    /// Voies : `(décalage du bord gauche, décalage du bord droit, sens aller)`
    /// pour chaque voie, de gauche à droite.
    #[must_use]
    pub fn lanes(&self) -> Vec<(f32, f32, bool)> {
        let mut lanes = Vec::new();
        if self.two_way() {
            let inner = self.median * 0.5 + self.inner_shoulder;
            for i in (0..self.backward).rev() {
                let o = inner + f32::from(i) * self.lane;
                lanes.push((-(o + self.lane), -o, false));
            }
            for i in 0..self.forward {
                let o = inner + f32::from(i) * self.lane;
                lanes.push((o, o + self.lane, true));
            }
        } else {
            let count = self.forward + self.backward;
            let start = -(f32::from(count) * self.lane) * 0.5;
            let forward = self.forward > 0;
            for i in 0..count {
                let o = start + f32::from(i) * self.lane;
                lanes.push((o, o + self.lane, forward));
            }
        }
        lanes
    }
}

/// Polyligne d'axe ré-échantillonnable : stations (abscisse curviligne),
/// normales en onglet (à droite du sens de marche).
struct Path {
    points: Vec<[f32; 3]>,
    normals: Vec<[f32; 2]>,
    arc: Vec<f32>,
}

impl Path {
    fn new(points: &[[f32; 3]]) -> Self {
        let n = points.len();
        let mut normals = Vec::with_capacity(n);
        let mut arc = Vec::with_capacity(n);
        let mut s = 0.0;
        for i in 0..n {
            if i > 0 {
                s += ((points[i][0] - points[i - 1][0]).powi(2) + (points[i][2] - points[i - 1][2]).powi(2)).sqrt();
            }
            arc.push(s);
            let prev = points[i.saturating_sub(1)];
            let next = points[(i + 1).min(n - 1)];
            let d = [next[0] - prev[0], next[2] - prev[2]];
            let l = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-6);
            // Droite du sens de marche (Y vers le haut) : d × Y = (-dz, dx).
            normals.push([-d[1] / l, d[0] / l]);
        }
        Self { points: points.to_vec(), normals, arc }
    }

    fn length(&self) -> f32 {
        *self.arc.last().unwrap_or(&0.0)
    }

    /// Point et normale à l'abscisse `s`.
    fn at(&self, s: f32) -> ([f32; 3], [f32; 2]) {
        let n = self.points.len();
        let i = self.arc.partition_point(|&a| a < s).clamp(1, n - 1);
        let t = ((s - self.arc[i - 1]) / (self.arc[i] - self.arc[i - 1]).max(1e-6)).clamp(0.0, 1.0);
        let (a, b) = (self.points[i - 1], self.points[i]);
        let p = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
        let (na, nb) = (self.normals[i - 1], self.normals[i]);
        let (nx, nz) = (na[0] + (nb[0] - na[0]) * t, na[1] + (nb[1] - na[1]) * t);
        let l = (nx * nx + nz * nz).sqrt().max(1e-6);
        (p, [nx / l, nz / l])
    }

    /// Stations de `s0` à `s1` : les deux bornes et les sommets entre elles.
    fn stations(&self, s0: f32, s1: f32) -> Vec<([f32; 3], [f32; 2], f32)> {
        let mut out = vec![{
            let (p, n) = self.at(s0);
            (p, n, s0)
        }];
        for (i, &a) in self.arc.iter().enumerate() {
            if a > s0 && a < s1 {
                out.push((self.points[i], self.normals[i], a));
            }
        }
        let (p, n) = self.at(s1);
        out.push((p, n, s1));
        out
    }
}

fn offset(p: [f32; 3], n: [f32; 2], o: f32, dy: f32) -> [f32; 3] {
    [p[0] + n[0] * o, p[1] + dy, p[2] + n[1] * o]
}

/// Bande horizontale entre deux décalages, de `s0` à `s1`, texture
/// projetée en coordonnées monde (pas de couture d'un segment à l'autre).
#[allow(clippy::too_many_arguments)]
fn band(mesh: &mut UrbanMesh, path: &Path, o0: f32, o1: f32, s0: f32, s1: f32, dy: f32, layer: Layer, tint: [f32; 3]) {
    if s1 - s0 < 0.01 {
        return;
    }
    let st = path.stations(s0, s1);
    for w in st.windows(2) {
        let ((pa, na, _), (pb, nb, _)) = (w[0], w[1]);
        let quad = [offset(pa, na, o0, dy), offset(pa, na, o1, dy), offset(pb, nb, o1, dy), offset(pb, nb, o0, dy)];
        mesh.surface(&quad, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], layer, tint, UP);
    }
}

/// Paroi verticale le long de l'axe, au décalage `o`, de `dy0` à `dy1`
/// (relatifs à la chaussée), tournée vers `side` (±1 : droite/gauche).
#[allow(clippy::too_many_arguments)]
fn wall(mesh: &mut UrbanMesh, path: &Path, o: f32, s0: f32, s1: f32, dy0: f32, dy1: f32, side: f32, layer: Layer, tint: [f32; 3]) {
    if s1 - s0 < 0.01 {
        return;
    }
    let [um, vm] = layer.size_m();
    let st = path.stations(s0, s1);
    for w in st.windows(2) {
        let ((pa, na, sa), (pb, nb, sb)) = (w[0], w[1]);
        let quad = [offset(pa, na, o, dy0), offset(pb, nb, o, dy0), offset(pb, nb, o, dy1), offset(pa, na, o, dy1)];
        let uv = [[sa / um, (dy1 - dy0) / vm], [sb / um, (dy1 - dy0) / vm], [sb / um, 0.0], [sa / um, 0.0]];
        let facing = [na[0] * side, 0.0, na[1] * side];
        mesh.polygon(&quad, &uv, layer, tint, facing);
    }
}

/// Talus : du bord `o` (à hauteur `dy`) jusqu'au terrain, légèrement enterré.
#[allow(clippy::too_many_arguments)]
fn skirt(world: &World, mesh: &mut UrbanMesh, path: &Path, o: f32, s0: f32, s1: f32, dy: f32, side: f32, layer: Layer, tint: [f32; 3]) {
    if s1 - s0 < 0.01 {
        return;
    }
    let st = path.stations(s0, s1);
    for w in st.windows(2) {
        let ((pa, na, _), (pb, nb, _)) = (w[0], w[1]);
        let (ta, tb) = (offset(pa, na, o, dy), offset(pb, nb, o, dy));
        let ga = ground_altitude_m(world, ta[0], ta[2]) - 0.6;
        let gb = ground_altitude_m(world, tb[0], tb[2]) - 0.6;
        // Le pied du talus s'écarte un peu : pente douce plutôt qu'un mur.
        let spread = 1.0 + (ta[1] - ga).max(0.0) * 0.8;
        let (fa, fb) = (offset(pa, na, o + spread * side, 0.0), offset(pb, nb, o + spread * side, 0.0));
        let quad = [ta, tb, [fb[0], gb, fb[2]], [fa[0], ga, fa[2]]];
        mesh.surface(&quad, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], layer, tint, [na[0] * side, 0.6, na[1] * side]);
    }
}

/// Ligne pointillée (ou continue si `gap == 0`) au décalage `o`.
#[allow(clippy::too_many_arguments)]
fn line(mesh: &mut UrbanMesh, path: &Path, o: f32, s0: f32, s1: f32, dash: f32, gap: f32, dy: f32) {
    if gap <= 0.0 {
        band(mesh, path, o - LINE_M * 0.5, o + LINE_M * 0.5, s0, s1, dy + PAINT_LIFT_M, Layer::Marking, WHITE);
        return;
    }
    let mut s = s0 + gap * 0.5;
    while s < s1 {
        let e = (s + dash).min(s1);
        band(mesh, path, o - LINE_M * 0.5, o + LINE_M * 0.5, s, e, dy + PAINT_LIFT_M, Layer::Marking, WHITE);
        s += dash + gap;
    }
}

/// Flèche au sol centrée sur la voie `(o0, o1)` à l'abscisse `s`.
fn arrow(mesh: &mut UrbanMesh, path: &Path, o0: f32, o1: f32, s: f32, forward: bool) {
    let [_, len] = Layer::Arrow.size_m();
    let (s0, s1) = (s - len * 0.5, s + len * 0.5);
    if s0 < 0.0 || s1 > path.length() {
        return;
    }
    let (pa, na) = path.at(s0);
    let (pb, nb) = path.at(s1);
    let dy = PAINT_LIFT_M * 0.5;
    let quad = [offset(pa, na, o0, dy), offset(pa, na, o1, dy), offset(pb, nb, o1, dy), offset(pb, nb, o0, dy)];
    // La flèche de la texture pointe vers v = 0 (haut de l'image).
    let uv = if forward {
        [[1.0, 1.0], [0.0, 1.0], [0.0, 0.0], [1.0, 0.0]]
    } else {
        [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
    };
    mesh.polygon(&quad, &uv, Layer::Arrow, WHITE, UP);
}

/// Balises de chantier en travers de la chaussée à l'abscisse `s`.
fn beacons(mesh: &mut UrbanMesh, path: &Path, s: f32, half: f32) {
    let (p, n) = path.at(s.clamp(0.0, path.length()));
    let d = [n[1], -n[0]]; // sens de marche
    let [bw, bh] = Layer::Beacon.size_m();
    let count = ((2.0 * half) / (bw + 0.4)).floor().max(1.0) as i32;
    for k in 0..count {
        let c = -half + (k as f32 + 0.5) * (2.0 * half / count as f32);
        let a = offset(p, n, c - bw * 0.5, 0.0);
        let b = offset(p, n, c + bw * 0.5, 0.0);
        let quad = [[a[0], a[1] + 0.2, a[2]], [b[0], b[1] + 0.2, b[2]], [b[0], b[1] + 0.2 + bh, b[2]], [a[0], a[1] + 0.2 + bh, a[2]]];
        let uv = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        mesh.polygon(&quad, &uv, Layer::Beacon, WHITE, [d[0], 0.0, d[1]]);
        mesh.polygon(&quad, &uv, Layer::Beacon, WHITE, [-d[0], 0.0, -d[1]]);
    }
}

/// Une route complète : chaussée(s), terre-plein, marquages, trottoirs,
/// glissières, talus. `trim` : longueur réservée au carrefour à chaque bout
/// (pas de trottoir ni de marquage). `inner_side` : côté de l'îlot pour un
/// anneau de rond-point (pas de trottoir de ce côté).
fn road(world: &World, mesh: &mut UrbanMesh, strip: &RoadStrip, trim: [f32; 2], inner_side: Option<f32>) {
    if strip.points.len() < 2 {
        return;
    }
    let path = Path::new(&strip.points);
    let total = path.length();
    let profile = Profile::of(strip.spec);
    let half = profile.half_width();
    let open = strip.progress >= 1.0;
    let (s0, s1) = (trim[0].min(total * 0.5), (total - trim[1]).max(total * 0.5));

    // --- revêtement (asphalte / gravier / chantier)
    let paved_to = if open { total } else { total * strip.progress };
    let median_half = if profile.planted_median { profile.median * 0.5 } else { 0.0 };
    let carriageways: Vec<(f32, f32)> = if median_half > 0.0 { vec![(-half, -median_half), (median_half, half)] } else { vec![(-half, half)] };
    for (o0, o1) in &carriageways {
        band(mesh, &path, *o0, *o1, 0.0, paved_to, 0.0, profile.surface, WHITE);
        band(mesh, &path, *o0, *o1, paved_to, total, 0.0, Layer::Works, WHITE);
    }
    if !open {
        beacons(mesh, &path, 0.4, half);
        beacons(mesh, &path, total - 0.4, half);
        if paved_to > 1.0 && paved_to < total - 1.0 {
            beacons(mesh, &path, paved_to, half);
        }
        skirt(world, mesh, &path, half, 0.0, total, 0.0, 1.0, Layer::Works, WHITE);
        skirt(world, mesh, &path, -half, 0.0, total, 0.0, -1.0, Layer::Works, WHITE);
        return;
    }

    // --- terre-plein
    if profile.planted_median {
        band(mesh, &path, -median_half, median_half, s0, s1, CURB_M, Layer::Lawn, [0.85, 1.0, 0.85]);
        wall(mesh, &path, -median_half, s0, s1, 0.0, CURB_M, -1.0, Layer::Concrete, [0.95, 0.95, 0.95]);
        wall(mesh, &path, median_half, s0, s1, 0.0, CURB_M, 1.0, Layer::Concrete, [0.95, 0.95, 0.95]);
        // Le terre-plein s'arrête avant le carrefour : la chaussée continue.
        band(mesh, &path, -median_half, median_half, 0.0, s0, 0.0, Layer::Asphalt, WHITE);
        band(mesh, &path, -median_half, median_half, s1, total, 0.0, Layer::Asphalt, WHITE);
    } else if profile.median > 0.0 {
        // Séparateur en béton (type GBA) : deux flancs et un dessus.
        let m = 0.3;
        wall(mesh, &path, -m, s0, s1, 0.0, 0.85, -1.0, Layer::Concrete, [0.9, 0.9, 0.88]);
        wall(mesh, &path, m, s0, s1, 0.0, 0.85, 1.0, Layer::Concrete, [0.9, 0.9, 0.88]);
        band(mesh, &path, -m, m, s0, s1, 0.85, Layer::Concrete, [0.9, 0.9, 0.88]);
    }

    // --- marquages
    if profile.surface == Layer::Asphalt {
        let lanes = profile.lanes();
        let highway = strip.spec.class == RoadClass::Highway;
        let (dash, gap) = if highway { (6.0, 12.0) } else { (3.0, 5.0) };
        for pair in lanes.windows(2) {
            let ((_, right, dir_a), (left, _, dir_b)) = (pair[0], pair[1]);
            if (right - left).abs() > 0.01 {
                continue; // terre-plein entre les deux
            }
            if dir_a == dir_b {
                line(mesh, &path, right, s0, s1, dash, gap, 0.0);
            } else if strip.spec.class == RoadClass::Avenue {
                // Double ligne continue entre les sens.
                line(mesh, &path, right - 0.15, s0, s1, 0.0, 0.0, 0.0);
                line(mesh, &path, right + 0.15, s0, s1, 0.0, 0.0, 0.0);
            } else {
                line(mesh, &path, right, s0, s1, 3.0, 5.0, 0.0);
            }
        }
        if highway || profile.planted_median {
            // Lignes de rive continues le long des voies extrêmes.
            if let (Some(first), Some(last)) = (lanes.first(), lanes.last()) {
                line(mesh, &path, first.0 + LINE_M, s0, s1, 0.0, 0.0, 0.0);
                line(mesh, &path, last.1 - LINE_M, s0, s1, 0.0, 0.0, 0.0);
            }
            if profile.two_way() {
                let inner = profile.median * 0.5 + profile.inner_shoulder;
                line(mesh, &path, -inner - LINE_M, s0, s1, 0.0, 0.0, 0.0);
                line(mesh, &path, inner + LINE_M, s0, s1, 0.0, 0.0, 0.0);
            }
        }
        // Flèches : voies à sens unique, tous les 45 m.
        if strip.spec.is_one_way() && inner_side.is_none() {
            let mut s = s0 + 12.0;
            while s < s1 - 6.0 {
                for (o0, o1, forward) in &lanes {
                    arrow(mesh, &path, *o0 + 0.3, *o1 - 0.3, s, *forward);
                }
                s += 45.0;
            }
        }
    }

    // --- trottoirs (bordure, dessus pavé) ou talus
    for side in [-1.0f32, 1.0] {
        let edge = half * side;
        if profile.sidewalk > 0.0 && inner_side != Some(side) {
            let outer = (half + profile.sidewalk) * side;
            let (lo, hi) = if side > 0.0 { (edge, outer) } else { (outer, edge) };
            band(mesh, &path, lo, hi, s0, s1, CURB_M, Layer::Sidewalk, WHITE);
            wall(mesh, &path, edge, s0, s1, 0.0, CURB_M, -side, Layer::Concrete, [0.95, 0.95, 0.95]);
            skirt(world, mesh, &path, outer, s0, s1, CURB_M, side, Layer::Concrete, [0.8, 0.8, 0.78]);
            // Sous le carrefour : talus au ras de la chaussée.
            skirt(world, mesh, &path, edge, 0.0, s0, 0.0, side, Layer::Concrete, [0.8, 0.8, 0.78]);
            skirt(world, mesh, &path, edge, s1, total, 0.0, side, Layer::Concrete, [0.8, 0.8, 0.78]);
        } else if inner_side == Some(side) {
            // Côté îlot : bordure seule (l'îlot est dessiné à part).
            wall(mesh, &path, edge, 0.0, total, 0.0, CURB_M, -side, Layer::Concrete, [0.95, 0.95, 0.95]);
        } else {
            skirt(world, mesh, &path, edge, 0.0, total, 0.0, side, Layer::Gravel, [0.85, 0.85, 0.8]);
        }
        // Glissières : lisse à 0,45–0,77 m sur poteaux tous les 4 m.
        if profile.guardrails {
            let o = (half - 0.4) * side;
            wall(mesh, &path, o, s0, s1, 0.45, 0.77, side, Layer::Guardrail, WHITE);
            wall(mesh, &path, o, s0, s1, 0.45, 0.77, -side, Layer::Guardrail, WHITE);
            let mut s = s0 + 2.0;
            while s < s1 {
                let (p, n) = path.at(s);
                let d = [n[1], -n[0]];
                let c = offset(p, n, o + 0.1 * side, 0.0);
                let corners = [
                    [c[0] - d[0] * 0.07 - n[0] * 0.07, c[2] - d[1] * 0.07 - n[1] * 0.07],
                    [c[0] + d[0] * 0.07 - n[0] * 0.07, c[2] + d[1] * 0.07 - n[1] * 0.07],
                    [c[0] + d[0] * 0.07 + n[0] * 0.07, c[2] + d[1] * 0.07 + n[1] * 0.07],
                    [c[0] - d[0] * 0.07 + n[0] * 0.07, c[2] - d[1] * 0.07 + n[1] * 0.07],
                ];
                for i in 0..4 {
                    let (a, b) = (corners[i], corners[(i + 1) % 4]);
                    let mid = [(a[0] + b[0]) * 0.5 - c[0], (a[1] + b[1]) * 0.5 - c[2]];
                    let quad = [[a[0], c[1], a[1]], [b[0], c[1], b[1]], [b[0], c[1] + 0.72, b[1]], [a[0], c[1] + 0.72, a[1]]];
                    mesh.surface(&quad, [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], Layer::Concrete, [0.45, 0.46, 0.48], [mid[0], 0.0, mid[1]]);
                }
                s += 4.0;
            }
        }
    }
}

/// Toute la voirie : routes, carrefours, îlots des ronds-points.
#[must_use]
pub fn road_mesh(world: &World, strips: &[RoadStrip]) -> UrbanMesh {
    let mut mesh = UrbanMesh::default();
    let mut ends: HashMap<u32, Vec<(usize, bool)>> = HashMap::new();
    for (i, strip) in strips.iter().enumerate() {
        ends.entry(strip.nodes[0]).or_default().push((i, true));
        ends.entry(strip.nodes[1]).or_default().push((i, false));
    }
    let tangent = |strip: &RoadStrip, start: bool| -> [f32; 2] {
        let n = strip.points.len();
        let (a, b) = if start { (strip.points[0], strip.points[1.min(n - 1)]) } else { (strip.points[n - 1], strip.points[n.saturating_sub(2)]) };
        let d = [b[0] - a[0], b[2] - a[2]];
        let l = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-6);
        [d[0] / l, d[1] / l]
    };
    let mut trims = vec![[0.0f32; 2]; strips.len()];
    let mut junctions = Vec::new();
    // Ordre déterministe des nœuds (le HashMap n'est pas ordonné).
    let mut nodes: Vec<_> = ends.into_iter().collect();
    nodes.sort_by_key(|(id, _)| *id);
    for (_, list) in &nodes {
        let is_junction = match list.as_slice() {
            [_] => false,
            [(i, si), (j, sj)] => {
                let (ta, tb) = (tangent(&strips[*i], *si), tangent(&strips[*j], *sj));
                ta[0] * tb[0] + ta[1] * tb[1] > -0.94
            }
            _ => true,
        };
        if !is_junction {
            continue;
        }
        let widest = list.iter().map(|(i, _)| Profile::of(strips[*i].spec).half_width()).fold(0.0, f32::max);
        let walk = list.iter().map(|(i, _)| Profile::of(strips[*i].spec).sidewalk).fold(0.0, f32::max);
        for (i, start) in list {
            trims[*i][usize::from(!*start)] = widest + walk + 0.5;
        }
        let (i, start) = list[0];
        let p = if start { strips[i].points[0] } else { strips[i].points[strips[i].points.len() - 1] };
        let y = list
            .iter()
            .map(|(k, s)| {
                let pts = &strips[*k].points;
                if *s { pts[0][1] } else { pts[pts.len() - 1][1] }
            })
            .fold(f32::NEG_INFINITY, f32::max);
        junctions.push(([p[0], y, p[2]], widest + 0.3));
    }

    for (strip, trim) in strips.iter().zip(&trims) {
        let inner = strip.ring_center.map(|c| {
            let path = Path::new(&strip.points);
            let (p, n) = path.at(0.0);
            if (c[0] - p[0]) * n[0] + (c[1] - p[2]) * n[1] > 0.0 { 1.0 } else { -1.0 }
        });
        road(world, &mut mesh, strip, *trim, inner);
    }
    for (center, radius) in junctions {
        disc(&mut mesh, center, radius, 0.015, Layer::Asphalt, WHITE);
    }
    // Îlots centraux des ronds-points : pelouse surélevée bordée.
    for roundabout in world.roads().roundabouts() {
        let Some(strip) = strips.iter().find(|s| s.ring_center.is_some_and(|c| {
            (c[0] - roundabout.center.x.to_f32()).abs() < 0.01 && (c[1] - roundabout.center.y.to_f32()).abs() < 0.01
        })) else {
            continue;
        };
        let c = [roundabout.center.x.to_f32(), strip.points[0][1], roundabout.center.y.to_f32()];
        let r = roundabout.radius_m.to_f32() - Profile::of(roundabout.spec).half_width();
        if r > 1.0 {
            disc(&mut mesh, c, r, CURB_M, Layer::Lawn, [0.85, 1.0, 0.85]);
            disc(&mut mesh, c, r - 0.05, 0.0, Layer::Asphalt, WHITE);
            ring_wall(&mut mesh, c, r, CURB_M);
            skirt_disc(world, &mut mesh, c, r);
        }
    }
    mesh
}

fn disc(mesh: &mut UrbanMesh, center: [f32; 3], radius: f32, dy: f32, layer: Layer, tint: [f32; 3]) {
    let n = 32;
    let pts: Vec<[f32; 3]> = (0..n)
        .map(|k| {
            let a = k as f32 / n as f32 * std::f32::consts::TAU;
            [center[0] + radius * a.cos(), center[1] + dy, center[2] + radius * a.sin()]
        })
        .collect();
    mesh.surface(&pts, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], layer, tint, UP);
}

fn ring_wall(mesh: &mut UrbanMesh, center: [f32; 3], radius: f32, height: f32) {
    let n = 32;
    for k in 0..n {
        let (a0, a1) = (k as f32 / n as f32 * std::f32::consts::TAU, (k + 1) as f32 / n as f32 * std::f32::consts::TAU);
        let p0 = [center[0] + radius * a0.cos(), center[2] + radius * a0.sin()];
        let p1 = [center[0] + radius * a1.cos(), center[2] + radius * a1.sin()];
        let quad = [[p0[0], center[1], p0[1]], [p1[0], center[1], p1[1]], [p1[0], center[1] + height, p1[1]], [p0[0], center[1] + height, p0[1]]];
        let mid = (a0 + a1) * 0.5;
        mesh.surface(&quad, [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], Layer::Concrete, [0.95, 0.95, 0.95], [mid.cos(), 0.0, mid.sin()]);
    }
}

/// Sous l'îlot : rien à cacher (la chaussée de l'anneau l'entoure) ; on
/// ferme seulement le dessous pour les vues rasantes.
fn skirt_disc(_world: &World, _mesh: &mut UrbanMesh, _center: [f32; 3], _radius: f32) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene;
    use crate::urban::LAYER_ENCODING;
    use std::collections::HashSet;

    fn layers(mesh: &UrbanMesh) -> HashSet<usize> {
        mesh.colors.iter().map(|c| (c[3] * LAYER_ENCODING).round() as usize).collect()
    }

    #[test]
    fn profiles_are_consistent_for_the_whole_catalogue() {
        for (key, spec) in RoadSpec::CATALOGUE {
            let p = Profile::of(spec);
            let lanes = p.lanes();
            assert_eq!(lanes.len(), usize::from(spec.total_lanes()), "{key}");
            // Les voies sont dans la chaussée et ne se chevauchent pas.
            for w in lanes.windows(2) {
                assert!(w[0].1 <= w[1].0 + 1e-4, "{key} : voies qui se chevauchent");
            }
            for (o0, o1, _) in &lanes {
                assert!(*o0 >= -p.half_width() - 1e-4 && *o1 <= p.half_width() + 1e-4, "{key} : voie hors chaussée");
            }
            // Circulation à droite : les voies aller sont à droite de l'axe.
            if p.two_way() {
                assert!(lanes.iter().all(|(o0, o1, f)| if *f { *o0 >= 0.0 } else { *o1 <= 0.0 }), "{key}");
            }
            assert!(p.reach() >= p.half_width());
        }
        assert!(Profile::of(RoadSpec::HIGHWAY).half_width() > Profile::of(RoadSpec::AVENUE).half_width());
        assert_eq!(Profile::of(RoadSpec::HIGHWAY).sidewalk, 0.0);
        assert!(Profile::of(RoadSpec::BOULEVARD).planted_median);
    }

    fn strip(spec: RoadSpec, progress: f32) -> RoadStrip {
        RoadStrip {
            points: (0..=20).map(|i| [10.0 + i as f32 * 5.0, 1.0, 50.0]).collect(),
            half_width_m: Profile::of(spec).half_width(),
            spec,
            nodes: [0, 1],
            progress,
            ring_center: None,
        }
    }

    #[test]
    fn each_road_type_gets_its_details() {
        let world = sim_core::World::new(sim_core::SimConfig::default(), 1, sim_core::Terrain::flat(32, 32));
        let mesh_of = |spec, progress| {
            let mut mesh = UrbanMesh::default();
            road(&world, &mut mesh, &strip(spec, progress), [0.0, 0.0], None);
            assert!(mesh.is_sane());
            mesh
        };
        let street = layers(&mesh_of(RoadSpec::TWO_WAY, 1.0));
        assert!(street.contains(&Layer::Sidewalk.index()) && street.contains(&Layer::Marking.index()));
        let one_way = layers(&mesh_of(RoadSpec::ONE_WAY, 1.0));
        assert!(one_way.contains(&Layer::Arrow.index()), "flèches de sens unique");
        let highway = layers(&mesh_of(RoadSpec::HIGHWAY, 1.0));
        assert!(highway.contains(&Layer::Guardrail.index()) && !highway.contains(&Layer::Sidewalk.index()));
        let boulevard = layers(&mesh_of(RoadSpec::BOULEVARD, 1.0));
        assert!(boulevard.contains(&Layer::Lawn.index()), "terre-plein planté");
        let gravel = layers(&mesh_of(RoadSpec::GRAVEL, 1.0));
        assert!(gravel.contains(&Layer::Gravel.index()) && !gravel.contains(&Layer::Marking.index()));
        let works = layers(&mesh_of(RoadSpec::AVENUE, 0.4));
        assert!(works.contains(&Layer::Works.index()) && works.contains(&Layer::Beacon.index()));
        assert!(!works.contains(&Layer::Marking.index()) && !works.contains(&Layer::Sidewalk.index()));
    }

    #[test]
    fn demo_network_renders_with_roundabout_island() {
        let mut world = scene::demo_city();
        for _ in 0..3000 {
            world.tick();
        }
        assert!(!world.roads().roundabouts().is_empty(), "la démo a un rond-point");
        let strips = scene::roads(&world);
        let mesh = road_mesh(&world, &strips);
        assert!(mesh.is_sane());
        let used = layers(&mesh);
        for layer in [Layer::Asphalt, Layer::Marking, Layer::Sidewalk, Layer::Lawn, Layer::Guardrail, Layer::Gravel] {
            assert!(used.contains(&layer.index()), "{layer:?} absent de la démo");
        }
    }
}
