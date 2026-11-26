//! Graphe routier : nœuds en coordonnées continues + segments orientés.
//!
//! # Pourquoi un graphe continu et pas une grille de routes ?
//! Les city-builders "modernes" (CS:Skylines, Transport Fever) ont besoin de
//! courbes, de diagonales et d'intersections à angle libre. Une grille de routes
//! rend l'outil de construction rigide et rend les embouteillages illisibles.
//! On garde donc :
//! - **positions continues** (`Fixed`, mètres) pour les nœuds ;
//! - **segments courbes** ([`BezierCubic`]) paramétrés par `t ∈ [0, 1]` : le
//!   trafic et le pathfinding raisonnent toujours en `SegmentId + t`, jamais en
//!   coordonnées ; changer la géométrie d'une route n'invalide donc ni un
//!   chemin ni un agent en transit ;
//! - une **adjacence CSR** (`offsets` + `edges`) reconstruite à la demande :
//!   c'est la structure qui rend le pathfinding cache-friendly (voisins
//!   contigus en mémoire) et qui évite les `Vec<Vec<>>` dans les hot loops.
//!
//! # Invariants
//! - Un segment est unique pour une paire de nœuds non ordonnée
//!   (pas de doublon A→B / B→A) : sinon le trafic aurait deux arêtes parallèles.
//! - Les segments sont supprimés par tombstone (`Option`), les ids ne sont
//!   **jamais** réutilisés : un id périmé reste inoffensif.
//! - `neighbors()` exige une adjacence à jour (`ensure_topology`). Toute
//!   mutation de topologie s'accumule et ne coûte qu'un rebuild par tick.
//! - **Une poignée par nœud** (tangente sortante) : deux segments voisins
//!   partagent la même tangente, donc le réseau est G1 par construction. Une
//!   poignée nulle donne une droite exacte (cf. [`crate::spline`]).
//! - Toucher une poignée ne recalcule que les segments incidents
//!   ([`RoadNetwork::set_node_handle`]), jamais tout le graphe.

use std::collections::HashMap;

use crate::fixed::Fixed;
use crate::ids::{NodeId, SegmentId};
use crate::spline::BezierCubic;

/// Point du plan en mètres (point fixe).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    pub x: Fixed,
    pub y: Fixed,
}

impl Point {
    #[must_use]
    pub const fn new(x: Fixed, y: Fixed) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub fn distance_squared(self, other: Self) -> Fixed {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx.mul(dx) + dy.mul(dy)
    }

    #[must_use]
    pub fn distance(self, other: Self) -> Fixed {
        self.distance_squared(other).sqrt()
    }

    #[must_use]
    pub fn lerp(self, other: Self, t: Fixed) -> Self {
        Self {
            x: Fixed::lerp(self.x, other.x, t),
            y: Fixed::lerp(self.y, other.y, t),
        }
    }
}

/// Caractéristiques d'une voie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoadSpec {
    /// Voies autorisées de `a` vers `b`.
    pub forward_lanes: u8,
    /// Voies autorisées de `b` vers `a` (0 = sens unique).
    pub backward_lanes: u8,
    pub speed_kph: u16,
}

impl RoadSpec {
    pub const TWO_WAY: Self = Self {
        forward_lanes: 1,
        backward_lanes: 1,
        speed_kph: 50,
    };

    pub const AVENUE: Self = Self {
        forward_lanes: 2,
        backward_lanes: 2,
        speed_kph: 70,
    };

    pub const ONE_WAY: Self = Self {
        forward_lanes: 2,
        backward_lanes: 0,
        speed_kph: 50,
    };

    #[must_use]
    pub const fn total_lanes(self) -> u8 {
        self.forward_lanes + self.backward_lanes
    }

