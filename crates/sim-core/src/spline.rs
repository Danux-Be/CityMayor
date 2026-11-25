//! Géométrie des segments : **Bézier cubique en point fixe**.
//!
//! # Pourquoi une Bézier cubique et pas une spline Catmull-Rom ou une NURBS ?
//! - **Cubique** : c'est le plus petit degré qui donne une courbure continue
//!   (une quadratique a une discontinuité de courbure aux jonctions, visible en
//!   rendu et pénible pour le trafic) ;
//! - **Bézier** : les points de contrôle s'interprètent comme des *poignées*
//!   d'outil de construction (comme un logiciel vectoriel), donc l'UI peut les
//!   exposer directement au joueur ;
//! - **locale** : déplacer une poignée ne change que les segments incidents.
//!   Une Catmull-Rom globale propagerait les changements à tout le réseau —
//!   inacceptable pour un outil temps réel.
//!
//! # Représentation dans le graphe
//! Chaque [`crate::road::RoadNode`] porte **une** poignée sortante (`handle`),
//! en mètres. Un segment `a → b` est construit ainsi :
//! ```text
//! c0 = a.pos + a.handle      c1 = b.pos - b.handle
//! ```
//! De ce fait, deux segments qui partagent un nœud ont la **même tangente** en
//! ce nœud : la continuité G1 est structurelle, pas une convention. Une poignée
//! nulle (`Point::default()`, la valeur par défaut) produit exactement une
//! droite : les routes droites ne coûtent rien et restent exactes.
//!
//! # Invariants
//! - `arc_length` est une somme de cordes (`Fixed`, donc bit-à-bit stable) ;
//!   l'erreur est `O(1/n²)` et bornée sous 1 cm pour les pas utilisés.
//! - `min_curvature_radius` renvoie `None` si la courbe est quasi droite
//!   (dénominateur colinéaire) : jamais une division par zéro, jamais `inf`.
//! - Aucune méthode ne panique sur une courbe dégénérée : les fonctions
//!   renvoient `Option`/valeurs bornées.

use crate::fixed::Fixed;
use crate::road::Point;

/// Rayon de courbure minimal recommandé pour une route (20 m ≈ un virage
/// serré à 50 km/h). Utilisé par l'outil de construction pour colorer le ghost
/// en rouge, **pas** par la simulation pour refuser un segment : on ne bloque
/// jamais une intention de joueur sans raison mécanique.
pub const MIN_RECOMMENDED_RADIUS_M: Fixed = Fixed::from_int(20);

/// Pas d'échantillonnage cible pour les longueurs d'arc et polylignes (mètres).
const TARGET_SAMPLE_STEP_M: Fixed = Fixed::from_int(2);

/// Bézier cubique à points de contrôle absolus (mètres).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BezierCubic {
    pub p0: Point,
    pub c0: Point,
    pub c1: Point,
    pub p1: Point,
}

impl BezierCubic {
    /// Segment droit : les points de contrôle sont confondus avec les extrémités.
    #[must_use]
    pub const fn straight(a: Point, b: Point) -> Self {
        Self {
            p0: a,
            c0: a,
            c1: b,
            p1: b,
        }
    }

    /// Construit la courbe à partir des poignées **sortantes** des deux nœuds.
    #[must_use]
    pub fn from_handles(a: Point, a_handle: Point, b: Point, b_handle: Point) -> Self {
        Self {
            p0: a,
            c0: Point::new(a.x + a_handle.x, a.y + a_handle.y),
            c1: Point::new(b.x - b_handle.x, b.y - b_handle.y),
            p1: b,
        }
    }

    #[must_use]
    pub fn is_straight(&self) -> bool {
        self.c0 == self.p0 && self.c1 == self.p1
    }

    /// Évaluation par l'algorithme de de Casteljau (numériquement stable).
    #[must_use]
    pub fn eval(self, t: Fixed) -> Point {
        let a = Point::lerp(self.p0, self.c0, t);
        let b = Point::lerp(self.c0, self.c1, t);
        let c = Point::lerp(self.c1, self.p1, t);
        let d = Point::lerp(a, b, t);
        let e = Point::lerp(b, c, t);
        Point::lerp(d, e, t)
    }

