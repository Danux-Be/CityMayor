//! `sim-core` — cœur de simulation **déterministe** et orienté données.
//!
//! # Invariants non négociables
//!
//! 1. **Aucun flottant dans l'état simulé.** Toute grandeur scalaire (valeurs
//!    foncières, demande, bonheur, positions) est un entier à point fixe
//!    [`Fixed`] (Q48.16, 16 bits fractionnaires). Les `f32`/`f64` n'existent
//!    qu'à la frontière (rendu, UI, configuration).
//! 2. **Aucun accès au temps mural.** La simulation n'avance que par
//!    [`world::World::tick`]. Le nombre de ticks exécutés dépend de l'hôte
//!    (cf. [`time::FixedStepClock`]) mais **jamais** l'état produit par un tick.
//! 3. **Aucune dépendance au rendu, à l'audio ou à l'IO.** `sim-core` ne
//!    compile qu'avec `std`, pas de `unsafe`, pas de thread non déterministe.
//! 4. **Itération stable.** Les systèmes parcourent toujours les slabs dans
//!    l'ordre des indices. Deux villes identiques (même seed, mêmes commandes,
//!    même nombre de ticks, même version du code) produisent exactement le
//!    même [`metrics::Snapshot`] — c'est testé.
//! 5. **Les mutations ne viennent que de commandes** ([`command::Command`]),
//!    appliquées en début de tick. C'est ce qui rend le replay, le multijoueur
//!    déterministe et le "ghost preview" d'un outil de construction possible
//!    sans dupliquer le monde en entier.
//!
//! # Pipeline d'un tick (ordre figé, ne pas réordonner sans test de régression)
//!
//! ```text
//! 1. apply_commands      commandes UI -> mutations validées (zonage, routes, démolition)
//! 2. refresh_road_access rasterisation du réseau routier (uniquement si topologie salie)
//! 3. land_value          recalcul borné (budget de tuiles/tick, curseur persistant)
//! 4. move_population     flux migratoire entrant/sortant (budget de bâtiments/tick)
//! 5. grow                apparition de bâtiments sur parcelles zonées desservies (quota)
//! 6. census              agrégats population / emploi / bonheur (EMA)
//! 7. demand              signaux RCI dérivés du census (déterministes, sans bruit)
//! 8. budget              règlement mensuel si frontière de mois
//! ```
//!
//! La demande est volontairement **calculée en fin de tick** et consommée au
//! tick suivant : ce décalage d'un tick est inobservable (50 ms) et évite de
//! recalculer le census deux fois par tick.
//!
//! # Ce que cette itération ne fait pas (assumé)
//! - pas d'agents individuels (citoyens/véhicules) ni de pathfinding ;
//! - pas d'évolution des bâtiments (niveau 2/3) ni démolition
//!   automatique (anti "death wave") ;
//! - pas de services publics, pollution, bruit, réseaux eau/électricité ;
//! - pas de sérialisation (l'état est déjà 100 % POD, un `serde` binaire
//!   viendra en crate dédiée pour éviter d'alourdir le cœur).

pub mod building;
pub mod command;
pub mod demand;
pub mod economy;
pub mod fixed;
pub mod grid;
pub mod ids;
pub mod land_value;
pub mod metrics;
pub mod population;
pub mod road;
pub mod rng;
pub mod spline;
pub mod terrain;
pub mod time;
pub mod world;
pub mod worldgen;
pub mod zoning;

pub use building::{Building, BuildingKind, BuildingTotals, Buildings, ConstructionStage};
pub use command::{Command, CommandError, Rect};
pub use demand::{Demand, DemandParams};
pub use economy::{BudgetBreakdown, BudgetParams, BudgetReport, Treasury, MAX_TAX_PERCENT, NEUTRAL_TAX_PERCENT};
pub use fixed::Fixed;
pub use grid::Grid;
pub use ids::{BuildingId, NodeId, SegmentId, TileId};
pub use land_value::{LandValueLayer, LandValueParams};
pub use metrics::{Overlay, Snapshot};
pub use population::Census;
pub use road::{EdgeRef, Point, RoadClass, RoadError, RoadNetwork, RoadSegment, RoadSpec, Roundabout};
pub use spline::BezierCubic;
pub use rng::Pcg32;
pub use terrain::Terrain;
pub use time::{Calendar, FixedStepClock, Tick, TickBatch};
pub use world::{SimConfig, SimEvent, TickReport, World};
pub use worldgen::{RiverParams, WorldGen, WorldGenParams};
pub use zoning::{ZoneType, Zoning};

/// Taille d'une tuile de terrain, en mètres. La grille est la source de vérité
/// du MVP : elle simplifie le snapping, le zonage et les overlays. Le réseau
/// routier reste, lui, en coordonnées continues (voir [`road::Point`]).
pub const TILE_SIZE_M: i32 = 8;

/// Centre d'une tuile exprimée en coordonnées monde (mètres).
///
/// Utilisé par les outils de construction pour "snapper" une route sur le
/// centre des tuiles, et par les tests d'intégration.
#[must_use]
pub fn tile_center_m(tile_coord: i32) -> Fixed {
    Fixed::from_int(tile_coord * TILE_SIZE_M + TILE_SIZE_M / 2)
}
