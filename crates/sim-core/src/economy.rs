//! Trésorerie et budget mensuel.
//!
//! Volontairement minimal dans cette itération : impôts par tête et par emploi,
//! charges d'entretien par segment et par bâtiment. C'est suffisant pour que le
//! joueur ressente la causalité (étalement urbain coûteux, densité rentable),
//! et c'est testable de façon déterministe.
//!
//! La micro-économie (entreprises, chaînes d'approvisionnement, marché foncier)
//! vivra dans `sim-economy` ; `Treasury` reste ici parce que c'est un agrégat du
//! cœur de boucle (comme le census) et qu'il doit être sérialisé avec la ville.

use crate::population::Census;

/// Paramètres budgétaires (en centimes, jamais en flottants).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BudgetParams {
    pub start_funds_cents: i64,
    pub tax_per_resident_cents: i64,
    pub tax_per_job_cents: i64,
    pub upkeep_per_segment_cents: i64,
    pub upkeep_per_building_cents: i64,
}

impl Default for BudgetParams {
    fn default() -> Self {
        Self {
            start_funds_cents: 5_000_000, // 50 000 $
            tax_per_resident_cents: 800,
            tax_per_job_cents: 600,
            upkeep_per_segment_cents: 150,
            upkeep_per_building_cents: 100,
        }
    }
}

/// Résultat d'un règlement mensuel (poussé dans les événements de tick).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BudgetReport {
    pub income_cents: i64,
    pub expenses_cents: i64,
    pub net_cents: i64,
    pub funds_cents: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Treasury {
    pub funds_cents: i64,
    pub last_income_cents: i64,
    pub last_expenses_cents: i64,
    pub month_count: u32,
}

impl Treasury {
    #[must_use]
    pub fn new(params: &BudgetParams) -> Self {
        Self {
            funds_cents: params.start_funds_cents,
            last_income_cents: 0,
            last_expenses_cents: 0,
            month_count: 0,
        }
    }

    #[must_use]
    pub fn is_bankrupt(&self) -> bool {
        self.funds_cents < 0
    }

    /// Règle le mois : encaisse les impôts, paie l'entretien.
    pub fn settle_month(
        &mut self,
        census: &Census,
        road_segments: u32,
        buildings: u32,
        params: &BudgetParams,
    ) -> BudgetReport {
        let income = i64::from(census.population)
            .saturating_mul(params.tax_per_resident_cents)
            .saturating_add(i64::from(census.employed).saturating_mul(params.tax_per_job_cents));
        let expenses = i64::from(road_segments)
            .saturating_mul(params.upkeep_per_segment_cents)
            .saturating_add(i64::from(buildings).saturating_mul(params.upkeep_per_building_cents));
        let net = income.saturating_sub(expenses);

        self.funds_cents = self.funds_cents.saturating_add(net);
        self.last_income_cents = income;
        self.last_expenses_cents = expenses;
        self.month_count = self.month_count.saturating_add(1);

        BudgetReport {
            income_cents: income,
            expenses_cents: expenses,
            net_cents: net,
            funds_cents: self.funds_cents,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taxes_follow_population_and_jobs() {
        let params = BudgetParams::default();
        let mut treasury = Treasury::new(&params);
        let census = Census {
            population: 100,
            employed: 40,
            ..Census::default()
        };
        let report = treasury.settle_month(&census, 10, 20, &params);
        assert_eq!(report.income_cents, 100 * 800 + 40 * 600);
        assert_eq!(report.expenses_cents, 10 * 150 + 20 * 100);
        assert_eq!(report.net_cents, report.income_cents - report.expenses_cents);
        assert_eq!(treasury.funds_cents, params.start_funds_cents + report.net_cents);
        assert_eq!(treasury.month_count, 1);
    }

    #[test]
    fn empty_city_runs_a_deficit() {
        let params = BudgetParams::default();
        let mut treasury = Treasury::new(&params);
        let report = treasury.settle_month(&Census::default(), 40, 0, &params);
        assert!(report.net_cents < 0);
        assert!(treasury.funds_cents < params.start_funds_cents);
    }
}
