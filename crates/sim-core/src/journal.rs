//! Journal des commandes et sauvegardes par rejeu.
//!
//! La simulation est déterministe : une partie est entièrement décrite par
//! son **scénario** de départ (nom connu de l'hôte, ex. `demo`), la liste des
//! commandes reçues avec leur tick, et le tick atteint. Sauvegarder, c'est
//! écrire ce journal ; charger, c'est reconstruire le scénario et rejouer.
//! Le fichier reste petit (quelques kilo-octets), lisible, et ne dépend
//! d'aucune bibliothèque de sérialisation.
//!
//! Limite assumée : le chargement coûte le temps de rejouer la partie (une
//! partie d'une heure se rejoue en quelques dizaines de secondes). Une
//! sauvegarde par instantané viendra avec l'étape « production ».
//!
//! # Format (texte, une entrée par ligne)
//! ```text
//! citymayor-save 1
//! scenario demo
//! name My city
//! saved-at 1795712912
//! skip 412
//! tick 36000
//! cmd 0 road 1234 5678 9012 3456 0 0 0 0 1 1 50 1
//! ```

use crate::command::{Command, Rect};
use crate::fixed::Fixed;
use crate::ids::SegmentId;
use crate::road::{Point, RoadClass, RoadSpec};
use crate::zoning::ZoneType;

/// Version du format de sauvegarde.
pub const SAVE_VERSION: u32 = 1;

/// Partie sauvegardée.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveGame {
    /// Scénario de départ (reconstruit par l'hôte).
    pub scenario: String,
    /// Nom donné à la partie.
    pub name: String,
    /// Horodatage Unix de la sauvegarde (secondes).
    pub saved_at: u64,
    /// Entrées du journal produites par le scénario lui-même : déjà
    /// rejouées en le reconstruisant, à ne pas rejouer une seconde fois.
    pub skip: usize,
    /// Tick atteint.
    pub tick: u64,
    /// Journal complet : `(tick de mise en file, commande)`.
    pub entries: Vec<(u64, Command)>,
}

impl SaveGame {
    /// Texte du fichier.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = format!(
            "citymayor-save {SAVE_VERSION}\nscenario {}\nname {}\nsaved-at {}\nskip {}\ntick {}\n",
            self.scenario,
            self.name.replace('\n', " "),
            self.saved_at,
            self.skip,
            self.tick
        );
        for (tick, command) in &self.entries {
            out.push_str(&format!("cmd {tick} {}\n", encode(command)));
        }
        out
    }

    /// Lit un fichier de sauvegarde.
    ///
    /// # Errors
    /// En-tête absent, version inconnue, ligne illisible.
    pub fn from_text(text: &str) -> Result<Self, String> {
        let mut lines = text.lines();
        let header = lines.next().ok_or("fichier vide")?;
        let version = header.strip_prefix("citymayor-save ").ok_or("pas une sauvegarde CityMayor")?;
        if version.trim().parse::<u32>().map_err(|e| e.to_string())? != SAVE_VERSION {
            return Err(format!("version de sauvegarde inconnue : {version}"));
        }
        let mut save = SaveGame { scenario: String::new(), name: String::new(), saved_at: 0, skip: 0, tick: 0, entries: Vec::new() };
        for line in lines {
            let (key, rest) = line.split_once(' ').unwrap_or((line, ""));
            match key {
                "scenario" => save.scenario = rest.to_owned(),
                "name" => save.name = rest.to_owned(),
                "saved-at" => save.saved_at = rest.parse().map_err(|e| format!("saved-at : {e}"))?,
                "skip" => save.skip = rest.parse().map_err(|e| format!("skip : {e}"))?,
                "tick" => save.tick = rest.parse().map_err(|e| format!("tick : {e}"))?,
                "cmd" => {
                    let (tick, command) = rest.split_once(' ').ok_or("commande sans tick")?;
                    save.entries.push((tick.parse().map_err(|e| format!("tick de commande : {e}"))?, decode(command)?));
                }
                "" => {}
                other => return Err(format!("ligne inconnue : {other}")),
            }
        }
        if save.scenario.is_empty() {
            return Err("scénario absent".into());
        }
        if save.skip > save.entries.len() {
            return Err("journal tronqué".into());
        }
        Ok(save)
    }
}

