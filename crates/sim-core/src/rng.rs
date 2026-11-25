//! Générateur pseudo-aléatoire déterministe (PCG32).
//!
//! Écrit à la main plutôt que dépendance externe : la séquence fait partie du
//! contrat de reproductibilité. Toute consommation du RNG est ordonnée par le
//! pipeline de tick, donc reproductible ; un système qui tire un nombre aléatoire
//! doit documenter *quand* il le tire (voir `world.rs`, section Déterminisme).

use crate::fixed::Fixed;

/// PCG32 (XSH-RR), période 2^64 par flux.
#[derive(Clone, Debug)]
pub struct Pcg32 {
    state: u64,
    inc: u64,
}

const MULTIPLIER: u64 = 6_364_136_223_846_793_005;

impl Pcg32 {
    /// `stream` sépare des séquences indépendantes pour un même `seed`
    /// (ex. : un flux par système, pour ne pas dépendre de l'ordre d'appel).
    #[must_use]
    pub fn new(seed: u64, stream: u64) -> Self {
        let mut rng = Self {
            state: 0,
            inc: (stream << 1) | 1,
        };
        let _ = rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        let _ = rng.next_u32();
        rng
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(MULTIPLIER).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    pub fn next_u64(&mut self) -> u64 {
        (u64::from(self.next_u32()) << 32) | u64::from(self.next_u32())
    }

    /// Uniforme dans `[0, bound)`. `bound == 0` renvoie 0.
    pub fn next_below(&mut self, bound: u32) -> u32 {
        if bound == 0 { 0 } else { self.next_u32() % bound }
    }

    /// Uniforme dans `[0, 1]` en point fixe.
    pub fn next_fixed(&mut self) -> Fixed {
        Fixed::from_raw(i64::from(self.next_u32() >> 16))
    }

    /// Uniforme dans `[lo, hi]` inclus.
    pub fn next_in_range(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        let span = (i64::from(hi) - i64::from(lo) + 1) as u32;
        lo + self.next_below(span) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = Pcg32::new(42, 0);
        let mut b = Pcg32::new(42, 0);
        for _ in 0..64 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
    }

    #[test]
    fn different_streams_diverge() {
        let mut a = Pcg32::new(42, 0);
        let mut b = Pcg32::new(42, 1);
        let differ = (0..16).any(|_| a.next_u32() != b.next_u32());
        assert!(differ);
    }

    #[test]
    fn next_fixed_stays_in_unit_interval() {
        let mut rng = Pcg32::new(7, 3);
        for _ in 0..1000 {
            let v = rng.next_fixed();
            assert!(v >= Fixed::ZERO && v <= Fixed::ONE);
        }
    }
}
