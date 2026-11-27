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
/// Taux d'imposition « neutre » (%) : celui pour lequel les recettes de base
/// (`tax_per_*_cents`) s'appliquent telles quelles et la demande n'est ni
/// freinée ni stimulée.
pub const NEUTRAL_TAX_PERCENT: u8 = 9;
/// Taux maximal autorisé (%).
pub const MAX_TAX_PERCENT: u8 = 25;

/// Détail d'un budget mensuel (réel ou estimé), en centimes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BudgetBreakdown {
    pub residential_tax_cents: i64,
    pub business_tax_cents: i64,
    pub road_upkeep_cents: i64,
    pub building_upkeep_cents: i64,
}

impl BudgetBreakdown {
    #[must_use]
    pub const fn income_cents(&self) -> i64 {
        self.residential_tax_cents.saturating_add(self.business_tax_cents)
    }

    #[must_use]
    pub const fn expenses_cents(&self) -> i64 {
        self.road_upkeep_cents.saturating_add(self.building_upkeep_cents)
    }

    #[must_use]
    pub const fn net_cents(&self) -> i64 {
        self.income_cents().saturating_sub(self.expenses_cents())
    }
}

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
    /// Taux d'imposition (%) : habitants, puis entreprises (emplois).
    pub tax_percent: [u8; 2],
    /// Détail du dernier mois réglé.
    pub last_breakdown: BudgetBreakdown,
}

impl Treasury {
    #[must_use]
    pub fn new(params: &BudgetParams) -> Self {
        Self {
            funds_cents: params.start_funds_cents,
            last_income_cents: 0,
            last_expenses_cents: 0,
            month_count: 0,
            tax_percent: [NEUTRAL_TAX_PERCENT; 2],
            last_breakdown: BudgetBreakdown::default(),
        }
    }

    /// Fixe les taux (bornés à `MAX_TAX_PERCENT`).
    pub fn set_tax_percent(&mut self, residential: u8, business: u8) {
        self.tax_percent = [residential.min(MAX_TAX_PERCENT), business.min(MAX_TAX_PERCENT)];
    }

    /// Budget d'un mois avec la ville telle qu'elle est (estimation affichée
    /// en continu, et base du règlement mensuel).
    #[must_use]
    pub fn estimate(&self, census: &Census, road_segments: u32, buildings: u32, params: &BudgetParams) -> BudgetBreakdown {
        let scaled = |base: i64, count: u32, percent: u8| {
            i64::from(count).saturating_mul(base).saturating_mul(i64::from(percent)) / i64::from(NEUTRAL_TAX_PERCENT)
        };
        BudgetBreakdown {
            residential_tax_cents: scaled(params.tax_per_resident_cents, census.population, self.tax_percent[0]),
            business_tax_cents: scaled(params.tax_per_job_cents, census.employed, self.tax_percent[1]),
            road_upkeep_cents: i64::from(road_segments).saturating_mul(params.upkeep_per_segment_cents),
            building_upkeep_cents: i64::from(buildings).saturating_mul(params.upkeep_per_building_cents),
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
        let breakdown = self.estimate(census, road_segments, buildings, params);
        let (income, expenses, net) = (breakdown.income_cents(), breakdown.expenses_cents(), breakdown.net_cents());
        self.last_breakdown = breakdown;

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
    fn tax_rates_scale_income_linearly() {
        let params = BudgetParams::default();
        let census = Census { population: 100, employed: 40, ..Census::default() };
        let mut treasury = Treasury::new(&params);
        let neutral = treasury.estimate(&census, 0, 0, &params);
        treasury.set_tax_percent(18, 0);
        let doubled = treasury.estimate(&census, 0, 0, &params);
        assert_eq!(doubled.residential_tax_cents, neutral.residential_tax_cents * 2);
        assert_eq!(doubled.business_tax_cents, 0);
        treasury.set_tax_percent(200, 200);
        assert_eq!(treasury.tax_percent, [MAX_TAX_PERCENT; 2]);
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
