//! `city-ui` — itération 2 : HUD, overlays et outils de construction.
//!
//! # Règles
//! - L'UI **n'écrit jamais** dans `World` : elle empile des
//!   [`sim_core::Command`] et lit des [`sim_core::Snapshot`]. C'est ce qui
//!   garantit que le rendu et l'UI peuvent tourner à une autre fréquence que la
//!   simulation.
//! - Un outil de construction a trois états : *hover* (ghost), *valid* (vert),
//!   *invalid* (rouge + raison). La raison vient de
//!   [`sim_core::CommandError`] renvoyé par le tick — texte localisé, jamais un
//!   code d'erreur brut affiché au joueur.
//! - Les overlays se contentent de mapper `Overlay::label` + `overlay_cell` sur
//!   une palette. Aucune logique de jeu dans l'UI.
//!
//! # Prochain contenu de cette crate
//! - `BuildTool` : machine à états (idle → drag → confirm) avec snapping et
//!   contraintes de coût ;
//! - `OverlaySelector`, `DemandPanel` (RCI), `BudgetPanel` ;
//! - `CameraInput` (orbite/pan/zoom amortis, focus au clic sur une tuile).

pub mod text;

pub use city_i18n::{Arg, Localizer};

/// Étape de la roadmap à laquelle cette crate devient fonctionnelle.
pub const PLANNED_ITERATION: u8 = 2;
