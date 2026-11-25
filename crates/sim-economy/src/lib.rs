//! `sim-economy` — itération 5 : micro-économie.
//!
//! # Périmètre prévu
//! - **Entreprises** : entités `Company { kind, jobs, revenue, inputs, outputs }`
//!   dans un slab contigu, avec des flux (pas des graphes d'objets).
//! - **Chaînes d'approvisionnement** : industrie → commerce, avec demande
//!   dérivée du census ; les pénuries doivent produire un signal lisible
//!   (pénurie affichée), pas une simulation muette.
//! - **Marché foncier** : loyers dérivés de la valeur foncière déjà calculée
//!   par `sim-core`, avec élasticité pour éviter les spirales de loyers.
//! - **Budget des ménages** : revenus (emploi) − loyer − transport ; c'est ce
//!   qui rendra la "fuite des centres" émergente au lieu d'être scriptée.
//!
//! # Frontière avec `sim-core`
//! `sim-core` garde les agrégats de boucle (census, trésorerie, demande RCI)
//! parce qu'ils conditionnent chaque tick. `sim-economy` ajoute la granularité
//! (entreprises, contrats) et tourne à une fréquence plus basse (ex. 1 tick sur
//! 20), ce qui est déjà supporté : un système à fréquence réduite se branche sur
//! `tick.raw() % n == 0` sans changer le pipeline.

/// Étape de la roadmap à laquelle cette crate devient fonctionnelle.
pub const PLANNED_ITERATION: u8 = 5;