fn zone_index(zone: ZoneType) -> u8 {
    zone as u8
}

fn zone_from(index: i64) -> Result<ZoneType, String> {
    ZoneType::ALL.iter().copied().find(|z| i64::from(*z as u8) == index).ok_or_else(|| format!("zone inconnue : {index}"))
}

fn class_index(class: RoadClass) -> i64 {
    RoadClass::ALL.iter().position(|c| *c == class).unwrap_or(0) as i64
}

fn class_from(index: i64) -> Result<RoadClass, String> {
    usize::try_from(index).ok().and_then(|i| RoadClass::ALL.get(i).copied()).ok_or_else(|| format!("classe de route inconnue : {index}"))
}

fn spec_fields(spec: RoadSpec) -> [i64; 4] {
    [i64::from(spec.forward_lanes), i64::from(spec.backward_lanes), i64::from(spec.speed_kph), class_index(spec.class)]
}

fn point_fields(p: Point) -> [i64; 2] {
    [p.x.raw(), p.y.raw()]
}

/// Commande → texte (`mot-clé` puis entiers).
#[must_use]
pub fn encode(command: &Command) -> String {
    let join = |name: &str, fields: &[i64]| {
        let mut s = name.to_owned();
        for f in fields {
            s.push(' ');
            s.push_str(&f.to_string());
        }
        s
    };
    let rect = |r: &Rect| [i64::from(r.min_x), i64::from(r.min_y), i64::from(r.max_x), i64::from(r.max_y)];
    match command {
        Command::ZoneRect { rect: r, zone } => join("zone", &[&rect(r)[..], &[i64::from(zone_index(*zone))]].concat()),
        Command::BulldozeRect { rect: r } => join("bulldoze", &rect(r)),
        Command::BuildRoad { from, to, spec } => {
            join("road", &[&point_fields(*from)[..], &point_fields(*to)[..], &spec_fields(*spec)[..]].concat())
        }
        Command::BuildCurve { from, to, from_handle, to_handle, spec } => join(
            "curve",
            &[&point_fields(*from)[..], &point_fields(*to)[..], &point_fields(*from_handle)[..], &point_fields(*to_handle)[..], &spec_fields(*spec)[..]]
                .concat(),
        ),
        Command::BuildRoundabout { center, radius_m, spec } => {
            join("roundabout", &[&point_fields(*center)[..], &[radius_m.raw()], &spec_fields(*spec)[..]].concat())
        }
        Command::BulldozeRoad { segment } => join("bulldoze-road", &[i64::from(segment.raw())]),
        Command::SetTaxes { residential_percent, business_percent } => {
            join("taxes", &[i64::from(*residential_percent), i64::from(*business_percent)])
        }
        Command::SetRoadConstruction { enabled } => join("road-works", &[i64::from(*enabled)]),
    }
}

