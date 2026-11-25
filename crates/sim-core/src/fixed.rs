//! Arithmétique à point fixe Q48.16 — seule arithmétique autorisée dans la
//! simulation.
//!
//! Pourquoi pas `f32` ? Parce qu'un city-builder est un état *persistant* :
//! moindre écart d'arrondi (FMA, réassociation, changement de cible) et deux
//! joueurs divergent après 10 000 ticks. Le point fixe rend les résultats
//! bit-à-bit reproductibles, à coût quasi nul sur du `i64`.
//!
//! Précision : 1/65536 ≈ 15 µm. Plage : ±2^47 mètres. Largement suffisant pour
//! une carte de 100 km sans dépassement.

use core::fmt;
use core::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// Nombre de bits fractionnaires.
pub const FRAC_BITS: u32 = 16;
/// Représentation brute de `1.0`.
pub const ONE_RAW: i64 = 1 << FRAC_BITS;

/// Scalaire à point fixe signé, Q48.16.
///
/// `Ord`/`Eq` sont dérivés : l'ordre total porte sur la représentation brute,
/// ce qui est exactement l'ordre numérique (dénominateur commun).
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Fixed(i64);

impl Fixed {
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(ONE_RAW);
    pub const HALF: Self = Self(ONE_RAW / 2);

    #[must_use]
    pub const fn from_raw(raw: i64) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> i64 {
        self.0
    }

    #[must_use]
    pub const fn from_int(value: i32) -> Self {
        Self((value as i64) << FRAC_BITS)
    }

    /// `permille / 1000`, arrondi vers zéro. Sert à écrire les constantes de
    /// tuning en clair (`Fixed::from_permille(550)` = 0.55).
    #[must_use]
    pub const fn from_permille(permille: u16) -> Self {
        Self((permille as i64 * ONE_RAW) / 1000)
    }

    /// `num / den`, arrondi vers zéro.
    ///
    /// # Panics
    /// Si `den == 0` (erreur de programmation, pas une donnée joueur).
    #[must_use]
    pub const fn from_ratio(num: i32, den: i32) -> Self {
        assert!(den != 0, "Fixed::from_ratio: denominateur nul");
        Self((num as i64 * ONE_RAW) / den as i64)
    }

    #[must_use]
    pub fn to_f32(self) -> f32 {
        self.0 as f32 / ONE_RAW as f32
    }

    #[must_use]
    pub fn to_f64(self) -> f64 {
        self.0 as f64 / ONE_RAW as f64
    }

    /// Multiplication avec produit intermédiaire en `i128` : pas de
    /// débordement pour des valeurs de jeu réalistes.
    ///
    /// Méthode inhérente **et** opérateur `*` (même sémantique) : le point fixe
    /// est l'API primaire du simulateur, on veut pouvoir l'écrire `a.mul(b)`
    /// dans une chaîne sans importer `Mul`.
    #[must_use]
    #[allow(clippy::should_implement_trait)]
    pub fn mul(self, rhs: Self) -> Self {
        Self(((self.0 as i128 * rhs.0 as i128) >> FRAC_BITS) as i64)
    }

    /// # Panics
    /// Si `rhs` vaut zéro.
    #[must_use]
    #[allow(clippy::should_implement_trait)]
    pub fn div(self, rhs: Self) -> Self {
        assert!(rhs.0 != 0, "Fixed::div: division par zero");
        Self((((self.0 as i128) << FRAC_BITS) / rhs.0 as i128) as i64)
    }

    #[must_use]
    pub fn clamp(self, lo: Self, hi: Self) -> Self {
        if self < lo {
            lo
        } else if self > hi {
            hi
        } else {
            self
        }
    }

    /// Borne dans `[0, 1]` : convention utilisée pour tous les ratios du
    /// simulateur (demande, bonheur, valeur foncière, occupation).
    #[must_use]
    pub fn clamp01(self) -> Self {
        self.clamp(Self::ZERO, Self::ONE)
    }

    #[must_use]
    pub fn min(self, other: Self) -> Self {
        if self < other { self } else { other }
    }

    #[must_use]
    pub fn max(self, other: Self) -> Self {
        if self > other { self } else { other }
    }

    /// Plancher entier (division arithmétique, correcte aussi en négatif).
    #[must_use]
    pub fn floor_to_int(self) -> i32 {
        (self.0 >> FRAC_BITS) as i32
    }

