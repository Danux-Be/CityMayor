//! Démographie et emploi — agrégats dérivés des bâtiments.
//!
//! # Modèle MVP (sans agents individuels)
//! Un logement porte des `occupants` (pas de citoyen-entité). L'emploi est
//! affecté **globalement** : `employés = min(population active, emplois)`.
//!
//! Limite assumée : impossible de savoir *qui* travaille *où*, donc pas encore
//! de trajet domicile-travail. C'est exactement ce que la couche agents
//! (`sim-transport`, itération 3) ajoutera : à ce moment-là, `Census` restera
//! l'agrégat de contrôle et les agents deviendront la source de vérité fine.
//!
//! # Boucles de rétroaction (anti "death wave")
//! - Les logements vides **n'appellent pas** de nouveaux logements : la vacance
//!   fait chuter la demande résidentielle (terme `vacancy` dans `demand`).
//! - Aucune démolition automatique : un logement vide reste vide. Les vagues de
//!   destruction ne peuvent donc venir que du joueur, jamais d'un système.
//! - Le bonheur est un **EMA** (lissage), pas un calcul instantané : pas de
//!   bascule binaire toutes les secondes.

use crate::building::{BuildingKind, Buildings};
use crate::fixed::Fixed;

/// Agrégats de la ville pour un tick donné.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Census {
    /// Résidents effectifs.
    pub population: u32,
    /// Logements occupés.
    pub households: u32,
    /// Capacité résidentielle totale (occupée + vide).
    pub housing_capacity: u32,
    /// Emplois commerciaux (capacité).
    pub commercial_jobs: u32,
    /// Emplois industriels (capacité).
    pub industrial_jobs: u32,
    /// Emplois totaux.
    pub jobs: u32,
    /// Population active (population × taux de participation).
    pub labor_force: u32,
    /// Actifs ayant un emploi.
    pub employed: u32,
    /// Actifs sans emploi.
    pub unemployed: u32,
    /// Bonheur lissé, dans `[0, 1]`.
    pub happiness: Fixed,
}

impl Default for Census {
    fn default() -> Self {
        Self {
            population: 0,
            households: 0,
            housing_capacity: 0,
            commercial_jobs: 0,
            industrial_jobs: 0,
            jobs: 0,
            labor_force: 0,
            employed: 0,
            unemployed: 0,
            happiness: Fixed::HALF,
        }
    }
}

impl Census {
    /// Recalcule les agrégats. `O(bâtiments)`, un seul passage, zéro allocation.
    #[must_use]
    pub fn recompute(
        buildings: &Buildings,
        labor_participation: Fixed,
        previous_happiness: Fixed,
        happiness_smoothing: Fixed,
    ) -> Self {
        let mut population = 0u32;
        let mut households = 0u32;
        let mut housing_capacity = 0u32;
        let mut commercial_jobs = 0u32;
        let mut industrial_jobs = 0u32;

        for (_, b) in buildings.iter() {
            match b.kind {
                BuildingKind::Household => {
                    housing_capacity = housing_capacity.saturating_add(b.capacity);
                    population = population.saturating_add(b.occupants);
                    if b.occupants > 0 {
                        households += 1;
                    }
                }
                BuildingKind::Shop => commercial_jobs = commercial_jobs.saturating_add(b.capacity),
                BuildingKind::Factory => {
                    industrial_jobs = industrial_jobs.saturating_add(b.capacity);
                }
            }
        }

        let jobs = commercial_jobs.saturating_add(industrial_jobs);
        // Point fixe : population × participation, sans détour par f32.
        let labor_force = ((i64::from(population) * labor_participation.raw()) >> 16) as u32;
        let employed = labor_force.min(jobs);

        let mut census = Self {
            population,
            households,
            housing_capacity,
            commercial_jobs,
            industrial_jobs,
            jobs,
            labor_force,
            employed,
            unemployed: labor_force - employed,
            happiness: previous_happiness,
        };

        let target = census.happiness_target();
        census.happiness = Fixed::lerp(previous_happiness, target, happiness_smoothing).clamp01();
        census
    }