    /// # Errors
    /// Aucune voie, ou vitesse nulle.
    pub fn validate(self) -> Result<(), RoadError> {
        if self.total_lanes() == 0 {
            return Err(RoadError::NoLanes);
        }
        if self.speed_kph == 0 {
            return Err(RoadError::ZeroSpeed);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoadNode {
    pub pos: Point,
    /// Poignée de tangente **sortante**, en mètres (vecteur depuis `pos`).
    ///
    /// `Point::default()` = nœud anguleux. Non nulle = virage lissé, et la
    /// continuité G1 est automatique avec les segments voisins.
    pub handle: Point,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoadSegment {
    pub a: NodeId,
    pub b: NodeId,
    pub spec: RoadSpec,
    /// Géométrie réelle, orientée `a → b`.
    pub curve: BezierCubic,
    /// Longueur d'arc précalculée (mètres) : budget, trafic, coût de
    /// construction. Recalculée quand une poignée incidente change.
    pub length_m: Fixed,
}

/// Arête orientée d'adjacence CSR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EdgeRef {
    pub segment: SegmentId,
    pub from: NodeId,
    pub to: NodeId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoadError {
    UnknownNode(NodeId),
    UnknownSegment(SegmentId),
    SelfLoop,
    DuplicateSegment,
    NoLanes,
    ZeroSpeed,
    DegenerateLength,
}

impl RoadError {
    /// Clé du message affiché au joueur (fichiers de langue).
    #[must_use]
    pub const fn message_key(&self) -> &'static str {
        match self {
            RoadError::UnknownNode(_) => "error-road-unknown-node",
            RoadError::UnknownSegment(_) => "error-road-unknown-segment",
            RoadError::SelfLoop => "error-road-self-loop",
            RoadError::DuplicateSegment => "error-road-duplicate",
            RoadError::NoLanes => "error-road-no-lanes",
            RoadError::ZeroSpeed => "error-road-zero-speed",
            RoadError::DegenerateLength => "error-road-too-short",
        }
    }

    /// Toutes les variantes (valeurs d'exemple pour les identifiants) : sert
    /// aux tests de complétude des fichiers de langue.
    pub const ALL: [RoadError; 7] = [
        RoadError::UnknownNode(NodeId::new(0)),
        RoadError::UnknownSegment(SegmentId::new(0)),
        RoadError::SelfLoop,
        RoadError::DuplicateSegment,
        RoadError::NoLanes,
        RoadError::ZeroSpeed,
        RoadError::DegenerateLength,
    ];
}

impl core::fmt::Display for RoadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RoadError::UnknownNode(n) => write!(f, "nœud inconnu {n}"),
            RoadError::UnknownSegment(s) => write!(f, "segment inconnu {s}"),
            RoadError::SelfLoop => write!(f, "un segment ne peut pas boucler sur son nœud"),
            RoadError::DuplicateSegment => write!(f, "ces deux nœuds sont déjà reliés"),
            RoadError::NoLanes => write!(f, "segment sans voie"),
            RoadError::ZeroSpeed => write!(f, "vitesse nulle"),
            RoadError::DegenerateLength => write!(f, "segment de longueur nulle"),
        }
    }
}

impl std::error::Error for RoadError {}

/// Réseau routier (nœuds + segments + adjacence CSR).
#[derive(Clone, Debug, Default)]
pub struct RoadNetwork {
    nodes: Vec<RoadNode>,
    segments: Vec<Option<RoadSegment>>,
    live_segments: u32,
    /// Index paire -> segment. `HashMap` uniquement pour les *lookups*
    /// (jamais itéré : l'ordre d'itération d'un HashMap n'est pas déterministe).
    pair_index: HashMap<(u32, u32), SegmentId>,
    offsets: Vec<u32>,
    edges: Vec<EdgeRef>,
    adjacency_stale: bool,
}

impl RoadNetwork {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn node_count(&self) -> u32 {
        self.nodes.len() as u32
    }

    #[must_use]
    pub fn live_segments(&self) -> u32 {
        self.live_segments
    }

    #[must_use]
    pub fn node(&self, id: NodeId) -> Option<&RoadNode> {
        self.nodes.get(id.index())
    }

    #[must_use]
    pub fn segment(&self, id: SegmentId) -> Option<&RoadSegment> {
        self.segments.get(id.index()).and_then(Option::as_ref)
    }