    /// Dérivée première (non normalisée) : direction de circulation et
    /// tangente de mesh. Le facteur 3 est conservé pour que `|d|` soit
    /// proportionnel à la vitesse paramétrique.
    #[must_use]
    pub fn derivative(self, t: Fixed) -> Point {
        let one_minus_t = Fixed::ONE - t;
        let three = Fixed::from_int(3);
        let a = Point::new(self.c0.x - self.p0.x, self.c0.y - self.p0.y);
        let b = Point::new(self.c1.x - self.c0.x, self.c1.y - self.c0.y);
        let c = Point::new(self.p1.x - self.c1.x, self.p1.y - self.c1.y);
        let ca = one_minus_t.mul(one_minus_t);
        let cb = one_minus_t.mul(t).mul(Fixed::from_int(2));
        let cc = t.mul(t);
        Point::new(
            three.mul(ca.mul(a.x) + cb.mul(b.x) + cc.mul(c.x)),
            three.mul(ca.mul(a.y) + cb.mul(b.y) + cc.mul(c.y)),
        )
    }

    /// Estimation de la longueur de la courbe (corde + poignées) : sert à
    /// choisir un nombre de pas proportionnel au besoin réel.
    #[must_use]
    fn length_estimate(self) -> Fixed {
        self.p0.distance(self.p1) + self.p0.distance(self.c0) + self.p1.distance(self.c1)
    }

    /// Nombre de pas d'échantillonnage pour viser un pas de ~2 m,
    /// borné dans `[8, 256]` (déterministe : ne dépend que de la géométrie).
    #[must_use]
    pub fn suggested_steps(self) -> u32 {
        let steps = self
            .length_estimate()
            .div(TARGET_SAMPLE_STEP_M)
            .ceil_to_int();
        steps.clamp(8, 256) as u32
    }

    /// Longueur d'arc approchée par somme de cordes.
    ///
    /// Avec `steps` pas réguliers en `t`, l'erreur est `O(1/steps²)` ; à
    /// `steps >= 8` par 2 m elle reste sous le centimètre pour des courbures
    /// de rayon ≥ 20 m. C'est l'invariant dont dépendent le coût de
    /// construction et le budget de trafic.
    #[must_use]
    pub fn arc_length(self, steps: u32) -> Fixed {
        let steps = steps.max(1);
        let mut total = Fixed::ZERO;
        let mut prev = self.p0;
        for i in 1..=steps {
            let t = ratio(i, steps);
            let p = self.eval(t);
            total += prev.distance(p);
            prev = p;
        }
        total
    }

    /// Longueur avec le nombre de pas recommandé.
    #[must_use]
    pub fn length(self) -> Fixed {
        self.arc_length(self.suggested_steps())
    }

    /// Écrit `steps + 1` points le long de la courbe (mesh et trafic).
    pub fn sample_into(self, steps: u32, out: &mut Vec<Point>) {
        let steps = steps.max(1);
        out.clear();
        out.reserve((steps + 1) as usize);
        out.push(self.p0);
        for i in 1..=steps {
            out.push(self.eval(ratio(i, steps)));
        }
    }

    /// Rayon de courbure minimal (m), ou `None` si la courbe est (presque)
    /// droite. Mesuré par le cercle circonscrit à trois échantillons
    /// consécutifs : `R = |AB|·|BC|·|CA| / (2·|AB × AC|)`.
    ///
    /// Une courbure qui varie le long du segment est résumée par son minimum :
    /// c'est le point le plus serré qui contraint la vitesse réelle.
    #[must_use]
    pub fn min_curvature_radius(self, steps: u32) -> Option<Fixed> {
        let steps = steps.clamp(4, 128);
        let mut points = Vec::with_capacity((steps + 1) as usize);
        self.sample_into(steps, &mut points);

        let mut min_radius: Option<Fixed> = None;
        for window in points.windows(3) {
            let Some(radius) = circumradius(window[0], window[1], window[2]) else {
                continue;
            };
            min_radius = Some(match min_radius {
                Some(current) if current <= radius => current,
                _ => radius,
            });
        }
        min_radius
    }

    /// Vrai si la courbe est dégénérée (extrémités confondues à 1 cm près).
    #[must_use]
    pub fn is_degenerate(self) -> bool {
        self.p0.distance(self.p1) <= Fixed::from_raw(1)
    }
}

/// Cercle circonscrit à trois points : `None` si les points sont colinéaires.
fn circumradius(a: Point, b: Point, c: Point) -> Option<Fixed> {
    let cross = cross(b, a, c);
    // `4·aire = 2·|cross|`. Un seuil à 1 cm² évite la division par un
    // dénominateur quasi nul (rayon astronomique, non informatif).
    let denominator = cross.abs().mul(Fixed::from_int(2));
    if denominator <= Fixed::from_raw(1) {
        return None;
    }
    let ab = a.distance(b);
    let bc = b.distance(c);
    let ca = c.distance(a);
    if ab.is_zero() || bc.is_zero() || ca.is_zero() {
        return None;
    }
    Some(ab.mul(bc).mul(ca).div(denominator))
}

