//! Demande RCI (résidentiel / commercial / industriel).
//!
//! # Règles (explicites, testables, sans hasard)
//! ```text
//! demande_R = 0.25 + 0.50 * emplois_ouverts_ratio + 0.35 * (1 - 2*vacance) + 0.20 * (bonheur - 0.5)
//! demande_C = (0.30 + 1.50 * (cible_C - emplois_C/résidents)) * min(1, résidents/cible_pop_C)
//! demande_I = (0.30 + 1.20 * (cible_I - emplois_I/résidents) + 0.30 * chômage) * min(1, résidents/cible_pop_I)
//! ```
//! tout borné dans `[0, 1]`.
//!
//! Le facteur `min(1, résidents / seuil)` empêche une ville naissante de
//! construire des commerces sans clients — c'est la cause racine des économies
//! absurdes du genre. La contrepartie : la ville démarre nécessairement par du
//! résidentiel, ce qui est aussi le comportement attendu par le joueur.

use crate::fixed::Fixed;
use crate::population::Census;
use crate::zoning::ZoneType;

/// Coefficients de la demande. Regroupés en struct pour être tunés (et testés)
/// sans toucher au système.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DemandParams {
    /// Plancher de la demande résidentielle (amorçage de la ville).
    pub residential_floor: Fixed,
    pub job_weight: Fixed,
    pub vacancy_weight: Fixed,
    pub happiness_weight: Fixed,
    /// Emplois commerciaux visés par résident.
    pub commercial_target_ratio: Fixed,
    /// Emplois industriels visés par résident.
    pub industrial_target_ratio: Fixed,
    pub commercial_ratio_weight: Fixed,
    pub industrial_ratio_weight: Fixed,
    pub unemployment_weight: Fixed,
    /// Population minimale pour que la demande commerciale s'active.
    pub commercial_min_population: u32,
    /// Population minimale pour que la demande industrielle s'active.
    pub industrial_min_population: u32,
}

impl Default for DemandParams {
    fn default() -> Self {
        Self {
            residential_floor: Fixed::from_permille(250),
            job_weight: Fixed::from_permille(500),
            vacancy_weight: Fixed::from_permille(350),
            happiness_weight: Fixed::from_permille(200),
            commercial_target_ratio: Fixed::from_permille(300),
            industrial_target_ratio: Fixed::from_permille(450),
            commercial_ratio_weight: Fixed::from_permille(1500),
            industrial_ratio_weight: Fixed::from_permille(1200),
            unemployment_weight: Fixed::from_permille(300),
            // Seuils d'amorçage : un hameau de 8 habitants suffit à attirer de
            // l'industrie (main-d'œuvre disponible), 12 pour du commerce (clients).
            // Sans ces seuils bas, la ville naissante n'a jamais d'emplois, donc
            // jamais de demande résidentielle : boucle morte.
            commercial_min_population: 12,
            industrial_min_population: 8,
        }
    }
}

/// Signaux RCI consommés par le système de croissance et affichés en HUD.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Demand {
    pub residential: Fixed,
    pub commercial: Fixed,
    pub industrial: Fixed,
}

impl Demand {
    #[must_use]
    pub const fn get(&self, zone: ZoneType) -> Fixed {
        match zone {
            ZoneType::Residential => self.residential,
            ZoneType::Commercial => self.commercial,
            ZoneType::Industrial => self.industrial,
            ZoneType::Unzoned => Fixed::ZERO,
        }
    }

    /// Moyenne des trois signaux : métrique de santé urbaine (overlay "demande").
    #[must_use]
    pub fn mean(self) -> Fixed {
        (self.residential + self.commercial + self.industrial).div(Fixed::from_int(3))
    }
}

/// Effet des impôts : chaque point au-dessus du taux neutre freine la
/// demande (résidentielle pour l'impôt des habitants, commerciale et
/// industrielle pour celui des entreprises), chaque point en dessous la
/// stimule. ±1,5 % de demande par point.
#[must_use]
pub fn apply_taxes(demand: Demand, tax_percent: [u8; 2]) -> Demand {
    let effect = |percent: u8| {
        let points = i32::from(crate::economy::NEUTRAL_TAX_PERCENT) - i32::from(percent);
        Fixed::from_permille(15).mul(Fixed::from_int(points))
    };
    Demand {
        residential: (demand.residential + effect(tax_percent[0])).clamp01(),
        commercial: (demand.commercial + effect(tax_percent[1])).clamp01(),
        industrial: (demand.industrial + effect(tax_percent[1])).clamp01(),
    }
}

