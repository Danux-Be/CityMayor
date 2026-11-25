//! Zonage : une couche de grille, une valeur par tuile.
//!
//! Choix de design MVP : le zonage est **par tuile**, pas par parcelle. C'est
//! volontairement plus simple que *Cities: Skylines* (où le zonage se fait par
//! front de rue avec des parcelles découpées par la géométrie). Conséquence
//! assumée : les bâtiments sont alignés sur la grille. On récupérera la
//! flexibilité (parcelles, front de rue, courbes) en iteration 4 en gardant
//! cette couche comme *masque* et en ajoutant une couche parcelle au-dessus.

use crate::grid::Grid;
use crate::ids::TileId;

/// Type de zone. `Unzoned` est la valeur par défaut (dézonifier = réaffecter).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum ZoneType {
    #[default]
    Unzoned = 0,
    Residential = 1,
    Commercial = 2,
    Industrial = 3,
}

impl ZoneType {
    /// Zones constructibles, dans l'ordre d'évaluation du système de croissance.
    pub const DEVELOPABLE: [Self; 3] = [Self::Residential, Self::Commercial, Self::Industrial];

    /// Index dense `0..3` pour les tableaux de curseur/quota, `None` si non zoné.
    #[must_use]
    pub const fn developable_index(self) -> Option<usize> {
        match self {
            Self::Residential => Some(0),
            Self::Commercial => Some(1),
            Self::Industrial => Some(2),
            Self::Unzoned => None,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unzoned => "non zoné",
            Self::Residential => "résidentiel",
            Self::Commercial => "commercial",
            Self::Industrial => "industriel",
        }
    }
}

/// Couche de zonage (même indexation que le terrain).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Zoning {
    grid: Grid<ZoneType>,
}

impl Zoning {
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            grid: Grid::new_filled(width, height, ZoneType::Unzoned),
        }
    }

    #[must_use]
    pub fn grid(&self) -> &Grid<ZoneType> {
        &self.grid
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.grid.width()
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.grid.height()
    }

    #[must_use]
    pub fn get(&self, tile: TileId) -> Option<ZoneType> {
        self.grid.get(tile).copied()
    }

    /// Affecte une zone. Renvoie l'ancienne valeur si la tuile existe.
    pub fn set(&mut self, tile: TileId, zone: ZoneType) -> Option<ZoneType> {
        let slot = self.grid.get_mut(tile)?;
        let old = *slot;
        *slot = zone;
        Some(old)
    }

    #[must_use]
    pub fn count(&self, zone: ZoneType) -> u32 {
        self.grid.iter().filter(|(_, z)| **z == zone).count() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_unzoned() {
        let z = Zoning::new(3, 3);
        assert_eq!(z.get(TileId::new(0)), Some(ZoneType::Unzoned));
        assert_eq!(z.count(ZoneType::Unzoned), 9);
    }

    #[test]
    fn set_returns_previous_value() {
        let mut z = Zoning::new(3, 3);
        assert_eq!(
            z.set(TileId::new(4), ZoneType::Residential),
            Some(ZoneType::Unzoned)
        );
        assert_eq!(
            z.set(TileId::new(4), ZoneType::Commercial),
            Some(ZoneType::Residential)
        );
        assert_eq!(z.count(ZoneType::Commercial), 1);
    }

    #[test]
    fn developable_index_is_dense() {
        assert_eq!(ZoneType::Residential.developable_index(), Some(0));
        assert_eq!(ZoneType::Commercial.developable_index(), Some(1));
        assert_eq!(ZoneType::Industrial.developable_index(), Some(2));
        assert_eq!(ZoneType::Unzoned.developable_index(), None);
    }
}