    pub fn segments(&self) -> impl Iterator<Item = (SegmentId, &RoadSegment)> + '_ {
        self.segments
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|s| (SegmentId::new(i as u32), s)))
    }

    pub fn nodes(&self) -> impl Iterator<Item = (NodeId, &RoadNode)> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (NodeId::new(i as u32), n))
    }

    pub fn add_node(&mut self, pos: Point) -> NodeId {
        let id = NodeId::new(self.nodes.len() as u32);
        self.nodes.push(RoadNode {
            pos,
            handle: Point::default(),
        });
        self.adjacency_stale = true;
        id
    }

    /// Déplace la poignée d'un nœud et recalcule la géométrie **des seuls**
    /// segments incidents.
    ///
    /// Coût `O(segments)` (balayage des segments pour trouver les incidents) :
    /// acceptable car appelé par l'outil de construction au relâchement de la
    /// souris, jamais dans une boucle de tick. Un index nœud → segments
    /// remplacera ce balayage quand l'édition deviendra continue.
    ///
    /// # Errors
    /// Nœud inconnu.
    pub fn set_node_handle(&mut self, node: NodeId, handle: Point) -> Result<(), RoadError> {
        let Some(slot) = self.nodes.get_mut(node.index()) else {
            return Err(RoadError::UnknownNode(node));
        };
        slot.handle = handle;
        self.refresh_geometry_at(node);
        Ok(())
    }

    /// Recalcule courbe + longueur de tous les segments touchant `node`.
    fn refresh_geometry_at(&mut self, node: NodeId) {
        for index in 0..self.segments.len() {
            let Some(seg) = self.segments[index].as_ref() else {
                continue;
            };
            if seg.a != node && seg.b != node {
                continue;
            }
            let (a, b) = (seg.a, seg.b);
            let Some(curve) = self.curve_between(a, b) else {
                continue;
            };
            if let Some(slot) = self.segments[index].as_mut() {
                slot.curve = curve;
                slot.length_m = curve.length();
            }
        }
    }

    /// Courbe orientée `a → b` construite depuis les poignées des deux nœuds.
    #[must_use]
    pub fn curve_between(&self, a: NodeId, b: NodeId) -> Option<BezierCubic> {
        let na = self.node(a)?;
        let nb = self.node(b)?;
        Some(BezierCubic::from_handles(
            na.pos, na.handle, nb.pos, nb.handle,
        ))
    }

    /// Polyligne pour le mesh et les agents : `steps + 1` points.
    #[must_use]
    pub fn segment_polyline(&self, id: SegmentId, steps: u32) -> Option<Vec<Point>> {
        let seg = self.segment(id)?;
        let mut points = Vec::new();
        seg.curve.sample_into(steps, &mut points);
        Some(points)
    }

    /// Crée un segment. Sens : `forward_lanes` = `a -> b`.
    ///
    /// # Errors
    /// Nœud inconnu, boucle, doublon, spec invalide, longueur nulle.
    pub fn connect(&mut self, a: NodeId, b: NodeId, spec: RoadSpec) -> Result<SegmentId, RoadError> {
        spec.validate()?;
        if a == b {
            return Err(RoadError::SelfLoop);
        }
        let curve = self
            .curve_between(a, b)
            .ok_or(RoadError::UnknownNode(a))?;
        if self.pair_index.contains_key(&pair_key(a, b)) {
            return Err(RoadError::DuplicateSegment);
        }
        if curve.is_degenerate() {
            // < 1 cm : artefact d'outil de construction, jamais une route.
            return Err(RoadError::DegenerateLength);
        }
        let length_m = curve.length();
        let id = SegmentId::new(self.segments.len() as u32);
        self.segments.push(Some(RoadSegment {
            a,
            b,
            spec,
            curve,
            length_m,
        }));
        self.live_segments += 1;
        self.pair_index.insert(pair_key(a, b), id);
        self.adjacency_stale = true;
        Ok(id)
    }

    /// Supprime un segment (tombstone : l'id reste réservé).
    ///
    /// # Errors
    /// Segment inconnu ou déjà supprimé.
    pub fn disconnect(&mut self, id: SegmentId) -> Result<(), RoadError> {
        let Some(slot) = self.segments.get_mut(id.index()) else {
            return Err(RoadError::UnknownSegment(id));
        };
        let Some(seg) = slot.take() else {
            return Err(RoadError::UnknownSegment(id));
        };
        self.pair_index.remove(&pair_key(seg.a, seg.b));
        self.live_segments -= 1;
        self.adjacency_stale = true;
        Ok(())
    }

    /// Vrai si un segment relie directement ces deux nœuds (peu importe le sens).
    #[must_use]
    pub fn find_segment(&self, a: NodeId, b: NodeId) -> Option<SegmentId> {
        self.pair_index.get(&pair_key(a, b)).copied()
    }

    #[must_use]
    pub fn topology_is_stale(&self) -> bool {
        self.adjacency_stale
    }

    /// Reconstruit l'adjacence seulement si nécessaire.
    pub fn ensure_topology(&mut self) {
        if self.adjacency_stale {
            self.rebuild_adjacency();
        }
    }

    /// Reconstruit l'adjacence CSR. Coût `O(E log E)` (tri par nœud source).
    ///
    /// On trie plutôt que de compter : le tri rend le résultat indépendant de
    /// l'ordre d'insertion, donc stable après sérialisation/désérialisation.
    pub fn rebuild_adjacency(&mut self) {
        let node_count = self.nodes.len();
        let mut raw: Vec<(u32, SegmentId, EdgeRef)> = Vec::with_capacity(self.live_segments as usize * 2);
        for (i, slot) in self.segments.iter().enumerate() {
            let Some(seg) = slot else { continue };
            let sid = SegmentId::new(i as u32);
            if seg.spec.forward_lanes > 0 {
                raw.push((
                    seg.a.raw(),
                    sid,
                    EdgeRef {
                        segment: sid,
                        from: seg.a,
                        to: seg.b,
                    },
                ));
            }
            if seg.spec.backward_lanes > 0 {
                raw.push((
                    seg.b.raw(),
                    sid,
                    EdgeRef {
                        segment: sid,
                        from: seg.b,
                        to: seg.a,
                    },
                ));
            }
        }
        raw.sort_unstable_by_key(|(from, sid, _)| (*from, sid.raw()));
        self.offsets.clear();
        self.offsets.resize(node_count + 1, 0);
        for (from, _, _) in &raw {
            self.offsets[*from as usize + 1] += 1;
        }
        for i in 1..=node_count {
            self.offsets[i] += self.offsets[i - 1];
        }
        self.edges = raw.into_iter().map(|(_, _, e)| e).collect();
        self.adjacency_stale = false;
    }

    /// Voisinage sortant d'un nœud (slice contiguë, zéro allocation).
    ///
    /// # Invariants
    /// Renvoie un slice vide si l'adjacence est périmée : appeler
    /// [`RoadNetwork::ensure_topology`] avant (le `World` le fait une fois par
    /// tick, pas par requête).
    #[must_use]
    pub fn neighbors(&self, node: NodeId) -> &[EdgeRef] {
        debug_assert!(!self.adjacency_stale, "adjacence routiere perimee");
        if self.adjacency_stale {
            return &[];
        }
        let i = node.index();
        if i + 1 >= self.offsets.len() {
            return &[];
        }
        let start = self.offsets[i] as usize;
        let end = self.offsets[i + 1] as usize;
        &self.edges[start..end]
    }

    /// Rasterise le réseau sur la grille : une tuile est « desservie » si elle
    /// est à moins de `radius_tiles` (distance euclidienne) de la **courbe**
    /// d'un segment.
    ///
    /// La courbe est échantillonnée (pas ~2 m) puis parcourue tuile par tuile
    /// (DDA) : la desserte suit donc réellement la route, ce qui compte dès
    /// qu'une avenue courbe borde une parcelle. Coût
    /// `O(échantillons × longueur_du_parcours × (2r+1)²)` ; à remplacer par un
    /// index spatial quand le réseau dépassera quelques milliers de segments.
    #[must_use]
    pub fn rasterize_access(
        &self,
        width: u32,
        height: u32,
        radius_tiles: i32,
        tile_size_m: i32,
    ) -> Vec<bool> {
        let mut access = vec![false; (width as usize) * (height as usize)];
        if radius_tiles < 0 || tile_size_m <= 0 {
            return access;
        }
        let radius = radius_tiles;
        let radius_sq = radius * radius;
        let mut scratch: Vec<Point> = Vec::new();

        for (_, seg) in self.segments() {
            seg.curve.sample_into(seg.curve.suggested_steps(), &mut scratch);
            let mut prev = scratch.first().map(|p| to_tile(*p, tile_size_m));
            for point in &scratch {
                let current = to_tile(*point, tile_size_m);
                let start = prev.unwrap_or(current);
                walk_tiles(start, current, |x, y| {
                    for oy in -radius..=radius {
                        for ox in -radius..=radius {
                            if ox * ox + oy * oy > radius_sq {
                                continue;
                            }
                            let tx = x + ox;
                            let ty = y + oy;
                            if tx < 0 || ty < 0 || tx >= width as i32 || ty >= height as i32 {
                                continue;
                            }
                            access[ty as usize * width as usize + tx as usize] = true;
                        }
                    }
                });
                prev = Some(current);
            }
        }
        access
    }
}