#[must_use]
pub fn compute(census: &Census, p: &DemandParams) -> Demand {
    let vacancy = census.vacancy_ratio();
    let job_open = census.job_open_ratio();
    let happiness_delta = census.happiness - Fixed::HALF;

    let residential = (p.residential_floor
        + job_open.mul(p.job_weight)
        + (Fixed::ONE - vacancy.mul(Fixed::from_int(2)).clamp01()).mul(p.vacancy_weight)
        + happiness_delta.mul(p.happiness_weight))
    .clamp01();

    let commercial_scale = scale_for(census.population, p.commercial_min_population);
    let industrial_scale = scale_for(census.population, p.industrial_min_population);

    let commercial_ratio = ratio(census.commercial_jobs, census.population);
    let industrial_ratio = ratio(census.industrial_jobs, census.population);

    let commercial = (Fixed::from_permille(300)
        + (p.commercial_target_ratio - commercial_ratio).mul(p.commercial_ratio_weight))
    .mul(commercial_scale)
    .clamp01();

    let industrial = (Fixed::from_permille(300)
        + (p.industrial_target_ratio - industrial_ratio).mul(p.industrial_ratio_weight)
        + census.unemployment_ratio().mul(p.unemployment_weight))
    .mul(industrial_scale)
    .clamp01();

    Demand {
        residential,
        commercial,
        industrial,
    }
}

fn scale_for(population: u32, threshold: u32) -> Fixed {
    if threshold == 0 {
        return Fixed::ONE;
    }
    Fixed::from_ratio(population.min(threshold) as i32, threshold as i32).clamp01()
}

fn ratio(numerator: u32, denominator: u32) -> Fixed {
    if denominator == 0 {
        return Fixed::ZERO;
    }
    // Plafonné à 4.0 : au-delà, le ratio écraserait le terme de demande.
    Fixed::from_ratio(numerator.min(denominator.saturating_mul(4)) as i32, denominator as i32)
        .clamp(Fixed::ZERO, Fixed::from_int(4))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn census(population: u32, housing: u32, jobs: u32, happiness: Fixed) -> Census {
        Census {
            population,
            households: population.min(housing),
            housing_capacity: housing,
            commercial_jobs: jobs,
            industrial_jobs: 0,
            jobs,
            labor_force: population / 2,
            employed: (population / 2).min(jobs),
            unemployed: (population / 2).saturating_sub(jobs),
            happiness,
        }
    }

    #[test]
    fn empty_plot_wants_housing() {
        let d = compute(
            &census(0, 0, 0, Fixed::HALF),
            &DemandParams::default(),
        );
        assert_eq!(d.residential, Fixed::from_permille(600));
        assert!(d.commercial < Fixed::from_permille(550), "pas de commerce sans habitants");
        assert!(d.industrial < Fixed::from_permille(550), "pas d'industrie sans habitants");
    }

    #[test]
    fn spare_housing_and_matching_jobs_stop_growth() {
        // 100 habitants dans 160 logements (37.5 % de vacance), 55 emplois pour
        // 50 actifs : la ville n'a plus de raison de construire du résidentiel.
        let d = compute(
            &census(100, 160, 55, Fixed::from_permille(700)),
            &DemandParams::default(),
        );
        let growth_threshold = crate::SimConfig::default().growth_threshold;
        assert!(
            d.residential < growth_threshold,
            "demande R = {} devrait rester sous le seuil de croissance {}",
            d.residential,
            growth_threshold
        );
    }

    #[test]
    fn open_jobs_without_housing_max_out_residential() {
        // Beaucoup d'emplois, zéro vacance : c'est le signal d'or pour construire.
        let d = compute(
            &census(100, 100, 110, Fixed::from_permille(700)),
            &DemandParams::default(),
        );
        assert_eq!(d.residential, Fixed::ONE);
    }

    #[test]
    fn shortage_of_shops_creates_commercial_demand() {
        let d = compute(
            &census(200, 240, 10, Fixed::from_permille(600)),
            &DemandParams::default(),
        );
        assert!(d.commercial > Fixed::from_permille(600));
    }

    #[test]
    fn unemployment_drives_industry() {
        let with_jobs = compute(&census(200, 240, 90, Fixed::from_permille(600)), &DemandParams::default());
        let without_jobs = compute(&census(200, 240, 0, Fixed::from_permille(600)), &DemandParams::default());
        assert!(without_jobs.industrial > with_jobs.industrial);
    }

    #[test]
    fn demand_is_always_a_ratio() {
        for population in [0u32, 1, 19, 20, 500, 100_000] {
            for jobs in [0u32, 1, 5_000] {
                let d = compute(&census(population, population * 2, jobs, Fixed::ONE), &DemandParams::default());
                for v in [d.residential, d.commercial, d.industrial] {
                    assert!(v >= Fixed::ZERO && v <= Fixed::ONE, "demande hors [0,1] : {v}");
                }
            }
        }
    }
}
