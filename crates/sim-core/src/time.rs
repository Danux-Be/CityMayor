//! Modèle de temps : ticks discrets + horloge à pas fixe côté hôte.
//!
//! ```text
//!  hôte (frame variable, ~16.7 ms)
//!      │  dt mural (Duration)
//!      ▼
//!  FixedStepClock  ── crée 0..N ticks ──►  World::tick()   (50 ms, fixe)
//! ```
//!
//! Conséquences de conception :
//! - la simulation **ne voit jamais** `dt` : tout système raisonne en ticks ;
//! - une frame peut produire 0 ou plusieurs ticks (rattrapage) ; le surplus est
//!   abandonné au-delà de `max_catch_up` pour éviter la spirale de mort ;
//! - le calendrier (jour/mois) est lui aussi exprimé en ticks, donc déterministe.

use core::fmt;
use core::time::Duration;

use crate::fixed::Fixed;

/// Numéro de tick absolu, monotone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tick(u64);

impl Tick {
    pub const ZERO: Self = Self(0);

    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }

    #[must_use]
    pub const fn elapsed_since(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

impl fmt::Display for Tick {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t{}", self.0)
    }
}

/// Résultat d'une avance d'horloge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TickBatch {
    /// Ticks à exécuter maintenant.
    pub ticks: u32,
    /// Ticks abandonnés (l'hôte n'a pas tenu la cadence).
    pub dropped: u32,
}

impl TickBatch {
    #[must_use]
    pub const fn is_idle(self) -> bool {
        self.ticks == 0
    }
}

/// Convertisseur temps mural → nombre de ticks à exécuter.
///
/// Choisir une fréquence qui divise 1e9 ns (1, 2, 4, 5, 8, 10, 20, 25, 40, 50)
/// évite toute dérive de troncature. 20 Hz = 50 ms exactement.
#[derive(Clone, Debug)]
pub struct FixedStepClock {
    step_ns: u64,
    accumulator_ns: u64,
    max_catch_up: u32,
}

impl FixedStepClock {
    /// # Panics
    /// Si `hz == 0`.
    #[must_use]
    pub fn from_hz(hz: u32) -> Self {
        assert!(hz > 0, "FixedStepClock: frequence nulle");
        Self {
            step_ns: 1_000_000_000 / u64::from(hz),
            accumulator_ns: 0,
            max_catch_up: 5,
        }
    }

    /// Nombre maximal de ticks rattrapables dans une même frame.
    #[must_use]
    pub const fn with_max_catch_up(mut self, max_catch_up: u32) -> Self {
        self.max_catch_up = if max_catch_up == 0 { 1 } else { max_catch_up };
        self
    }

    #[must_use]
    pub const fn step(&self) -> Duration {
        Duration::from_nanos(self.step_ns)
    }

    /// Temps mural déjà accumulé mais pas encore converti en tick
    /// (utile pour l'interpolation de rendu).
    #[must_use]
    pub const fn pending(&self) -> Duration {
        Duration::from_nanos(self.accumulator_ns)
    }

    /// Fraction du tick courant déjà écoulée, dans `[0, 1)` : c'est le
    /// paramètre d'interpolation `alpha` pour le rendu (positions lissées).
    #[must_use]
    pub fn alpha(&self) -> Fixed {
        Fixed::from_raw(((self.accumulator_ns as i128) << 16) as i64 / self.step_ns as i64)
    }

    pub fn advance(&mut self, wall_dt: Duration) -> TickBatch {
        let ns = u64::try_from(wall_dt.as_nanos()).unwrap_or(u64::MAX);
        self.accumulator_ns = self.accumulator_ns.saturating_add(ns);

        let due = self.accumulator_ns / self.step_ns;
        let due = u32::try_from(due).unwrap_or(u32::MAX);

        let mut ticks = due;
        let dropped = due.saturating_sub(self.max_catch_up);
        if dropped > 0 {
            ticks = self.max_catch_up;
            // On jette aussi le temps correspondant, sinon la dette revient
            // indéfiniment et la simulation ne rattrape jamais.
            self.accumulator_ns = self
                .accumulator_ns
                .saturating_sub(u64::from(dropped) * self.step_ns);
        }
        self.accumulator_ns -= u64::from(ticks) * self.step_ns;

        TickBatch { ticks, dropped }
    }

    pub fn reset(&mut self) {
        self.accumulator_ns = 0;
    }
}

impl Default for FixedStepClock {
    fn default() -> Self {
        Self::from_hz(20)
    }
}