fn pair_key(a: NodeId, b: NodeId) -> (u32, u32) {
    if a.raw() <= b.raw() {
        (a.raw(), b.raw())
    } else {
        (b.raw(), a.raw())
    }
}

/// Parcourt les tuiles entre deux points entiers (Bresenham) et appelle
/// `visit` sur chacune, extrémités comprises.
fn walk_tiles((x0, y0): (i32, i32), (x1, y1): (i32, i32), mut visit: impl FnMut(i32, i32)) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let (mut x, mut y) = (x0, y0);
    let mut err = dx + dy;
    loop {
        visit(x, y);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// Projection mètres -> coordonnée de tuile (plancher, correct en négatif).
fn to_tile(p: Point, tile_size_m: i32) -> (i32, i32) {
    let tsm = (tile_size_m as i64) << 16;
    ((p.x.raw() / tsm) as i32, (p.y.raw() / tsm) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net() -> (RoadNetwork, NodeId, NodeId, NodeId) {
        let mut n = RoadNetwork::new();
        let a = n.add_node(Point::new(Fixed::from_int(0), Fixed::from_int(0)));
        let b = n.add_node(Point::new(Fixed::from_int(10), Fixed::from_int(0)));
        let c = n.add_node(Point::new(Fixed::from_int(10), Fixed::from_int(10)));
        n.connect(a, b, RoadSpec::TWO_WAY).unwrap();
        n.connect(b, c, RoadSpec::TWO_WAY).unwrap();
        (n, a, b, c)
    }

    #[test]
    fn length_is_euclidean() {
        let (n, a, b, _) = net();
        let seg = n.find_segment(a, b).unwrap();
        assert_eq!(n.segment(seg).unwrap().length_m, Fixed::from_int(10));
    }

    #[test]
    fn duplicate_and_self_loop_are_rejected() {
        let (mut n, a, b, _) = net();
        assert_eq!(
            n.connect(a, b, RoadSpec::TWO_WAY).unwrap_err(),
            RoadError::DuplicateSegment
        );
        assert_eq!(
            n.connect(b, a, RoadSpec::TWO_WAY).unwrap_err(),
            RoadError::DuplicateSegment
        );
        assert_eq!(
            n.connect(a, a, RoadSpec::TWO_WAY).unwrap_err(),
            RoadError::SelfLoop
        );
    }

    #[test]
    fn csr_adjacency_is_bidirectional_for_two_way() {
        let (mut n, a, b, c) = net();
        n.ensure_topology();
        assert_eq!(n.neighbors(a).len(), 1);
        assert_eq!(n.neighbors(a)[0].to, b);
        assert_eq!(n.neighbors(b).len(), 2);
        assert_eq!(n.neighbors(c).len(), 1);
        assert_eq!(n.neighbors(c)[0].to, b);
    }

    #[test]
    fn one_way_has_single_direction() {
        let mut n = RoadNetwork::new();
        let a = n.add_node(Point::new(Fixed::ZERO, Fixed::ZERO));
        let b = n.add_node(Point::new(Fixed::from_int(5), Fixed::ZERO));
        n.connect(a, b, RoadSpec::ONE_WAY).unwrap();
        n.ensure_topology();
        assert_eq!(n.neighbors(a).len(), 1);
        assert!(n.neighbors(b).is_empty());
    }

    #[test]
    fn disconnect_keeps_topology_consistent() {
        let (mut n, a, b, _) = net();
        let seg = n.find_segment(a, b).unwrap();
        n.disconnect(seg).unwrap();
        n.ensure_topology();
        assert_eq!(n.live_segments(), 1);
        assert!(n.neighbors(a).is_empty());
        assert_eq!(n.neighbors(b).len(), 1);
        assert_eq!(
            n.disconnect(seg).unwrap_err(),
            RoadError::UnknownSegment(seg)
        );
    }

    #[test]
    fn junction_handles_give_g1_continuity() {
        let mut n = RoadNetwork::new();
        let a = n.add_node(Point::new(Fixed::ZERO, Fixed::ZERO));
        let b = n.add_node(Point::new(Fixed::from_int(100), Fixed::ZERO));
        let c = n.add_node(Point::new(Fixed::from_int(200), Fixed::from_int(100)));
        n.set_node_handle(b, Point::new(Fixed::from_int(30), Fixed::from_int(30)))
            .unwrap();

        let s1 = n.connect(a, b, RoadSpec::TWO_WAY).unwrap();
        let s2 = n.connect(b, c, RoadSpec::TWO_WAY).unwrap();

        let incoming = n.segment(s1).unwrap().curve.derivative(Fixed::ONE);
        let outgoing = n.segment(s2).unwrap().curve.derivative(Fixed::ZERO);
        assert_eq!(
            incoming, outgoing,
            "la tangente partagée doit être identique de part et d'autre du nœud"
        );
        assert!(!incoming.x.is_zero() && !incoming.y.is_zero());
    }

    #[test]
    fn handles_curve_the_segment_and_update_its_length() {
        let mut n = RoadNetwork::new();
        let a = n.add_node(Point::new(Fixed::from_int(4), Fixed::from_int(4)));
        let b = n.add_node(Point::new(Fixed::from_int(76), Fixed::from_int(4)));
        let straight_id = n.connect(a, b, RoadSpec::TWO_WAY).unwrap();
        let straight_len = n.segment(straight_id).unwrap().length_m;
        // Longueur d'arc = somme de cordes : 72 m à ~1 mm près (erreur O(1/n²)).
        let tolerance = Fixed::from_raw(1 << 10); // 1/64 m
        assert!(
            (straight_len - Fixed::from_int(72)).abs() < tolerance,
            "longueur droite: {straight_len}"
        );

        n.set_node_handle(a, Point::new(Fixed::from_int(20), Fixed::from_int(60)))
            .unwrap();
        n.set_node_handle(b, Point::new(Fixed::from_int(-20), Fixed::from_int(-60)))
            .unwrap();

        let curved = n.segment(straight_id).unwrap();
        assert!(!curved.curve.is_straight());
        assert!(
            curved.length_m > straight_len,
            "une courbe est plus longue que sa corde: {} vs {}",
            curved.length_m,
            straight_len
        );
        // La corde reste la distance entre les nœuds : la géométrie ne les déplace pas.
        assert!(
            (curved.curve.p0.distance(curved.curve.p1) - Fixed::from_int(72)).abs() < tolerance
        );
    }

    #[test]
    fn rasterization_follows_the_curve_not_the_chord() {
        let mut n = RoadNetwork::new();
        let a = n.add_node(Point::new(Fixed::from_int(4), Fixed::from_int(4)));
        let b = n.add_node(Point::new(Fixed::from_int(76), Fixed::from_int(4)));
        n.connect(a, b, RoadSpec::TWO_WAY).unwrap();
        let straight = n.rasterize_access(16, 16, 0, 8);

        // Poignées qui font bomber la route vers +y (nord).
        n.set_node_handle(a, Point::new(Fixed::from_int(20), Fixed::from_int(60)))
            .unwrap();
        n.set_node_handle(b, Point::new(Fixed::from_int(-20), Fixed::from_int(-60)))
            .unwrap();
        let curved = n.rasterize_access(16, 16, 0, 8);

        let mid = n
            .curve_between(a, b)
            .unwrap()
            .eval(Fixed::HALF);
        assert!(mid.y > Fixed::from_int(20), "la route doit bomber: {mid:?}");
        let (mx, my) = (
            (mid.x.raw() / (8 << 16)) as usize,
            (mid.y.raw() / (8 << 16)) as usize,
        );
        let bulge_tile = my * 16 + mx;
        assert!(!straight[bulge_tile], "la corde ne passe pas par le bombement");
        assert!(curved[bulge_tile], "la courbe, si");
    }

    #[test]
    fn polyline_is_ready_for_the_mesh() {
        let (n, a, b, _) = net();
        let id = n.find_segment(a, b).unwrap();
        let points = n.segment_polyline(id, 8).unwrap();
        assert_eq!(points.len(), 9);
        assert_eq!(points[0], n.node(a).unwrap().pos);
        assert_eq!(points[8], n.node(b).unwrap().pos);
    }

    #[test]
    fn unknown_node_handle_is_rejected() {
        let mut n = RoadNetwork::new();
        let id = n.add_node(Point::new(Fixed::ZERO, Fixed::ZERO));
        assert_eq!(
            n.set_node_handle(NodeId::new(99), Point::default()).unwrap_err(),
            RoadError::UnknownNode(NodeId::new(99))
        );
        assert!(n.set_node_handle(id, Point::default()).is_ok());
    }

    #[test]
    fn rasterization_marks_tiles_near_segment() {
        let mut n = RoadNetwork::new();
        let a = n.add_node(Point::new(Fixed::from_int(0), Fixed::from_int(40)));
        let b = n.add_node(Point::new(Fixed::from_int(80), Fixed::from_int(40)));
        n.connect(a, b, RoadSpec::TWO_WAY).unwrap();
        let access = n.rasterize_access(16, 16, 1, 8);
        // Segment sur y=40m = tuile 5, de x=0..10.
        assert!(access[5 * 16 + 5]);
        assert!(access[6 * 16 + 5], "rayon 1 tuile vers le bas");
        assert!(!access[10 * 16 + 5], "hors rayon");
        assert!(!access[0]);
    }
}