    /// Cible de bonheur avant lissage.
    ///
    /// `happiness = 0.30 + 0.40 * (1 - chômage) + 0.30 * logement_disponible`,
    /// borné dans `[0, 1]`.
    ///
    /// Trois propriétés voulues, chacune testée :
    /// 1. le plancher est **0.30**, jamais 0 : une ville sans emploi reste
    ///    vivable, elle n'entre pas en spirale de départ des habitants ;
    /// 2. une ville pleine emploi + logements disponibles approche 1.0 ;
    /// 3. les poids sont explicites, donc réglables sans deviner : les services
    ///    publics, la pollution, le bruit et les temps de trajet viendront
    ///    s'ajouter ici, un terme à la fois, chacun avec son test de causalité.
    #[must_use]
    pub fn happiness_target(&self) -> Fixed {
        let employment_term = Fixed::ONE - self.unemployment_ratio();
        let housing_term = (self.vacancy_ratio().mul(Fixed::from_int(2))).clamp01();
        (Fixed::from_permille(300)
            + employment_term.mul(Fixed::from_permille(400))
            + housing_term.mul(Fixed::from_permille(300)))
        .clamp01()
    }

    /// Part des actifs sans emploi, `[0, 1]`.
    #[must_use]
    pub fn unemployment_ratio(&self) -> Fixed {
        if self.labor_force == 0 {
            return Fixed::ZERO;
        }
        Fixed::from_ratio(self.unemployed as i32, self.labor_force as i32).clamp01()
    }

    /// Part de logements vacants, `[0, 1]`. Sans logement, la vacance est nulle
    /// (et la demande maximale) : c'est ce qui permet d'amorcer la ville.
    #[must_use]
    pub fn vacancy_ratio(&self) -> Fixed {
        if self.housing_capacity == 0 {
            return Fixed::ZERO;
        }
        let vacant = self.housing_capacity.saturating_sub(self.population);
        Fixed::from_ratio(vacant as i32, self.housing_capacity as i32).clamp01()
    }

    /// Emplois non pourvus rapportés à la population active.
    #[must_use]
    pub fn job_open_ratio(&self) -> Fixed {
        if self.labor_force == 0 {
            return Fixed::ZERO;
        }
        let open = self.jobs.saturating_sub(self.employed);
        Fixed::from_ratio(open as i32, self.labor_force as i32).clamp01()
    }
}

#[cfg(test)]
mod tests {
    use crate::ids::TileId;

    use super::*;

    fn city() -> Buildings {
        let mut b = Buildings::new(16);
        b.spawn(TileId::new(0), BuildingKind::Household, 10).unwrap();
        b.spawn(TileId::new(1), BuildingKind::Household, 10).unwrap();
        b.spawn(TileId::new(2), BuildingKind::Shop, 4).unwrap();
        b.spawn(TileId::new(3), BuildingKind::Factory, 6).unwrap();
        b
    }

    #[test]
    fn empty_city_has_no_jobs_and_max_demand_signal() {
        let census = Census::recompute(
            &Buildings::new(4),
            Fixed::from_permille(600),
            Fixed::HALF,
            Fixed::from_permille(20),
        );
        assert_eq!(census.population, 0);
        assert_eq!(census.jobs, 0);
        assert_eq!(census.vacancy_ratio(), Fixed::ZERO);
        assert_eq!(census.unemployment_ratio(), Fixed::ZERO);
    }

    #[test]
    fn employment_and_vacancy_are_derived() {
        let mut b = city();
        let household = b.at(TileId::new(0)).unwrap();
        b.get_mut(household).unwrap().occupants = 7;
        let census = Census::recompute(
            &b,
            Fixed::from_permille(500),
            Fixed::HALF,
            Fixed::from_permille(100),
        );
        assert_eq!(census.housing_capacity, 20);
        assert_eq!(census.population, 7);
        assert_eq!(census.households, 1);
        assert_eq!(census.jobs, 10);
        assert_eq!(census.labor_force, 3); // 7 * 0.5 = 3.5 -> 3
        assert_eq!(census.employed, 3);
        assert_eq!(census.unemployed, 0);
        assert_eq!(census.vacancy_ratio(), Fixed::from_ratio(13, 20));
    }

    #[test]
    fn unemployment_is_counted_when_jobs_are_scarce() {
        let mut b = Buildings::new(4);
        b.spawn(TileId::new(0), BuildingKind::Household, 100).unwrap();
        let household = b.at(TileId::new(0)).unwrap();
        b.get_mut(household).unwrap().occupants = 40;
        let census = Census::recompute(
            &b,
            Fixed::from_permille(500),
            Fixed::HALF,
            Fixed::from_permille(100),
        );
        assert_eq!(census.labor_force, 20);
        assert_eq!(census.unemployed, 20);
        assert_eq!(census.unemployment_ratio(), Fixed::ONE);
        assert!(census.happiness_target() < Fixed::from_permille(800));
    }
}