/// Produit vectoriel `(b - a) × (c - a)` (z).
fn cross(a: Point, b: Point, c: Point) -> Fixed {
    let abx = b.x - a.x;
    let aby = b.y - a.y;
    let acx = c.x - a.x;
    let acy = c.y - a.y;
    abx.mul(acy) - aby.mul(acx)
}

/// `num / den` en point fixe, pour une fraction d'itération.
fn ratio(num: u32, den: u32) -> Fixed {
    Fixed::from_raw(((num as i128) << 16) as i64 / den as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: i32, y: i32) -> Point {
        Point::new(Fixed::from_int(x), Fixed::from_int(y))
    }

    fn assert_close(actual: Fixed, expected: Fixed, tolerance_m: i32, what: &str) {
        let delta = (actual - expected).abs();
        assert!(
            delta <= Fixed::from_int(tolerance_m),
            "{what}: {actual} vs {expected} (ecart {delta})"
        );
    }

    /// Quart de cercle de rayon 100 : la constante 0.5523 approxime un arc de
    /// cercle à 0.03 % près avec une seule cubique.
    fn quarter_circle_radius_100() -> BezierCubic {
        let k = Fixed::from_permille(552);
        BezierCubic::from_handles(pt(0, 0), Point::new(k.mul(Fixed::from_int(100)), Fixed::ZERO), pt(100, 100), Point::new(Fixed::ZERO, k.mul(Fixed::from_int(100))))
    }

    #[test]
    fn straight_curve_matches_the_line_it_replaces() {
        let a = pt(0, 0);
        let b = pt(37, 12);
        let curve = BezierCubic::straight(a, b);
        assert!(curve.is_straight());
        let mid = curve.eval(Fixed::HALF);
        assert_close(mid.x, Fixed::from_ratio(37, 2), 1, "x milieu");
        assert_close(mid.y, Fixed::from_int(6), 1, "y milieu");
        assert_close(curve.length(), a.distance(b), 1, "longueur droite");
        assert_eq!(curve.min_curvature_radius(32), None, "une droite n'a pas de rayon");
    }

    #[test]
    fn quarter_circle_length_and_curvature_are_physical() {
        let curve = quarter_circle_radius_100();
        let expected_arc = Fixed::from_ratio(15_708, 100); // π/2 · 100 ≈ 157.08 m
        assert_close(curve.length(), expected_arc, 2, "longueur du quart de cercle");

        let radius = curve.min_curvature_radius(64).expect("courbe non colinéaire");
        assert!(
            radius > Fixed::from_int(95) && radius < Fixed::from_int(105),
            "rayon mesuré {radius}, attendu ~100"
        );
        assert!(radius >= MIN_RECOMMENDED_RADIUS_M);
    }

    #[test]
    fn a_curve_is_longer_than_its_chord() {
        let curve = quarter_circle_radius_100();
        assert!(curve.length() > curve.p0.distance(curve.p1) + Fixed::from_int(10));
    }

    #[test]
    fn sampling_is_exact_at_the_endpoints() {
        let curve = quarter_circle_radius_100();
        let mut points = Vec::new();
        curve.sample_into(16, &mut points);
        assert_eq!(points.len(), 17);
        assert_eq!(points[0], curve.p0);
        assert_eq!(points[16], curve.p1);
    }

    #[test]
    fn degenerate_curve_is_flagged_not_panicking() {
        let curve = BezierCubic::straight(pt(4, 4), pt(4, 4));
        assert!(curve.is_degenerate());
        assert_eq!(curve.min_curvature_radius(8), None);
        assert_eq!(curve.arc_length(8), Fixed::ZERO);
    }

    #[test]
    fn arc_length_is_deterministic() {
        let curve = quarter_circle_radius_100();
        assert_eq!(curve.arc_length(37), curve.arc_length(37));
        // Convergence en O(1/n²) : doubler les pas réduit l'écart de ~4×.
        let coarse = curve.arc_length(16);
        let fine = curve.arc_length(64);
        assert!((coarse - fine).abs() < Fixed::from_int(1));
    }

    #[test]
    fn tangent_points_outward_at_both_ends() {
        let curve = quarter_circle_radius_100();
        let start = curve.derivative(Fixed::ZERO);
        let end = curve.derivative(Fixed::ONE);
        assert!(start.x > Fixed::from_int(100), "départ vers +x : {start:?}");
        assert_close(start.y, Fixed::ZERO, 1, "départ sans composante y");
        assert!(end.y > Fixed::from_int(100), "arrivée vers +y : {end:?}");
        assert_close(end.x, Fixed::ZERO, 1, "arrivée sans composante x");
    }
}