/// Texte → commande.
///
/// # Errors
/// Mot-clé inconnu, nombre de champs incorrect, valeur hors bornes.
pub fn decode(text: &str) -> Result<Command, String> {
    let mut parts = text.split_whitespace();
    let name = parts.next().ok_or("commande vide")?;
    let f: Vec<i64> = parts.map(|p| p.parse::<i64>().map_err(|e| format!("{p} : {e}"))).collect::<Result<_, _>>()?;
    let need = |n: usize| if f.len() == n { Ok(()) } else { Err(format!("{name} : {n} champs attendus, {} reçus", f.len())) };
    let u32_of = |v: i64| u32::try_from(v).map_err(|_| format!("valeur hors bornes : {v}"));
    let u8_of = |v: i64| u8::try_from(v).map_err(|_| format!("valeur hors bornes : {v}"));
    let rect = |s: &[i64]| -> Result<Rect, String> { Ok(Rect::new(u32_of(s[0])?, u32_of(s[1])?, u32_of(s[2])?, u32_of(s[3])?)) };
    let point = |s: &[i64]| Point::new(Fixed::from_raw(s[0]), Fixed::from_raw(s[1]));
    let spec = |s: &[i64]| -> Result<RoadSpec, String> {
        Ok(RoadSpec {
            forward_lanes: u8_of(s[0])?,
            backward_lanes: u8_of(s[1])?,
            speed_kph: u16::try_from(s[2]).map_err(|_| format!("vitesse hors bornes : {}", s[2]))?,
            class: class_from(s[3])?,
        })
    };
    Ok(match name {
        "zone" => {
            need(5)?;
            Command::ZoneRect { rect: rect(&f)?, zone: zone_from(f[4])? }
        }
        "bulldoze" => {
            need(4)?;
            Command::BulldozeRect { rect: rect(&f)? }
        }
        "road" => {
            need(8)?;
            Command::BuildRoad { from: point(&f[0..2]), to: point(&f[2..4]), spec: spec(&f[4..8])? }
        }
        "curve" => {
            need(12)?;
            Command::BuildCurve {
                from: point(&f[0..2]),
                to: point(&f[2..4]),
                from_handle: point(&f[4..6]),
                to_handle: point(&f[6..8]),
                spec: spec(&f[8..12])?,
            }
        }
        "roundabout" => {
            need(7)?;
            Command::BuildRoundabout { center: point(&f[0..2]), radius_m: Fixed::from_raw(f[2]), spec: spec(&f[3..7])? }
        }
        "bulldoze-road" => {
            need(1)?;
            Command::BulldozeRoad { segment: SegmentId::new(u32_of(f[0])?) }
        }
        "taxes" => {
            need(2)?;
            Command::SetTaxes { residential_percent: u8_of(f[0])?, business_percent: u8_of(f[1])? }
        }
        "road-works" => {
            need(1)?;
            Command::SetRoadConstruction { enabled: f[0] != 0 }
        }
        other => return Err(format!("commande inconnue : {other}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples() -> Vec<Command> {
        let p = |x: i64, y: i64| Point::new(Fixed::from_raw(x), Fixed::from_raw(y));
        vec![
            Command::ZoneRect { rect: Rect::new(1, 2, 30, 40), zone: ZoneType::Commercial },
            Command::BulldozeRect { rect: Rect::new(0, 0, 3, 3) },
            Command::BuildRoad { from: p(123_456, -7), to: p(9, 10), spec: RoadSpec::AVENUE },
            Command::BuildCurve { from: p(1, 2), to: p(3, 4), from_handle: p(-5, 6), to_handle: p(7, -8), spec: RoadSpec::HIGHWAY_RAMP },
            Command::BuildRoundabout { center: p(65_536 * 100, 65_536 * 200), radius_m: Fixed::from_int(20), spec: RoadSpec::ONE_WAY },
            Command::BulldozeRoad { segment: SegmentId::new(42) },
            Command::SetTaxes { residential_percent: 12, business_percent: 7 },
            Command::SetRoadConstruction { enabled: true },
        ]
    }

    #[test]
    fn every_command_round_trips() {
        for command in samples() {
            assert_eq!(decode(&encode(&command)).unwrap(), command, "{}", encode(&command));
        }
        for (_, spec) in RoadSpec::CATALOGUE {
            let c = Command::BuildRoad { from: Point::default(), to: Point::default(), spec };
            assert_eq!(decode(&encode(&c)).unwrap(), c);
        }
    }

    #[test]
    fn saves_round_trip_and_reject_garbage() {
        let save = SaveGame {
            scenario: "demo".into(),
            name: "My city".into(),
            saved_at: 1_795_712_912,
            skip: 2,
            tick: 3600,
            entries: samples().into_iter().enumerate().map(|(i, c)| (i as u64 * 10, c)).collect(),
        };
        assert_eq!(SaveGame::from_text(&save.to_text()).unwrap(), save);
        assert!(SaveGame::from_text("hello").is_err());
        assert!(SaveGame::from_text("citymayor-save 99\nscenario demo\n").is_err());
        assert!(SaveGame::from_text("citymayor-save 1\nscenario demo\ncmd 0 teleport 1\n").is_err());
    }
}
