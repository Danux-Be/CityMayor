//! Vues de données pour la présentation : snapshots et overlays.
//!
//! # Contrat
//! La couche de rendu / UI ne lit **que** `Snapshot` (agrégats scalaires) et les
//! *vues* immuables (`&Grid<T>`, `&[bool]`). Elle ne reçoit jamais `&mut World`
//! et n'appelle jamais un système de simulation. Inversement, `sim-core` ne
//! connaît ni couleur, ni mesh, ni police : un overlay est une valeur
//! normalisée `[0, 1]` que la palette décide d'afficher.
//!
//! # Overlays : ce qui existe, ce qui est prévu
//! Les overlays sont déclarés dès maintenant (l'énumération est le contrat
//! d'API) ; `is_implemented` dit lesquels produisent une donnée dans cette
//! itération. Les autres renvoient `None` — jamais une valeur factice : un
//! overlay qui ment est pire qu'un overlay absent.

use crate::demand::Demand;
use crate::fixed::Fixed;
use crate::time::Tick;

/// Vue agrégée d'un tick. `Copy`, `Eq` : comparable dans les tests de
/// déterminisme et transmissible sans allocation à l'UI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub tick: Tick,
    pub day: u64,
    pub month: u64,
    pub hour: u8,

    pub population: u32,
    pub housing_capacity: u32,
    pub jobs: u32,
    /// Population active (population × taux de participation).
    pub labor_force: u32,
    pub employed: u32,
    pub unemployed: u32,
    pub unemployment: Fixed,
    pub vacancy: Fixed,
    pub happiness: Fixed,

    pub demand: Demand,

    pub buildings: u32,
    pub road_segments: u32,

    pub funds_cents: i64,
    pub average_land_value: Fixed,
}

impl Snapshot {
    /// Occupation des logements, `[0, 1]`.
    #[must_use]
    pub fn occupancy(&self) -> Fixed {
        if self.housing_capacity == 0 {
            return Fixed::ZERO;
        }
        Fixed::from_ratio(self.population as i32, self.housing_capacity as i32).clamp01()
    }
}

/// Cartes d'information affichables par-dessus la ville.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Overlay {
    Zone,
    LandValue,
    RoadAccess,
    Population,
    Traffic,
    Noise,
    Pollution,
    ServiceCoverage,
    Electricity,
    Water,
}

impl Overlay {
    pub const ALL: [Self; 10] = [
        Self::Zone,
        Self::LandValue,
        Self::RoadAccess,
        Self::Population,
        Self::Traffic,
        Self::Noise,
        Self::Pollution,
        Self::ServiceCoverage,
        Self::Electricity,
        Self::Water,
    ];

    /// Overlays dont la donnée existe réellement dans cette itération.
    #[must_use]
    pub const fn is_implemented(self) -> bool {
        matches!(self, Self::Zone | Self::LandValue | Self::RoadAccess)
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Zone => "Zonage",
            Self::LandValue => "Valeur foncière",
            Self::RoadAccess => "Desserte routière",
            Self::Population => "Densité de population",
            Self::Traffic => "Trafic",
            Self::Noise => "Bruit",
            Self::Pollution => "Pollution",
            Self::ServiceCoverage => "Couverture des services",
            Self::Electricity => "Électricité",
            Self::Water => "Eau",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_real_overlays_are_implemented() {
        let implemented: Vec<_> = Overlay::ALL
            .into_iter()
            .filter(|o| o.is_implemented())
            .collect();
        assert_eq!(
            implemented,
            vec![Overlay::Zone, Overlay::LandValue, Overlay::RoadAccess]
        );
    }

    #[test]
    fn occupancy_never_divides_by_zero() {
        let s = Snapshot::default();
        assert_eq!(s.occupancy(), Fixed::ZERO);
    }
}