    /// Plafond entier.
    #[must_use]
    pub fn ceil_to_int(self) -> i32 {
        (self.0.saturating_add(ONE_RAW - 1) >> FRAC_BITS) as i32
    }

    #[must_use]
    pub fn abs(self) -> Self {
        Self(self.0.saturating_abs())
    }

    #[must_use]
    pub fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Interpolation linéaire `a + (b - a) * t` avec `t ∈ [0, 1]`.
    ///
    /// Base de tous les lissages (EMA) du simulateur : on lisse les signaux
    /// d'itération en itération au lieu de les recalculer brutalement, ce qui
    /// évite les oscillations de demande.
    #[must_use]
    pub fn lerp(a: Self, b: Self, t: Self) -> Self {
        a + (b - a).mul(t)
    }

    /// Racine carrée (Newton entier). Utilisée pour les longueurs de segments.
    ///
    /// # Panics
    /// Si la valeur est négative.
    #[must_use]
    pub fn sqrt(self) -> Self {
        assert!(self.0 >= 0, "Fixed::sqrt: valeur negative");
        Self(isqrt_u128((self.0 as u128) << FRAC_BITS) as i64)
    }
}

fn isqrt_u128(value: u128) -> u128 {
    if value < 2 {
        return value;
    }
    let mut x = value;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + value / x) / 2;
    }
    x
}

impl Add for Fixed {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl Sub for Fixed {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}

impl Mul for Fixed {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
        Fixed::mul(self, rhs)
    }
}

impl Div for Fixed {
    type Output = Self;
    fn div(self, rhs: Self) -> Self {
        Fixed::div(self, rhs)
    }
}

impl Neg for Fixed {
    type Output = Self;
    fn neg(self) -> Self {
        Self(self.0.saturating_neg())
    }
}

impl AddAssign for Fixed {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl SubAssign for Fixed {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl From<i32> for Fixed {
    fn from(value: i32) -> Self {
        Self::from_int(value)
    }
}

impl From<Fixed> for f32 {
    fn from(value: Fixed) -> Self {
        value.to_f32()
    }
}

impl fmt::Debug for Fixed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fixed({:.4})", self.to_f64())
    }
}

impl fmt::Display for Fixed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3}", self.to_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mul_keeps_fixed_point_semantics() {
        assert_eq!(Fixed::from_int(2).mul(Fixed::from_int(3)), Fixed::from_int(6));
        assert_eq!(Fixed::HALF.mul(Fixed::HALF), Fixed::from_raw(ONE_RAW / 4));
    }

    #[test]
    fn permille_round_trip() {
        assert_eq!(Fixed::from_permille(1000), Fixed::ONE);
        assert_eq!(Fixed::from_permille(0), Fixed::ZERO);
        assert_eq!(Fixed::from_permille(500), Fixed::HALF);
    }

    #[test]
    fn floor_and_ceil_are_correct_for_negatives() {
        let minus_half = Fixed::from_raw(-(ONE_RAW / 2));
        assert_eq!(minus_half.floor_to_int(), -1);
        assert_eq!(minus_half.ceil_to_int(), 0);
        let plus_half = Fixed::HALF;
        assert_eq!(plus_half.floor_to_int(), 0);
        assert_eq!(plus_half.ceil_to_int(), 1);
    }

    #[test]
    fn sqrt_matches_known_values() {
        assert_eq!(Fixed::from_int(4).sqrt(), Fixed::from_int(2));
        assert_eq!(Fixed::ZERO.sqrt(), Fixed::ZERO);
        let r = Fixed::from_int(2).sqrt().to_f64();
        assert!((r - std::f64::consts::SQRT_2).abs() < 1e-4, "sqrt(2) = {r}");
    }

    #[test]
    fn lerp_is_clamped_by_inputs() {
        let a = Fixed::from_permille(200);
        let b = Fixed::from_permille(800);
        assert_eq!(Fixed::lerp(a, b, Fixed::ZERO), a);
        assert_eq!(Fixed::lerp(a, b, Fixed::ONE), b);
    }

    #[test]
    fn clamp01_contains_outliers() {
        assert_eq!(Fixed::from_int(5).clamp01(), Fixed::ONE);
        assert_eq!(Fixed::from_int(-5).clamp01(), Fixed::ZERO);
    }
}
