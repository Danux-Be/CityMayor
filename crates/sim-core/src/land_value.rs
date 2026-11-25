//! Valeur foncière — couche de grille recalculée **sous budget**.
//!
//! # Pourquoi un budget ?
//! La valeur foncière dépend du voisinage (route, zones, services à venir) :
//! un recalcul exhaustif par tick coûte `O(tuiles)`, soit 262 144 tuiles pour
//! une carte 512×512 × 20 Hz — inutile puisque la valeur varie lentement.
//! On maintient donc un **curseur persistant** qui rafraîchit `budget` tuiles
//! par tick ; la couche est entièrement à jour tous les `tuiles/budget` ticks
//! (≈ 3 s pour 512² avec un budget de 4096, à 20 Hz).
//!
//! # Invariants
//! - `values[tile]` vaut exactement `0` si la tuile n'est pas zonée (pas de
//!   valeur fantôme dans les overlays) ;
//! - `zoned_sum`/`zoned_count` sont maintenus **incrémentalement** dans
//!   `update`, donc `average()` est O(1) et exact ;
//! - la moyenne ne compte que les tuiles zonées (une moyenne sur toute la
//!   carte tendrait vers 0 à mesure que la carte grandit).

use crate::fixed::Fixed;
use crate::grid::Grid;
use crate::ids::TileId;
use crate::zoning::{ZoneType, Zoning};

/// Poids de la formule de valeur foncière (itérés séparément, donc tunables).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LandValueParams {
    /// Valeur d'une tuile zonée sans route ni aménité.
    pub base: Fixed,
    /// Bonus de desserte routière (le premier facteur causal : accès = valeur).
    pub road_access_bonus: Fixed,
    /// Bonus de zone (le commerce paie mieux le mètre carré).
    pub residential_bonus: Fixed,
    pub commercial_bonus: Fixed,
    pub industrial_bonus: Fixed,
}

