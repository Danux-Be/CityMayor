//! `sim-transport` — itération 3 : trafic, pathfinding et agents.
//!
//! # Pourquoi cette crate est vide pour l'instant
//! L'itération 1 livre le *graphe* (nœuds/segments/adjacence CSR) et la desserte
//! routière. Le trafic n'a de sens qu'une fois deux choix tranchés, et ils ont
//! un impact fort :
//!
//! 1. **Agents individuels vs flot macroscopique.**
//!    - Agents (`Vec<Vehicle>` contigu, SoA) : lisibilité maximale (on voit les
//!      bouchons), coût `O(véhicules)` par tick, idéal < 100 k véhicules.
//!    - Flot (débits par segment) : `O(segments)`, tient 1 M+ d'usagers, mais
//!      perte du feedback visuel direct.
//!    - Recommandation : hybride — flot pour les trajets pendulaires de fond,
//!      agents pour les véhicules proches de la caméra (LOD de simulation).
//! 2. **Recalcul de chemins.**
//!    - Jamais par frame, ni même par tick : chemins en cache avec invalidation
//!      par version de topologie (le graphe expose déjà `topology_is_stale`),
//!      recalcul étalé sur plusieurs ticks par un curseur borné.
//!
//! # API prévue (à figer avant d'écrire le code)
//! ```text
//! struct PathCache { paths: Vec<Option<Path>>, dirty_since: Tick }
//! fn route(net: &RoadNetwork, from: NodeId, to: NodeId, cache: &mut PathCache) -> Option<&Path>
//! fn advance_traffic(world: &mut World, tick: Tick)
//! struct TrafficView { density: &[Fixed], flow: &[Fixed] }  // overlay "Trafic"
//! ```
//! Le point important : `sim-transport` lit `World` via ses vues publiques et
//! n'ajoute **aucune** dépendance au rendu — les véhicules se projettent en
//! instances de rendu dans `city-render`, jamais l'inverse.

/// Étape de la roadmap à laquelle cette crate devient fonctionnelle.
pub const PLANNED_ITERATION: u8 = 3;
