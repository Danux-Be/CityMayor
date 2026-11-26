//! `city-render` — itération 2 : présentation 3D.
//!
//! # Choix de moteur (à trancher à l'itération 2)
//! Deux options, toutes deux compatibles avec l'architecture actuelle :
//!
//! | | Bevy 0.1x | Stack custom `wgpu` + ECS dédié |
//! |---|---|---|
//! | Vitesse de démarrage | très rapide | lente (il faut tout écrire) |
//! | Contrôle du pipeline | bon, mais imposé par les plugins | total |
//! | Écosystème UI | `bevy_ui` correct, mature pour du HUD | `egui`/custom |
//! | Risque de rupture API | réel entre versions majeures | faible |
//! | Lisibilité du rendu "AAA" | PBR + ombres + SSAO via plugins | à écrire |
//!
//! Recommandation : **Bevy** pour l'itération 2 (terrain + instancing +
//! caméra), avec une frontière stricte maintenue par cette crate : elle ne
//! possède *aucun* état de simulation, seulement des caches de présentation
//! (`Mesh`, `InstanceBuffer`, matériaux) reconstruits depuis les vues.
//!
//! # Contraintes de rendu dès le départ
//! - **Instancing massif** : un `InstanceBuffer` par (modèle, matériau), mis à
//!   jour par différence (`dirty` par tuile/bâtiment), jamais reconstruit à la
//!   frame.
//! - **LOD** : géométrie par niveau de détail + culling par cellule de grille ;
//!   les bâtiments lointains sont des boîtes, pas des modèles.
//! - **Interpolation** : le rendu lit `Snapshot` à `tick` et `tick-1` et
//!   interpole avec `FixedStepClock::alpha` — c'est pour cela que la simulation
//!   ne tourne jamais à la fréquence de la frame.
//! - **Overlays** : un seul shader de surface, un sampler de la couche choisie
//!   (`World::overlay_cell`), donc zéro duplication de géométrie.

pub mod nature;
pub mod scene;
pub mod urban;

#[cfg(feature = "viewer")]
pub mod viewer;
