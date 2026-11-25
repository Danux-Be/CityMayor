//! Terrain : heightmap centimétrique sur grille.
//!
//! Unités : centimètres au-dessus du niveau de la mer, en `i32` (pas de `f32` :
//! deux clients doivent générer exactement le même mesh et la même constructibilité).
//! Le relief n'est pas décoratif : la pente conditionne la constructibilité des
//! parcelles (premier frein "géographique" à l'étalement urbain).

use crate::grid::{Grid, GridError};
use crate::ids::TileId;

pub const WATER_LEVEL_CM: i32 = 0;

/// Altitude par défaut d'un terrain plat constructible : 5 m au-dessus de la
/// mer. Un terrain "plat à 0" serait de l'eau — piège classique qui rend toute
/// la carte inconstructible.
pub const DEFAULT_LAND_ALTITUDE_CM: i32 = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerrainError {
    Grid(GridError),
    HeightsMismatch { expected: usize, got: usize },
}

impl core::fmt::Display for TerrainError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            TerrainError::Grid(inner) => write!(f, "terrain invalide : {inner}"),
            TerrainError::HeightsMismatch { expected, got } => {
                write!(f, "heightmap invalide : attendu {expected} cellules, recu {got}")
            }
        }
    }
}

impl std::error::Error for TerrainError {}

/// Heightmap dense. `heights[tile]` = altitude en cm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Terrain {
    heights: Grid<i32>,
}

impl Terrain {
    /// Terrain plat constructible (5 m au-dessus du niveau de la mer).
    #[must_use]
    pub fn flat(width: u32, height: u32) -> Self {
        Self::at_altitude(width, height, DEFAULT_LAND_ALTITUDE_CM)
    }

    /// Terrain plat à une altitude donnée (`WATER_LEVEL_CM` = plan d'eau).
    #[must_use]
    pub fn at_altitude(width: u32, height: u32, altitude_cm: i32) -> Self {
        Self {
            heights: Grid::new_filled(width, height, altitude_cm),
        }
    }

    /// Terrain à partir d'un buffer d'altitudes (cm).
    ///
    /// # Errors
    /// Dimensions nulles ou buffer de taille incohérente.
    pub fn from_heights(width: u32, height: u32, heights: Vec<i32>) -> Result<Self, TerrainError> {
        let expected = (width as usize) * (height as usize);
        if heights.len() != expected {
            return Err(TerrainError::HeightsMismatch {
                expected,
                got: heights.len(),
            });
        }
        let heights = Grid::from_vec(width, height, heights).map_err(TerrainError::Grid)?;
        Ok(Self { heights })
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.heights.width()
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.heights.height()
    }

    #[must_use]
    pub fn tile_count(&self) -> usize {
        self.heights.len()
    }

    #[must_use]
    pub fn heights(&self) -> &Grid<i32> {
        &self.heights
    }

    #[must_use]
    pub fn altitude_cm(&self, tile: TileId) -> Option<i32> {
        self.heights.get(tile).copied()
    }

    pub fn set_altitude_cm(&mut self, tile: TileId, cm: i32) -> bool {
        if let Some(slot) = self.heights.get_mut(tile) {
            *slot = cm;
            true
        } else {
            false
        }
    }

    /// Pente maximale (cm) entre une tuile et ses 4 voisins orthogonaux.
    #[must_use]
    pub fn max_slope_cm(&self, tile: TileId) -> i32 {
        let Some(here) = self.altitude_cm(tile) else {
            return i32::MAX;
        };
        let Some((x, y)) = self.heights.xy(tile) else {
            return i32::MAX;
        };
        let mut worst = 0;
        for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < 0 || ny < 0 {
                continue;
            }
            let Some(other) = self.heights.get_xy(nx as u32, ny as u32).copied() else {
                continue;
            };
            worst = worst.max((other - here).abs());
        }
        worst
    }

    /// Constructible = au-dessus de l'eau et pente acceptable.
    ///
    /// Les zones humides et les pentes fortes seront des couches séparées
    /// (inondation, glissement) : ici, seule la contrainte MVP.
    #[must_use]
    pub fn is_buildable(&self, tile: TileId, max_slope_cm: i32) -> bool {
        match self.altitude_cm(tile) {
            Some(alt) if alt > WATER_LEVEL_CM => self.max_slope_cm(tile) <= max_slope_cm,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_terrain_is_land_with_no_slope() {
        let t = Terrain::flat(4, 4);
        assert_eq!(t.max_slope_cm(TileId::new(5)), 0);
        assert_eq!(t.altitude_cm(TileId::new(5)), Some(DEFAULT_LAND_ALTITUDE_CM));
        assert!(t.is_buildable(TileId::new(5), 0));
    }

    #[test]
    fn water_level_terrain_is_not_buildable() {
        let t = Terrain::at_altitude(4, 4, WATER_LEVEL_CM);
        assert!(!t.is_buildable(TileId::new(5), 1000), "niveau de la mer = eau");
    }

    #[test]
    fn slope_detects_height_difference() {
        let mut heights = vec![100; 16];
        heights[5] = 200;
        let t = Terrain::from_heights(4, 4, heights).unwrap();
        assert_eq!(t.max_slope_cm(TileId::new(5)), 100);
        assert!(t.is_buildable(TileId::new(5), 150));
        assert!(!t.is_buildable(TileId::new(5), 50));
    }

    #[test]
    fn heightmap_size_is_validated() {
        let err = Terrain::from_heights(2, 2, vec![0; 3]).unwrap_err();
        assert_eq!(
            err,
            TerrainError::HeightsMismatch {
                expected: 4,
                got: 3
            }
        );
    }
}
