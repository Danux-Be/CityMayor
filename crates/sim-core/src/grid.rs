//! Grille 2D contiguë — brique de base de toutes les couches spatiales
//! (terrain, zonage, valeur foncière, futurs overlays pollution/bruit/réseaux).
//!
//! Invariant central : `TileId(i)` correspond à la cellule `i = y * width + x`.
//! Toutes les couches partagent donc le même index, ce qui permet des overlays
//! et des systèmes multicouches sans conversion ni hash map.

use crate::ids::TileId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GridError {
    EmptyDimensions { width: u32, height: u32 },
    CellCount { expected: usize, got: usize },
}

impl core::fmt::Display for GridError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GridError::EmptyDimensions { width, height } => {
                write!(f, "grille vide : {width}x{height}")
            }
            GridError::CellCount { expected, got } => {
                write!(f, "nombre de cellules invalide : attendu {expected}, recu {got}")
            }
        }
    }
}

impl std::error::Error for GridError {}

/// Matrice dense, ligne par ligne (row-major).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grid<T> {
    width: u32,
    height: u32,
    cells: Vec<T>,
}

impl<T> Grid<T> {
    /// Construit une grille à partir d'un buffer déjà rempli.
    ///
    /// # Errors
    /// Dimensions nulles ou taille de buffer incohérente.
    pub fn from_vec(width: u32, height: u32, cells: Vec<T>) -> Result<Self, GridError> {
        if width == 0 || height == 0 {
            return Err(GridError::EmptyDimensions { width, height });
        }
        let expected = (width as usize) * (height as usize);
        if cells.len() != expected {
            return Err(GridError::CellCount {
                expected,
                got: cells.len(),
            });
        }
        Ok(Self {
            width,
            height,
            cells,
        })
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    #[must_use]
    pub const fn tile_at(&self, x: u32, y: u32) -> Option<TileId> {
        if x < self.width && y < self.height {
            Some(TileId::new(y * self.width + x))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn xy(&self, tile: TileId) -> Option<(u32, u32)> {
        // Un TileId hors grille est un bug d'appelant : on renvoie None plutôt
        // que de paniquer, pour rester utilisable depuis du code UI.
        if tile.raw() < self.width * self.height {
            Some((tile.raw() % self.width, tile.raw() / self.width))
        } else {
            None
        }
    }

    #[must_use]
    pub fn get(&self, tile: TileId) -> Option<&T> {
        self.cells.get(tile.index())
    }

    #[must_use]
    pub fn get_mut(&mut self, tile: TileId) -> Option<&mut T> {
        self.cells.get_mut(tile.index())
    }

    #[must_use]
    pub fn get_xy(&self, x: u32, y: u32) -> Option<&T> {
        self.tile_at(x, y).and_then(|t| self.get(t))
    }

    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        &self.cells
    }

    #[must_use]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.cells
    }

    pub fn iter(&self) -> impl Iterator<Item = (TileId, &T)> + '_ {
        self.cells
            .iter()
            .enumerate()
            .map(|(i, cell)| (TileId::new(i as u32), cell))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (TileId, &mut T)> + '_ {
        self.cells
            .iter_mut()
            .enumerate()
            .map(|(i, cell)| (TileId::new(i as u32), cell))
    }
}

impl<T: Copy> Grid<T> {
    #[must_use]
    pub fn new_filled(width: u32, height: u32, value: T) -> Self {
        assert!(width > 0 && height > 0, "grille vide : {width}x{height}");
        Self {
            width,
            height,
            cells: vec![value; (width as usize) * (height as usize)],
        }
    }

    pub fn fill(&mut self, value: T) {
        self.cells.fill(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_ids_are_row_major() {
        let g = Grid::new_filled(4, 3, 0u8);
        assert_eq!(g.tile_at(0, 0), Some(TileId::new(0)));
        assert_eq!(g.tile_at(3, 0), Some(TileId::new(3)));
        assert_eq!(g.tile_at(0, 1), Some(TileId::new(4)));
        assert_eq!(g.tile_at(4, 0), None);
        assert_eq!(g.xy(TileId::new(5)), Some((1, 1)));
    }

    #[test]
    fn rejects_bad_buffer() {
        let err = Grid::from_vec(2, 2, vec![0u8; 3]).unwrap_err();
        assert_eq!(
            err,
            GridError::CellCount {
                expected: 4,
                got: 3
            }
        );
    }

    #[test]
    fn out_of_grid_ids_are_none() {
        let mut g = Grid::new_filled(2, 2, 0u8);
        assert!(g.get(TileId::new(4)).is_none());
        assert!(g.get_mut(TileId::new(9)).is_none());
        assert_eq!(g.xy(TileId::new(4)), None);
    }
}
