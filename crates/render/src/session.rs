//! Parties et réglages, sans Bevy : nouvelle partie, sauvegarde, chargement
//! par rejeu du journal (`sim_core::journal`), liste des sauvegardes, et
//! réglages du joueur (fichier `key = value`).
//!
//! Le chargement rejoue la partie tick par tick : il rapporte sa
//! progression (`progress(0..=1)`) pour l'écran de chargement.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sim_core::{SaveGame, World};

use crate::scene;

/// Ticks simulés à la création d'une nouvelle partie : la ville est déjà
/// peuplée, avec des chantiers en cours.
pub const NEW_GAME_TICKS: u64 = 2_400;

/// Extension des fichiers de sauvegarde.
pub const SAVE_EXTENSION: &str = "cmsave";

/// Scénarios de départ connus.
#[must_use]
pub fn scenario(name: &str) -> Option<World> {
    match name {
        "demo" => Some(scene::demo_city()),
        "block" => Some(scene::small_block()),
        _ => None,
    }
}

/// Partie en cours : le monde, et de quoi la sauvegarder.
#[derive(Debug)]
pub struct Session {
    pub world: World,
    pub scenario: String,
    /// Entrées du journal produites par le scénario (non rejouées au chargement).
    pub skip: usize,
    pub name: String,
}

/// Nouvelle partie sur un scénario. Le scénario `demo` est pré-simulé
/// (`NEW_GAME_TICKS`), avec un chantier d'autoroute ouvert à mi-parcours.
///
/// # Errors
/// Scénario inconnu.
pub fn new_game(scenario_name: &str, name: &str, ticks: u64, progress: &mut dyn FnMut(f32)) -> Result<Session, String> {
    let mut world = scenario(scenario_name).ok_or_else(|| format!("scénario inconnu : {scenario_name}"))?;
    let skip = world.journal().len();
    for tick in 0..ticks {
        if scenario_name == "demo" && tick == ticks / 2 {
            scene::demo_road_works(&mut world);
        }
        world.tick();
        if tick % 32 == 0 {
            progress(tick as f32 / ticks.max(1) as f32);
        }
    }
    progress(1.0);
    Ok(Session { world, scenario: scenario_name.to_owned(), skip, name: name.to_owned() })
}

/// Recharge une partie en rejouant son journal.
///
/// # Errors
/// Scénario inconnu, journal incohérent avec le scénario.
pub fn load(save: &SaveGame, progress: &mut dyn FnMut(f32)) -> Result<Session, String> {
    let mut world = scenario(&save.scenario).ok_or_else(|| format!("scénario inconnu : {}", save.scenario))?;
    if world.journal().len() != save.skip {
        return Err("la sauvegarde ne correspond pas à ce scénario".into());
    }
    let replay = &save.entries[save.skip..];
    let mut next = 0;
    let start = world.current_tick().raw();
    let span = save.tick.saturating_sub(start).max(1);
    while world.current_tick().raw() < save.tick {
        let now = world.current_tick().raw();
        while let Some((tick, command)) = replay.get(next) {
            if *tick > now {
                break;
            }
            world.queue(command.clone());
            next += 1;
        }
        world.tick();
        if now % 32 == 0 {
            progress((now - start) as f32 / span as f32);
        }
    }
    // Commandes mises en file au tick de la sauvegarde (pas encore appliquées).
    for (_, command) in &replay[next..] {
        world.queue(command.clone());
    }
    progress(1.0);
    Ok(Session { world, scenario: save.scenario.clone(), skip: save.skip, name: save.name.clone() })
}

/// Sauvegarde d'un monde à son tick courant.
#[must_use]
pub fn make_save(world: &World, scenario: &str, skip: usize, name: &str) -> SaveGame {
    SaveGame {
        scenario: scenario.to_owned(),
        name: name.to_owned(),
        saved_at: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        skip,
        tick: world.current_tick().raw(),
        entries: world.journal().to_vec(),
    }
}

/// Écrit la sauvegarde d'un monde dans `dir` (un fichier par nom de
/// partie) ; renvoie le chemin du fichier.
///
/// # Errors
/// Dossier ou fichier impossible à écrire.
pub fn save_world(world: &World, scenario: &str, skip: usize, name: &str, dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let save = make_save(world, scenario, skip, name);
    let slug: String = name.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let path = dir.join(format!("{slug}.{SAVE_EXTENSION}"));
    // Écriture atomique : un fichier temporaire, puis renommage.
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, save.to_text())?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

impl Session {
    /// Sauvegarde de la partie à son tick courant.
    #[must_use]
    pub fn to_save(&self) -> SaveGame {
        make_save(&self.world, &self.scenario, self.skip, &self.name)
    }

    /// Écrit la sauvegarde dans `dir` ; renvoie le chemin du fichier.
    ///
    /// # Errors
    /// Dossier ou fichier impossible à écrire.
    pub fn save_to(&self, dir: &Path) -> std::io::Result<PathBuf> {
        save_world(&self.world, &self.scenario, self.skip, &self.name, dir)
    }
}

fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Dossier des sauvegardes (`CITYMAYOR_SAVES` pour le surcharger).
#[must_use]
pub fn saves_dir() -> PathBuf {
    std::env::var_os("CITYMAYOR_SAVES").map_or_else(|| data_home().join("citymayor/saves"), PathBuf::from)
}

/// Sauvegardes lisibles de `dir`, la plus récente d'abord.
#[must_use]
pub fn list_saves(dir: &Path) -> Vec<(PathBuf, SaveGame)> {
    let mut saves: Vec<(PathBuf, SaveGame)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == SAVE_EXTENSION))
        .filter_map(|p| {
            let text = std::fs::read_to_string(&p).ok()?;
            SaveGame::from_text(&text).ok().map(|s| (p, s))
        })
        .collect();
    saves.sort_by(|a, b| b.1.saved_at.cmp(&a.1.saved_at).then_with(|| a.0.cmp(&b.0)));
    saves
}

/// Réglages du joueur.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub fullscreen: bool,
    pub shadows: bool,
    /// Taille de l'interface (%).
    pub ui_scale_percent: u16,
    pub language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self { fullscreen: false, shadows: true, ui_scale_percent: 100, language: city_i18n_default_language() }
    }
}

fn city_i18n_default_language() -> String {
    "en".to_owned()
}

/// Tailles d'interface proposées (%).
pub const UI_SCALES: [u16; 4] = [85, 100, 115, 130];

impl Settings {
    /// Fichier des réglages (`CITYMAYOR_CONFIG` pour le surcharger).
    #[must_use]
    pub fn path() -> PathBuf {
        std::env::var_os("CITYMAYOR_CONFIG").map_or_else(|| config_home().join("citymayor/settings.cfg"), PathBuf::from)
    }

    /// Lit les réglages (valeurs par défaut pour ce qui manque ou est illisible).
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let mut settings = Self::default();
        let Ok(text) = std::fs::read_to_string(path) else { return settings };
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "fullscreen" => settings.fullscreen = value == "true",
                "shadows" => settings.shadows = value != "false",
                "ui_scale" => {
                    if let Ok(v) = value.parse::<u16>() {
                        settings.ui_scale_percent = v.clamp(UI_SCALES[0], UI_SCALES[UI_SCALES.len() - 1]);
                    }
                }
                "language" if !value.is_empty() => settings.language = value.to_owned(),
                _ => {}
            }
        }
        settings
    }

    /// Écrit les réglages.
    ///
    /// # Errors
    /// Dossier ou fichier impossible à écrire.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(
            path,
            format!(
                "# CityMayor settings\nfullscreen = {}\nshadows = {}\nui_scale = {}\nlanguage = {}\n",
                self.fullscreen, self.shadows, self.ui_scale_percent, self.language
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_core::{Command, Rect, ZoneType};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("citymayor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn saved_games_reload_identically() {
        let mut session = new_game("block", "Test town", 300, &mut |_| {}).unwrap();
        // Le joueur joue : zonage, impôts, puis encore du temps.
        session.world.queue(Command::ZoneRect { rect: Rect::new(10, 26, 50, 27), zone: ZoneType::Residential });
        session.world.queue(Command::SetTaxes { residential_percent: 12, business_percent: 8 });
        for _ in 0..500 {
            session.world.tick();
        }
        // Une commande encore en file au moment de la sauvegarde.
        session.world.queue(Command::ZoneRect { rect: Rect::new(10, 14, 50, 14), zone: ZoneType::Industrial });
        let dir = temp_dir("saves");
        let path = session.save_to(&dir).unwrap();
        assert!(path.ends_with("test-town.cmsave"));
        let listed = list_saves(&dir);
        assert_eq!(listed.len(), 1);
        let mut steps = 0;
        let mut loaded = load(&listed[0].1, &mut |_| steps += 1).unwrap();
        assert!(steps > 3, "la progression est rapportée");
        assert_eq!(loaded.world.snapshot(), session.world.snapshot());
        assert_eq!(loaded.world.journal(), session.world.journal());
        // Et la suite de la partie reste identique.
        for _ in 0..200 {
            loaded.world.tick();
            session.world.tick();
        }
        assert_eq!(loaded.world.snapshot(), session.world.snapshot());
        assert_eq!(loaded.world.treasury().tax_percent, [12, 8]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn settings_round_trip_and_survive_garbage() {
        let dir = temp_dir("settings");
        let path = dir.join("settings.cfg");
        assert_eq!(Settings::load(&path), Settings::default());
        let custom = Settings { fullscreen: true, shadows: false, ui_scale_percent: 115, language: "en".into() };
        custom.save(&path).unwrap();
        assert_eq!(Settings::load(&path), custom);
        std::fs::write(&path, "ui_scale = 9999\nnonsense\nshadows = maybe\n").unwrap();
        let read = Settings::load(&path);
        assert_eq!(read.ui_scale_percent, 130);
        assert!(read.shadows);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