/// Calendrier de jeu, entièrement dérivé du numéro de tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Calendar {
    ticks_per_day: u64,
    days_per_month: u64,
}

impl Calendar {
    /// # Panics
    /// Si une période vaut zéro.
    #[must_use]
    pub fn new(ticks_per_day: u64, days_per_month: u64) -> Self {
        assert!(ticks_per_day > 0, "Calendar: jour de 0 tick");
        assert!(days_per_month > 0, "Calendar: mois de 0 jour");
        Self {
            ticks_per_day,
            days_per_month,
        }
    }

    #[must_use]
    pub const fn ticks_per_day(&self) -> u64 {
        self.ticks_per_day
    }

    #[must_use]
    pub const fn ticks_per_month(&self) -> u64 {
        self.ticks_per_day * self.days_per_month
    }

    #[must_use]
    pub const fn day(&self, tick: Tick) -> u64 {
        tick.raw() / self.ticks_per_day
    }

    #[must_use]
    pub const fn month(&self, tick: Tick) -> u64 {
        self.day(tick) / self.days_per_month
    }

    #[must_use]
    pub const fn hour_of_day(&self, tick: Tick) -> u8 {
        ((tick.raw() % self.ticks_per_day) * 24 / self.ticks_per_day) as u8
    }

    /// Avancement dans la journée, `[0, 1)`. Base de la courbe de trafic
    /// (heures de pointe) et de l'éclairage (jour/nuit) à venir.
    #[must_use]
    pub fn day_progress(&self, tick: Tick) -> Fixed {
        let rem = tick.raw() % self.ticks_per_day;
        Fixed::from_raw(((rem as i128) << 16) as i64 / self.ticks_per_day as i64)
    }

    /// Vrai si `tick` est la première tuile d'un mois : déclencheur du budget.
    #[must_use]
    pub const fn is_month_start(&self, tick: Tick) -> bool {
        tick.raw() % self.ticks_per_month() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_hz_one_second_is_twenty_ticks() {
        // Sans plafond de rattrapage (outil headless / fast-forward) : 1 s = 20 ticks.
        let mut clock = FixedStepClock::from_hz(20).with_max_catch_up(u32::MAX);
        let batch = clock.advance(Duration::from_secs(1));
        assert_eq!(batch.ticks, 20);
        assert_eq!(batch.dropped, 0);
        assert!(clock.pending().is_zero());
    }

    #[test]
    fn sub_step_time_accumulates() {
        let mut clock = FixedStepClock::from_hz(20);
        assert_eq!(clock.advance(Duration::from_millis(30)).ticks, 0);
        assert_eq!(clock.advance(Duration::from_millis(30)).ticks, 1);
        // Reste 10 ms : pas encore un tick.
        assert_eq!(clock.advance(Duration::from_millis(30)).ticks, 0);
        assert_eq!(clock.advance(Duration::from_millis(20)).ticks, 1);
    }

    #[test]
    fn default_clock_absorbs_a_slow_frame_without_a_spiral() {
        let mut clock = FixedStepClock::default();
        let batch = clock.advance(Duration::from_secs(1)); // 20 ticks dus, 5 autorisés
        assert_eq!(batch.ticks, 5);
        assert_eq!(batch.dropped, 15);
    }

    #[test]
    fn catch_up_is_capped_and_drops_debt() {
        let mut clock = FixedStepClock::from_hz(20).with_max_catch_up(3);
        let batch = clock.advance(Duration::from_secs(5)); // 100 ticks dus
        assert_eq!(batch.ticks, 3);
        assert_eq!(batch.dropped, 97);
        // Plus aucune dette : un demi-tick plus tard ne déclenche rien.
        assert_eq!(clock.advance(Duration::from_millis(20)).ticks, 0);
    }

    #[test]
    fn calendar_derives_day_and_month() {
        let cal = Calendar::new(1200, 30);
        assert_eq!(cal.day(Tick::new(0)), 0);
        assert_eq!(cal.day(Tick::new(1199)), 0);
        assert_eq!(cal.day(Tick::new(1200)), 1);
        assert_eq!(cal.month(Tick::new(35_999)), 0);
        assert_eq!(cal.month(Tick::new(36_000)), 1);
        assert!(cal.is_month_start(Tick::new(36_000)));
        assert!(!cal.is_month_start(Tick::new(36_001)));
        assert_eq!(cal.hour_of_day(Tick::new(600)), 12);
        assert!(cal.day_progress(Tick::new(600)) == Fixed::HALF);
    }
}
