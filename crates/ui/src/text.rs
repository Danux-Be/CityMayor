//! Textes du jeu : traduction des concepts de la simulation.
//!
//! La simulation ne connaît que des clés (`ZoneType::message_key`…) ; ce
//! module les associe à leurs arguments et les fait formater par la langue
//! choisie. C'est le seul pont entre `sim-core` et `city-i18n`.

use city_i18n::{Arg, Localizer};
use sim_core::{BuildingKind, CommandError, Overlay, SimEvent, ZoneType};

#[must_use]
pub fn zone(loc: &Localizer, zone: ZoneType) -> String {
    loc.text(zone.message_key())
}

#[must_use]
pub fn building_kind(loc: &Localizer, kind: BuildingKind) -> String {
    loc.text(kind.message_key())
}

#[must_use]
pub fn overlay(loc: &Localizer, overlay: Overlay) -> String {
    loc.text(overlay.message_key())
}

#[must_use]
pub fn command_error(loc: &Localizer, error: &CommandError) -> String {
    loc.text(error.message_key())
}

/// Somme en dollars (centimes → unités entières).
#[must_use]
pub fn money(loc: &Localizer, cents: i64) -> String {
    loc.format("unit-money", &[("value", Arg::from(cents / 100))])
}

/// Message d'actualité d'un événement de simulation, s'il en mérite un.
/// `month` : mois courant (le rapport de budget ne le porte pas).
#[must_use]
pub fn event(loc: &Localizer, event: &SimEvent, month: u32) -> Option<String> {
    Some(match event {
        SimEvent::ZoneRectApplied { zone: z, changed, .. } => loc.format(
            "event-zone-applied",
            &[("changed", Arg::from(*changed)), ("zone", Arg::from(zone(loc, *z)))],
        ),
        SimEvent::BuildingsBulldozed { removed, .. } => {
            loc.format("event-bulldozed", &[("removed", Arg::from(*removed))])
        }
        SimEvent::RoadBuilt { length_m, .. } => {
            loc.format("event-road-built", &[("length", Arg::from(length_m.to_f32().round()))])
        }
        SimEvent::BuildingBuilt { kind, capacity, .. } => loc.format(
            "event-building-built",
            &[("kind", Arg::from(building_kind(loc, *kind).to_lowercase())), ("capacity", Arg::from(*capacity))],
        ),
        SimEvent::HouseholdEmptied { .. } => loc.text("event-household-emptied"),
        SimEvent::BudgetSettled { report } => loc.format(
            "event-budget-settled",
            &[
                ("month", Arg::from(month)),
                ("income", Arg::from(report.income_cents / 100)),
                ("expenses", Arg::from(report.expenses_cents / 100)),
                ("funds", Arg::from(report.funds_cents / 100)),
            ],
        ),
        SimEvent::CommandRejected { error } => {
            loc.format("error-command-rejected", &[("reason", Arg::from(command_error(loc, error)))])
        }
        #[allow(unreachable_patterns)]
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use sim_core::RoadError;

    /// Chaque clé exposée par la simulation existe en anglais.
    #[test]
    fn every_simulation_key_is_translated() {
        let en = Localizer::english();
        let mut keys: Vec<&str> = Vec::new();
        keys.extend(ZoneType::ALL.iter().map(|z| z.message_key()));
        keys.extend(BuildingKind::ALL.iter().map(|k| k.message_key()));
        keys.extend(Overlay::ALL.iter().map(|o| o.message_key()));
        keys.extend(RoadError::ALL.iter().map(RoadError::message_key));
        keys.push(CommandError::OutOfBounds.message_key());
        let missing: Vec<_> = keys.into_iter().filter(|k| !en.has(k)).collect();
        assert!(missing.is_empty(), "clés absentes de locales/en : {missing:?}");
    }

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Chaque clé écrite en dur dans le code (`.text("…")`, `.format("…"`)
    /// existe en anglais : une faute de frappe fait échouer le test au lieu
    /// d'afficher `[clé]` en jeu.
    #[test]
    fn every_literal_key_in_the_code_exists() {
        let en = Localizer::english();
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files = Vec::new();
        rust_files(&crates, &mut files);
        let mut checked = 0;
        for file in files {
            let source = std::fs::read_to_string(&file).unwrap();
            for marker in [".text(\"", ".format(\"", "text_key(\""] {
                for (at, _) in source.match_indices(marker) {
                    let rest = &source[at + marker.len()..];
                    let key: String = rest.chars().take_while(|c| *c != '"').collect();
                    if key.is_empty() || !key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
                        continue;
                    }
                    // Clés d'exemple des tests d'i18n.
                    if key == "no-such-key" {
                        continue;
                    }
                    assert!(en.has(&key), "{} : clé « {key} » absente de locales/en", file.display());
                    checked += 1;
                }
            }
        }
        assert!(checked > 10, "le balayage n'a rien trouvé ({checked})");
    }

    #[test]
    fn events_read_naturally() {
        let en = Localizer::english();
        let built = SimEvent::BuildingBuilt {
            id: sim_core::BuildingId::new(0),
            tile: sim_core::TileId::new(0),
            kind: BuildingKind::Shop,
            capacity: 12,
        };
        assert_eq!(event(&en, &built, 1).unwrap(), "New shop (12 capacity)");
        let rejected = SimEvent::CommandRejected { error: CommandError::OutOfBounds };
        assert_eq!(
            event(&en, &rejected, 1).unwrap(),
            "Action refused: You can't build outside the city limits."
        );
        assert_eq!(money(&en, 123_456), "$1234");
    }
}