impl Default for LandValueParams {
    fn default() -> Self {
        Self {
            base: Fixed::from_permille(300),
            road_access_bonus: Fixed::from_permille(250),
            residential_bonus: Fixed::from_permille(50),
            commercial_bonus: Fixed::from_permille(150),
            industrial_bonus: Fixed::ZERO,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LandValueLayer {
    values: Grid<Fixed>,
    params: LandValueParams,
    cursor: usize,
    zoned_sum_raw: i64,
    zoned_count: u32,
}

impl LandValueLayer {
    #[must_use]
    pub fn new(width: u32, height: u32, params: LandValueParams) -> Self {
        Self {
            values: Grid::new_filled(width, height, Fixed::ZERO),
            params,
            cursor: 0,
            zoned_sum_raw: 0,
            zoned_count: 0,
        }
    }

    #[must_use]
    pub const fn params(&self) -> LandValueParams {
        self.params
    }

    pub fn set_params(&mut self, params: LandValueParams) {
        self.params = params;
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.values.width()
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.values.height()
    }

    #[must_use]
    pub fn values(&self) -> &Grid<Fixed> {
        &self.values
    }

    #[must_use]
    pub fn get(&self, tile: TileId) -> Fixed {
        self.values.get(tile).copied().unwrap_or(Fixed::ZERO)
    }

    /// Moyenne exacte sur les tuiles zonées (`0` si aucune).
    #[must_use]
    pub fn average(&self) -> Fixed {
        if self.zoned_count == 0 {
            return Fixed::ZERO;
        }
        Fixed::from_raw(self.zoned_sum_raw / i64::from(self.zoned_count))
    }

    #[must_use]
    pub const fn zoned_count(&self) -> u32 {
        self.zoned_count
    }

    /// Prochaine tuile qui sera rafraîchie (curseur persistant).
    #[must_use]
    pub const fn cursor_position(&self) -> usize {
        self.cursor
    }

    /// Rafraîchit jusqu'à `budget` tuiles en repartant du curseur.
    ///
    /// Renvoie le nombre de tuiles dont la valeur a effectivement changé
    /// (métrique utile pour l'overlay et les tests de convergence).
    pub fn update(&mut self, zoning: &Zoning, road_access: &[bool], budget: usize) -> u32 {
        let len = self.values.len();
        if len == 0 {
            return 0;
        }
        let budget = budget.clamp(1, len);
        let mut changed = 0u32;
        for _ in 0..budget {
            let index = self.cursor % len;
            self.cursor = (self.cursor + 1) % len;
            let tile = TileId::new(index as u32);
            let zone = zoning.get(tile).unwrap_or(ZoneType::Unzoned);

            let new_value = if zone == ZoneType::Unzoned {
                Fixed::ZERO
            } else {
                let mut value = self.params.base;
                if road_access.get(index).copied().unwrap_or(false) {
                    value += self.params.road_access_bonus;
                }
                value += match zone {
                    ZoneType::Residential => self.params.residential_bonus,
                    ZoneType::Commercial => self.params.commercial_bonus,
                    ZoneType::Industrial => self.params.industrial_bonus,
                    ZoneType::Unzoned => Fixed::ZERO,
                };
                value.clamp01()
            };

            let old = self.values.get(tile).copied().unwrap_or(Fixed::ZERO);
            if old == new_value {
                continue;
            }
            if old > Fixed::ZERO {
                self.zoned_sum_raw -= old.raw();
                self.zoned_count = self.zoned_count.saturating_sub(1);
            }
            if new_value > Fixed::ZERO {
                self.zoned_sum_raw += new_value.raw();
                self.zoned_count += 1;
            }
            if let Some(slot) = self.values.get_mut(tile) {
                *slot = new_value;
            }
            changed += 1;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zoned_grid() -> Zoning {
        let mut z = Zoning::new(4, 4);
        z.set(TileId::new(5), ZoneType::Residential);
        z.set(TileId::new(6), ZoneType::Commercial);
        z
    }

    #[test]
    fn unzoned_tiles_have_no_value() {
        let mut layer = LandValueLayer::new(4, 4, LandValueParams::default());
        let z = zoned_grid();
        layer.update(&z, &[], 100);
        assert_eq!(layer.get(TileId::new(0)), Fixed::ZERO);
        assert_eq!(layer.zoned_count(), 2);
    }

    #[test]
    fn road_access_raises_value() {
        let layer_params = LandValueParams::default();
        let mut without = LandValueLayer::new(4, 4, layer_params);
        let mut with = LandValueLayer::new(4, 4, layer_params);
        let z = zoned_grid();
        without.update(&z, &[], 100);
        with.update(&z, &[true; 16], 100);
        assert!(
            with.get(TileId::new(5)) > without.get(TileId::new(5)),
            "la desserte routière doit augmenter la valeur"
        );
        assert!(with.average() > without.average());
    }

    #[test]
    fn budget_spreads_work_over_ticks() {
        let mut layer = LandValueLayer::new(8, 8, LandValueParams::default());
        let z = zoned_grid();
        // 64 tuiles, budget 10 : la première passe ne couvre que 10 tuiles.
        assert_eq!(layer.cursor_position(), 0);
        layer.update(&z, &[], 10);
        assert_eq!(layer.cursor_position(), 10);
        for _ in 0..6 {
            layer.update(&z, &[], 10);
        }
        assert_eq!(layer.cursor_position(), 6);
    }

    #[test]
    fn dezone_removes_value_and_updates_average() {
        let mut z = zoned_grid();
        let mut layer = LandValueLayer::new(4, 4, LandValueParams::default());
        layer.update(&z, &[], 100);
        let before = layer.average();
        assert!(before > Fixed::ZERO);
        z.set(TileId::new(5), ZoneType::Unzoned);
        z.set(TileId::new(6), ZoneType::Unzoned);
        layer.update(&z, &[], 100);
        assert_eq!(layer.zoned_count(), 0);
        assert_eq!(layer.average(), Fixed::ZERO);
    }
}
