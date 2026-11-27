//! Commandes : **seule** porte d'entrée des mutations du monde.
//!
//! # Pourquoi une file de commandes et pas des appels directs ?
//! 1. **Déterminisme** : les commandes sont appliquées en FIFO en début de
//!    tick. L'ordre UI → simulation est donc indépendant du framerate.
//! 2. **Replay / multijoueur** : `(seed, [Command])` suffit à reconstruire
//!    n'importe quelle ville. C'est aussi la base d'un système de sauvegarde
//!    incrémental et de tests de non-régression ("scénario + 10 000 ticks").
//! 3. **Outils de construction** : l'UI peut appeler `validate` sur une copie
//!    légère pour dessiner un ghost, sans muter la ville.
//!
//! Une commande refusée **ne modifie rien** et remonte un
//! [`crate::world::SimEvent::CommandRejected`] : l'UI affiche l'erreur, le joueur
//! comprend immédiatement pourquoi c'est rouge.

use crate::fixed::Fixed;
use crate::ids::{SegmentId, TileId};
use crate::road::{Point, RoadError, RoadSpec};
use crate::zoning::ZoneType;

/// Rectangle inclusif de tuiles, normalisé à la construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub min_x: u32,
    pub min_y: u32,
    pub max_x: u32,
    pub max_y: u32,
}

impl Rect {
    #[must_use]
    pub const fn new(ax: u32, ay: u32, bx: u32, by: u32) -> Self {
        Self {
            min_x: if ax < bx { ax } else { bx },
            min_y: if ay < by { ay } else { by },
            max_x: if ax > bx { ax } else { bx },
            max_y: if ay > by { ay } else { by },
        }
    }

    #[must_use]
    pub const fn single(x: u32, y: u32) -> Self {
        Self {
            min_x: x,
            min_y: y,
            max_x: x,
            max_y: y,
        }
    }

    /// Tuiles du rectangle, **découpées** aux bornes de la carte.
    ///
    /// Le découpage (plutôt que le rejet) est un choix d'ergonomie : un
    /// balayage de zonage qui déborde de la carte doit zoner ce qui est valide,
    /// pas échouer entièrement.
    pub fn tiles(self, width: u32, height: u32) -> impl Iterator<Item = TileId> {
        let (x0, x1) = if width == 0 {
            (1, 0)
        } else {
            (self.min_x, self.max_x.min(width - 1))
        };
        let (y0, y1) = if height == 0 {
            (1, 0)
        } else {
            (self.min_y.min(height - 1), self.max_y.min(height - 1))
        };
        (y0..=y1).flat_map(move |y| (x0..=x1).map(move |x| TileId::new(y * width + x)))
    }

    #[must_use]
    pub fn tile_count(self, width: u32, height: u32) -> u32 {
        self.tiles(width, height).count() as u32
    }
}

/// Mutation demandée par le joueur (ou par un scénario de test).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Peint une zone. N'affecte pas les tuiles déjà bâties (dézonifier ou
    /// changer une zone sous un bâtiment impose de le démolir d'abord).
    ZoneRect { rect: Rect, zone: ZoneType },
    /// Démolit les bâtiments du rectangle et remet les tuiles en "non zoné".
    BulldozeRect { rect: Rect },
    /// Construit une route droite entre deux points (mètres, coordonnées
    /// monde). Les extrémités sont *snappées* sur un nœud existant si elles
    /// sont à moins de `road_snap_radius_m`.
    BuildRoad {
        from: Point,
        to: Point,
        spec: RoadSpec,
    },
    /// Construit une route courbe : les poignées sont les tangentes sortantes
    /// des extrémités (équivalent à tirer les poignées dans l'outil de
    /// construction). Des poignées nulles produisent une droite, donc l'UI n'a
    /// qu'un seul chemin de code pour les deux cas.
    BuildCurve {
        from: Point,
        to: Point,
        from_handle: Point,
        to_handle: Point,
        spec: RoadSpec,
    },
    /// Pose un rond-point de rayon `radius_m` (axe de l'anneau) centré sur
    /// `center`. Les rues qui y aboutissent se raccordent à ses 8 nœuds
    /// (aux points cardinaux et diagonaux) par l'accroche habituelle.
    BuildRoundabout {
        center: Point,
        radius_m: Fixed,
        spec: RoadSpec,
    },
    /// Démolit un segment de route. Un arc de rond-point emporte tout
    /// l'anneau (un rond-point ouvert n'a pas de sens).
    BulldozeRoad { segment: SegmentId },
    /// Taux d'imposition (%) des habitants et des entreprises, bornés à
    /// `economy::MAX_TAX_PERCENT`.
    SetTaxes { residential_percent: u8, business_percent: u8 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    /// Point ou rectangle hors des limites de la carte.
    OutOfBounds,
    /// Le graphe routier a refusé la création (doublon, boucle, spec invalide…).
    Road(RoadError),
}

impl CommandError {
    /// Clé du message affiché au joueur (fichiers de langue). `Display` reste
    /// un texte de diagnostic pour les journaux, jamais montré en jeu.
    #[must_use]
    pub const fn message_key(&self) -> &'static str {
        match self {
            CommandError::OutOfBounds => "error-out-of-bounds",
            CommandError::Road(inner) => inner.message_key(),
        }
    }
}

impl core::fmt::Display for CommandError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CommandError::OutOfBounds => write!(f, "hors des limites de la carte"),
            CommandError::Road(inner) => write!(f, "route refusée : {inner}"),
        }
    }
}

impl std::error::Error for CommandError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_normalizes_corners() {
        assert_eq!(Rect::new(8, 9, 2, 3), Rect::new(2, 3, 8, 9));
    }

    #[test]
    fn rect_tiles_are_clipped_to_map() {
        let r = Rect::new(0, 0, 100, 100);
        let tiles: Vec<_> = r.tiles(4, 4).collect();
        assert_eq!(tiles.len(), 16);
        assert_eq!(r.tile_count(4, 4), 16);
    }

    #[test]
    fn rect_fully_outside_is_empty() {
        let r = Rect::new(10, 10, 12, 12);
        assert_eq!(r.tiles(4, 4).count(), 0);
    }

    #[test]
    fn zero_sized_map_yields_no_tiles() {
        assert_eq!(Rect::new(0, 0, 3, 3).tiles(0, 0).count(), 0);
    }
}
