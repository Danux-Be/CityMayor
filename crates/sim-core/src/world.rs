//! `World` : l'état complet et sérialisable de la ville, plus le pipeline de tick.
//!
//! # Ordre du pipeline (voir aussi `lib.rs`)
//! ```text
//! apply_commands → refresh_road_access → land_value → move_population
//!               → grow → census → demand → budget (frontière de mois)
//! ```
//! Chaque étape est *bornée* (budget de tuiles, budget de bâtiments, quota de
//! constructions) : le coût d'un tick ne dépend pas de la taille de la carte,
//! seulement des budgets configurés. C'est ce qui permet de tenir le tick rate
//! quand la ville grandit — et de le prouver avec des benchmarks plus tard.
//!
//! # Déterminisme
//! Toutes les boucles itèrent sur des `Vec`/`Grid` dans l'ordre des indices
//! (jamais sur un `HashMap`). Le RNG n'est pas encore consommé par un système ;
//! il est présent pour la génération de terrain et le bruit de croissance à
//! venir, avec la règle : **un flux par système**, pour que l'ajout d'un tirage
//! dans un système ne décale pas les autres.

use std::collections::VecDeque;

use crate::building::{BuildingKind, Buildings};
use crate::command::{Command, CommandError, Rect};
use crate::demand::{self, Demand, DemandParams};
use crate::economy::{BudgetParams, BudgetReport, Treasury};
use crate::fixed::Fixed;
use crate::grid::Grid;
use crate::ids::{BuildingId, NodeId, SegmentId, TileId};
use crate::land_value::{LandValueLayer, LandValueParams};
use crate::metrics::{Overlay, Snapshot};
use crate::population::Census;
use crate::road::{Point, RoadNetwork, RoadSpec};
use crate::rng::Pcg32;
use crate::terrain::Terrain;
use crate::time::{Calendar, Tick};
use crate::worldgen::{WorldGen, WorldGenParams};
use crate::zoning::{ZoneType, Zoning};

/// Toute la configuration du cœur de simulation, en un seul objet.
///
/// Les valeurs par défaut sont celles du vertical slice (`20 Hz`, `1 jour =
/// 1200 ticks`, `1 mois = 30 jours`, tuiles de 8 m). Une carte 512×512 avec un
/// budget de 4096 tuiles/tick rafraîchit entièrement la valeur foncière en
/// ~3 secondes — assez rapide pour paraître instantané, assez lent pour rester
/// gratuit en CPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SimConfig {
    pub tick_hz: u32,
    pub ticks_per_day: u64,
    pub days_per_month: u64,
    /// Taille d'une tuile, en mètres (doit correspondre à `crate::TILE_SIZE_M`).
    pub tile_size_m: i32,
    /// Pente maximale constructible (cm entre deux tuiles voisines).
    pub max_slope_cm: i32,
    pub road_access_radius_tiles: i32,
    pub road_snap_radius_m: Fixed,
    /// Les routes passent par un chantier avant d'ouvrir (durée selon
    /// `RoadClass::construction_ticks`). `false` : ouverture immédiate
    /// (scénarios de test, démo pré-construite).
    pub road_construction: bool,
    /// Les bâtiments passent par un chantier (stades, `ConstructionStage`)
    /// avant d'accueillir habitants et emplois. `false` : livraison immédiate.
    pub building_construction: bool,
    pub land_value_budget_per_tick: usize,
    pub population_budget_per_tick: usize,
    /// Plancher du flux migratoire entrant, en résidents par tick.
    ///
    /// Le flux effectif suit `migration_fill_rate × logements vides` : plus la
    /// ville a de logements vacants, plus elle attire vite, et la demande
    /// baisse dès que la vacance se résorbe. Un plafond fixe produirait une
    /// vacance permanente à grande échelle (demande R effondrée, croissance
    /// arrêtée) : c'est ce qu'on a mesuré sur 256².
    pub max_inflow_per_tick: u32,
    /// Taux de remplissage des logements vides par tick (5 % par défaut).
    /// Combiné à la demande (`flux = taux × vides × demande`), il fixe la
    /// vitesse de peuplement : une ville de 1 M se remplit en quelques centaines
    /// de ticks, et la croissance urbaine suit ce rythme (c'est elle qui limite).
    pub migration_fill_rate: Fixed,
    /// Départs par tick au maximum, quand la ville attire à l'envers.
    pub max_outflow_per_tick: u32,
    /// Taux de départ minimal (fraction de `max_outflow_per_tick`) tant que la
    /// trésorerie est négative, même si le bonheur reste au-dessus de
    /// `vacate_happiness`. Sans lui, la faillite n'avait aucun effet.
    pub bankruptcy_outflow_rate: Fixed,
    /// Seuil de demande au-delà duquel la ville construit.
    pub growth_threshold: Fixed,
    /// Taux de croissance **composé** : part du stock existant (par zone) qui
    /// peut apparaître par tick, à demande saturée.
    ///
    /// Pourquoi composé : à quota fixe, remplir une métropole de 60 000
    /// bâtiments prendrait des heures de tick. Ici la construction suit
    /// `stock × 1 %`, donc la ville croît exponentiellement puis se stabilise
    /// quand les stocks saturent le zonage. C'est aussi ce qui rend les débuts
    /// de partie lents (1 bâtiment/tick) et les villes mûres réactives.
    pub growth_rate_of_stock: Fixed,
    /// Plancher : une tuile zonée prête peut toujours apparaître (amorçage).
    pub growth_min_per_tick_per_zone: u32,
    /// Plafond de sécurité par zone et par tick (borne le coût d'un tick).
    pub growth_max_per_tick_per_zone: u32,
    /// Bonheur en dessous duquel les habitants commencent à partir. Doit rester
    /// **au-dessus** du plancher de bonheur (`0.30`) mais assez bas pour ne pas
    /// concurrencer l'immigration de base : sinon vague de départ auto-entretenue.
    pub vacate_happiness: Fixed,
    pub labor_participation: Fixed,
    pub happiness_smoothing: Fixed,
    pub demand: DemandParams,
    pub land_value: LandValueParams,
    pub budget: BudgetParams,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            tick_hz: 20,
            ticks_per_day: 1200,
            days_per_month: 30,
            tile_size_m: crate::TILE_SIZE_M,
            max_slope_cm: 400,
            road_access_radius_tiles: 2,
            road_snap_radius_m: Fixed::from_int(1),
            road_construction: true,
            building_construction: true,
            land_value_budget_per_tick: 4096,
            population_budget_per_tick: 4096,
            max_inflow_per_tick: 6,
            migration_fill_rate: Fixed::from_permille(50),
            max_outflow_per_tick: 4,
            bankruptcy_outflow_rate: Fixed::from_permille(500),
            growth_threshold: Fixed::from_permille(550),
            growth_rate_of_stock: Fixed::from_permille(10),
            growth_min_per_tick_per_zone: 1,
            growth_max_per_tick_per_zone: 64,
            vacate_happiness: Fixed::from_permille(450),
            labor_participation: Fixed::from_permille(600),
            happiness_smoothing: Fixed::from_permille(20),
            demand: DemandParams::default(),
            land_value: LandValueParams::default(),
            budget: BudgetParams::default(),
        }
    }
}

/// Événements produits par un tick. L'UI/audio les consomme pour le feedback
/// (flash d'un bâtiment, son de construction, refus d'outil).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SimEvent {
    ZoneRectApplied {
        rect: Rect,
        zone: ZoneType,
        changed: u32,
    },
    BuildingsBulldozed {
        rect: Rect,
        removed: u32,
    },
    RoadBuilt {
        segment: SegmentId,
        length_m: Fixed,
    },
    BuildingBuilt {
        id: BuildingId,
        tile: TileId,
        kind: BuildingKind,
        capacity: u32,
    },
    HouseholdEmptied {
        id: BuildingId,
    },
    BudgetSettled {
        report: BudgetReport,
    },
    CommandRejected {
        error: CommandError,
    },
    /// Un rond-point a été posé (ses 8 segments d'anneau).
    RoundaboutBuilt {
        center: Point,
        segments: u32,
    },
    /// Un chantier de bâtiment est livré : logements ou emplois disponibles.
    BuildingCompleted {
        id: BuildingId,
        kind: BuildingKind,
    },
    /// Des chantiers de route se sont achevés : les routes sont ouvertes.
    RoadsOpened {
        count: u32,
    },
}

/// Ce qu'un tick a produit. `tick` est le tick **exécuté** (pas le suivant).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TickReport {
    pub tick: Tick,
    pub events: Vec<SimEvent>,
}

impl TickReport {
    pub fn rejects(&self) -> impl Iterator<Item = &CommandError> {
        self.events.iter().filter_map(|e| match e {
            SimEvent::CommandRejected { error } => Some(error),
            _ => None,
        })
    }

    #[must_use]
    pub fn built(&self) -> u32 {
        self.events
            .iter()
            .filter(|e| matches!(e, SimEvent::BuildingBuilt { .. }))
            .count() as u32
    }
}

/// État complet de la ville.
#[derive(Clone, Debug)]
pub struct World {
    tick: Tick,
    config: SimConfig,
    calendar: Calendar,
    rng: Pcg32,

    terrain: Terrain,
    /// Générateur du relief, si la carte en vient : il prolonge le terrain
    /// au-delà de la zone jouable (décor jusqu'à l'horizon). `None` pour un
    /// terrain fourni à la main (tests, scénarios plats).
    worldgen: Option<WorldGen>,
    zoning: Zoning,
    roads: RoadNetwork,
    road_access: Vec<bool>,
    road_access_dirty: bool,
    /// Prochaine fin de chantier routier (évite un balayage par tick).
    next_road_opening: Option<Tick>,

    buildings: Buildings,
    land_value: LandValueLayer,

    census: Census,
    demand: Demand,
    treasury: Treasury,

    commands: VecDeque<Command>,
    /// Tuiles candidates à la construction, par zone développable, dans l'ordre
    /// des indices : zonées, libres, desservies et constructibles au moment de
    /// la reconstruction. `grow` les consomme via `growth_cursor` et revalide
    /// chaque entrée (une entrée périmée est sautée une seule fois).
    ///
    /// Pourquoi : l'ancien curseur balayait la grille jusqu'à `tile_count`
    /// tuiles par zone et par tick dès qu'une zone était saturée, soit un coût
    /// de tick proportionnel à la carte. La reconstruction reste O(carte), mais
    /// seulement quand le zonage, une démolition ou la desserte changent.
    growth_candidates: [Vec<u32>; 3],
    growth_candidates_dirty: bool,
    growth_cursor: [usize; 3],
    population_cursor: usize,
    events: Vec<SimEvent>,
}

impl World {
    /// Construit une ville vide sur un terrain donné.
    ///
    /// Le census et la demande sont amorcés ici : le `demand` du premier tick
    /// est déjà significatif, sinon la ville perdrait un tick à ne rien faire.
    #[must_use]
    pub fn new(config: SimConfig, seed: u64, terrain: Terrain) -> Self {
        let (width, height) = (terrain.width(), terrain.height());
        // Budgets dérivés de l'échelle : sur une carte 2048² (4,2 M de tuiles),
        // 4 096 tuiles/tick mettraient 51 s à rafraîchir la valeur foncière.
        // On vise un balayage complet en ~40 s à 20 Hz, sans jamais bloquer un
        // tick (plafond dur de 65 536 tuiles).
        let tiles = (width as usize) * (height as usize);
        let config = SimConfig {
            land_value_budget_per_tick: config
                .land_value_budget_per_tick
                .max(tiles / 400)
                .min(65_536),
            population_budget_per_tick: config
                .population_budget_per_tick
                .max(tiles / 400)
                .min(32_768),
            ..config
        };
        let zoning = Zoning::new(width, height);
        let land_value = LandValueLayer::new(width, height, config.land_value);
        let buildings = Buildings::new(terrain.tile_count());
        let census = Census::recompute(
            &buildings,
            config.labor_participation,
            Fixed::HALF,
            config.happiness_smoothing,
        );
        let demand = demand::compute(&census, &config.demand);
        let treasury = Treasury::new(&config.budget);
        let road_access = vec![false; terrain.tile_count()];
        let calendar = Calendar::new(config.ticks_per_day, config.days_per_month);

        Self {
            tick: Tick::ZERO,
            config,
            calendar,
            rng: Pcg32::new(seed, 0),
            terrain,
            worldgen: None,
            zoning,
            roads: RoadNetwork::new(),
            road_access,
            road_access_dirty: false,
            next_road_opening: None,
            buildings,
            land_value,
            census,
            demand,
            treasury,
            commands: VecDeque::new(),
            growth_candidates: [Vec::new(), Vec::new(), Vec::new()],
            growth_candidates_dirty: false,
            growth_cursor: [0; 3],
            population_cursor: 0,
            events: Vec::new(),
        }
    }

    // ----------------------------------------------------------------- accès

    /// Monde dont le relief est généré : la zone jouable `width × height`
    /// tuiles est un échantillonnage de `WorldGen`, qui reste disponible pour
    /// prolonger le paysage au-delà (`World::worldgen`).
    #[must_use]
    pub fn generated(config: SimConfig, seed: u64, params: WorldGenParams, width: u32, height: u32) -> Self {
        let tile = config.tile_size_m;
        let generator = WorldGen::new(params, i64::from(width) * i64::from(tile), i64::from(height) * i64::from(tile));
        let terrain = generator
            .terrain(width.max(1), height.max(1), tile)
            .unwrap_or_else(|_| Terrain::flat(width.max(1), height.max(1)));
        let mut world = Self::new(config, seed, terrain);
        world.worldgen = Some(generator);
        world
    }

    #[must_use]
    pub const fn worldgen(&self) -> Option<&WorldGen> {
        self.worldgen.as_ref()
    }

    /// Active ou coupe les chantiers routiers pour les routes tracées ensuite
    /// (scénarios : réseau initial livré ouvert, puis chantiers en jeu).
    pub fn set_road_construction(&mut self, enabled: bool) {
        self.config.road_construction = enabled;
    }

    #[must_use]
    pub const fn current_tick(&self) -> Tick {
        self.tick
    }

    #[must_use]
    pub const fn config(&self) -> &SimConfig {
        &self.config
    }

    #[must_use]
    pub const fn calendar(&self) -> &Calendar {
        &self.calendar
    }

    #[must_use]
    pub const fn terrain(&self) -> &Terrain {
        &self.terrain
    }

    #[must_use]
    pub const fn zoning(&self) -> &Zoning {
        &self.zoning
    }

    #[must_use]
    pub const fn roads(&self) -> &RoadNetwork {
        &self.roads
    }

    #[must_use]
    pub const fn buildings(&self) -> &Buildings {
        &self.buildings
    }

    #[must_use]
    pub const fn land_value(&self) -> &LandValueLayer {
        &self.land_value
    }

    #[must_use]
    pub const fn census(&self) -> &Census {
        &self.census
    }

    #[must_use]
    pub const fn demand(&self) -> &Demand {
        &self.demand
    }

    #[must_use]
    pub const fn treasury(&self) -> &Treasury {
        &self.treasury
    }

    #[must_use]
    pub fn pending_commands(&self) -> usize {
        self.commands.len()
    }

    /// Flux RNG des systèmes (un seul flux tant qu'il n'y a qu'un consommateur).
    pub fn rng_mut(&mut self) -> &mut Pcg32 {
        &mut self.rng
    }

    // ------------------------------------------------------------ commandes

    /// Met une commande en file. Elle sera appliquée au **prochain** tick.
    pub fn queue(&mut self, command: Command) {
        self.commands.push_back(command);
    }

    // ------------------------------------------------------------ pipeline

    /// Exécute un tick complet et renvoie ses événements.
    pub fn tick(&mut self) -> TickReport {
        let executed = self.tick;
        self.events.clear();

        self.apply_commands();
        self.open_finished_roads();
        for id in self.buildings.complete_due(self.tick) {
            if let Some(building) = self.buildings.get(id) {
                self.events.push(SimEvent::BuildingCompleted { id, kind: building.kind });
            }
        }
        self.refresh_road_access();

        let land_value_budget = self.config.land_value_budget_per_tick;
        self.land_value
            .update(&self.zoning, &self.road_access, land_value_budget);

        self.move_population();
        self.grow();

        self.census = Census::recompute(
            &self.buildings,
            self.config.labor_participation,
            self.census.happiness,
            self.config.happiness_smoothing,
        );
        self.demand = demand::compute(&self.census, &self.config.demand);

        self.tick = executed.next();

        // Frontière de mois : budget. On utilise le tick *après* incrément pour
        // que le premier règlement tombe après un mois complet de jeu.
        if self.calendar.is_month_start(self.tick) {
            let report = self.treasury.settle_month(
                &self.census,
                self.roads.live_segments(),
                self.buildings.live_count(),
                &self.config.budget,
            );
            self.events.push(SimEvent::BudgetSettled { report });
        }

        TickReport {
            tick: executed,
            events: std::mem::take(&mut self.events),
        }
    }

    // ---------------------------------------------------------- sous-systèmes

    fn apply_commands(&mut self) {
        let (width, height) = (self.zoning.width(), self.zoning.height());
        while let Some(command) = self.commands.pop_front() {
            match command {
                Command::ZoneRect { rect, zone } => {
                    let mut changed = 0u32;
                    for tile in rect.tiles(width, height) {
                        // Un bâtiment existant fige sa tuile : changer la zone
                        // sous un bâtiment créerait un état incohérent.
                        if self.buildings.at(tile).is_some() {
                            continue;
                        }
                        if self.zoning.get(tile) == Some(zone) {
                            continue;
                        }
                        if self.zoning.set(tile, zone).is_some() {
                            changed += 1;
                        }
                    }
                    if changed > 0 {
                        self.growth_candidates_dirty = true;
                    }
                    self.events
                        .push(SimEvent::ZoneRectApplied { rect, zone, changed });
                }
                Command::BulldozeRect { rect } => {
                    let mut removed = 0u32;
                    for tile in rect.tiles(width, height) {
                        if let Some(id) = self.buildings.at(tile) {
                            if self.buildings.remove(id).is_some() {
                                removed += 1;
                            }
                        }
                        self.zoning.set(tile, ZoneType::Unzoned);
                    }
                    self.growth_candidates_dirty = true;
                    self.events
                        .push(SimEvent::BuildingsBulldozed { rect, removed });
                }
                Command::BuildRoad { from, to, spec } => {
                    self.build_road(from, to, Point::default(), Point::default(), spec);
                }
                Command::BuildCurve {
                    from,
                    to,
                    from_handle,
                    to_handle,
                    spec,
                } => self.build_road(from, to, from_handle, to_handle, spec),
                Command::BuildRoundabout { center, radius_m, spec } => {
                    self.build_roundabout(center, radius_m, spec);
                }
            }
        }
    }

    fn build_roundabout(&mut self, center: Point, radius_m: Fixed, spec: RoadSpec) {
        let reach = Point::new(radius_m, radius_m);
        let corners = [
            Point::new(center.x - reach.x, center.y - reach.y),
            Point::new(center.x + reach.x, center.y + reach.y),
        ];
        if !corners.iter().all(|p| self.point_in_bounds(*p)) {
            self.events.push(SimEvent::CommandRejected { error: CommandError::OutOfBounds });
            return;
        }
        match self.roads.add_roundabout(center, radius_m, spec) {
            Ok(segments) => {
                for segment in &segments {
                    self.schedule_road_works(*segment);
                }
                self.roads.ensure_topology();
                self.road_access_dirty = true;
                self.events.push(SimEvent::RoundaboutBuilt { center, segments: segments.len() as u32 });
            }
            Err(error) => self.events.push(SimEvent::CommandRejected { error: CommandError::Road(error) }),
        }
    }

    /// Met un segment neuf en chantier (si la configuration le demande).
    fn schedule_road_works(&mut self, segment: SegmentId) {
        if !self.config.road_construction {
            return;
        }
        let Some(seg) = self.roads.segment(segment) else { return };
        let opens_at = Tick::new(self.tick.raw() + seg.spec.construction_ticks(seg.length_m));
        self.roads.set_opens_at(segment, opens_at);
        self.next_road_opening = Some(self.next_road_opening.map_or(opens_at, |t| t.min(opens_at)));
    }

    /// Fin de chantier : la desserte est recalculée quand une route ouvre.
    fn open_finished_roads(&mut self) {
        let Some(at) = self.next_road_opening else { return };
        if self.tick < at {
            return;
        }
        let count = self.roads.segments().filter(|(_, s)| s.opens_at == at).count() as u32;
        self.road_access_dirty = true;
        self.next_road_opening = self.roads.next_opening(self.tick);
        self.events.push(SimEvent::RoadsOpened { count });
    }

    fn build_road(
        &mut self,
        from: Point,
        to: Point,
        from_handle: Point,
        to_handle: Point,
        spec: RoadSpec,
    ) {
        if !self.point_in_bounds(from) || !self.point_in_bounds(to) {
            self.events.push(SimEvent::CommandRejected {
                error: CommandError::OutOfBounds,
            });
            return;
        }
        let snap = self.config.road_snap_radius_m;
        let (a, a_is_new) = self.node_at_or_create(from, snap);
        let (b, b_is_new) = self.node_at_or_create(to, snap);
        // Une poignée n'est appliquée qu'à un nœud **créé** par cette commande :
        // toucher la poignée d'un carrefour existant déformerait les routes
        // voisines, ce qui est un geste d'édition distinct (itération 2, outil
        // de poignées).
        if a_is_new {
            let _ = self.roads.set_node_handle(a, from_handle);
        }
        if b_is_new {
            let _ = self.roads.set_node_handle(b, to_handle);
        }
        match self.roads.connect(a, b, spec) {
            Ok(segment) => {
                self.schedule_road_works(segment);
                self.roads.ensure_topology();
                self.road_access_dirty = true;
                let length_m = self
                    .roads
                    .segment(segment)
                    .map_or(Fixed::ZERO, |s| s.length_m);
                self.events.push(SimEvent::RoadBuilt { segment, length_m });
            }
            Err(error) => self.events.push(SimEvent::CommandRejected {
                error: CommandError::Road(error),
            }),
        }
    }

    /// Réutilise un nœud existant à portée de `snap` (sinon en crée un).
    /// Renvoie aussi `true` si le nœud vient d'être créé.
    fn node_at_or_create(&mut self, point: Point, snap: Fixed) -> (NodeId, bool) {
        let snap_sq = snap.mul(snap);
        let mut best: Option<(NodeId, Fixed)> = None;
        for (id, node) in self.roads.nodes() {
            let d = node.pos.distance_squared(point);
            if d <= snap_sq && best.is_none_or(|(_, best_d)| d < best_d) {
                best = Some((id, d));
            }
        }
        match best {
            Some((id, _)) => (id, false),
            None => (self.roads.add_node(point), true),
        }
    }

    fn point_in_bounds(&self, point: Point) -> bool {
        let max = Fixed::from_int(self.zoning.width() as i32 * self.config.tile_size_m);
        let max_y = Fixed::from_int(self.zoning.height() as i32 * self.config.tile_size_m);
        point.x >= Fixed::ZERO && point.y >= Fixed::ZERO && point.x <= max && point.y <= max_y
    }

    fn refresh_road_access(&mut self) {
        if !self.road_access_dirty {
            return;
        }
        let (width, height) = (self.zoning.width(), self.zoning.height());
        self.road_access = self.roads.rasterize_access(
            width,
            height,
            self.config.road_access_radius_tiles,
            self.config.tile_size_m,
            self.tick,
        );
        self.road_access_dirty = false;
        self.growth_candidates_dirty = true;
    }

    /// Flux migratoires, borné par `population_budget_per_tick`.
    ///
    /// Le flux est **global** (pas un déplacement par bâtiment) : il remplit les
    /// logements vacants tant que la demande est positive et qu'il reste des
    /// places. C'est ce qui évite le blocage "bâtiments vides + demande nulle"
    /// (voir `population.rs`).
    fn move_population(&mut self) {
        // Itération sur le slab compact des logements, pas sur la grille : le
        // coût est proportionnel au nombre de logements visités, indépendant de
        // la taille de la carte. Un logement plein est sauté en O(1).
        let household_count = self.buildings.household_count();
        if household_count == 0 {
            return;
        }
        let unhappiness = (self.config.vacate_happiness - self.census.happiness).clamp01();
        let bankrupt = self.treasury.is_bankrupt();
        let vacating = unhappiness > Fixed::ZERO || bankrupt;

        // Invariant anti "death wave" : le flux sortant maximal
        // (`unhappiness × max_outflow`) reste sous le flux entrant minimal
        // (`demand_floor × max_inflow`) avec les valeurs par défaut.
        // En faillite, un départ minimal s'applique même si le bonheur est bon.
        let rate = if bankrupt {
            unhappiness.max(self.config.bankruptcy_outflow_rate)
        } else if vacating {
            unhappiness
        } else {
            self.demand.residential
        };
        // Flux piloté par la vacance : `fill_rate × logements vides`, borné
        // inférieurement par le plancher configuré (amorçage d'un hameau).
        // Agrégats courants (O(1)) : ils tiennent compte d'une démolition faite
        // par une commande de ce tick, contrairement au census précédent.
        let totals = self.buildings.totals();
        let vacant_people = totals.housing_capacity.saturating_sub(totals.population);
        // Rien à faire : ne pas balayer tous les logements pour rien (c'était le
        // cas de toute ville pleine, à chaque tick).
        if (vacating && totals.population == 0) || (!vacating && vacant_people == 0) {
            return;
        }
        let fill = self
            .config
            .migration_fill_rate
            .mul(Fixed::from_int(vacant_people as i32))
            .ceil_to_int()
            .max(0) as u32;
        let inflow_cap = fill.max(self.config.max_inflow_per_tick);
        let per_tick = if vacating {
            self.config.max_outflow_per_tick
        } else {
            inflow_cap
        };

        let requested = rate.mul(Fixed::from_int(per_tick as i32)).ceil_to_int();
        let mut remaining = requested
            .clamp(0, self.config.population_budget_per_tick as i32)
            .min(household_count as i32) as u32;
        if remaining == 0 {
            return;
        }

        // Inspection bornée par le budget : le curseur persiste, les logements
        // non vus seront visités aux ticks suivants.
        let inspect_limit = household_count.min(self.config.population_budget_per_tick);
        let mut cursor = self.population_cursor % household_count;
        let mut inspected = 0usize;
        while remaining > 0 && inspected < inspect_limit {
            let Some(id) = self.buildings.household_at(cursor) else {
                break;
            };
            cursor = (cursor + 1) % household_count;
            inspected += 1;

            let Some(building) = self.buildings.get(id) else {
                continue;
            };
            let (occupied, capacity) = (building.occupants, building.capacity);
            let new_occupants = if vacating {
                if occupied == 0 {
                    continue;
                }
                occupied.saturating_sub(1)
            } else {
                if occupied >= capacity {
                    continue;
                }
                (occupied + 1).min(capacity)
            };

            let _ = self.buildings.set_occupants(id, new_occupants);
            remaining -= 1;
            if new_occupants == 0 {
                self.events.push(SimEvent::HouseholdEmptied { id });
            }
        }
        self.population_cursor = cursor;
    }

    /// Croissance : construit sur les tuiles zonées, desservies, constructibles
    /// et inoccupées. `quota` proportionnel à la demande, plafonné par zone.
    /// Reconstruit les listes de candidats. O(carte), appelé seulement quand
    /// `growth_candidates_dirty` (zonage, démolition, desserte).
    fn rebuild_growth_candidates(&mut self) {
        for list in &mut self.growth_candidates {
            list.clear();
        }
        for (index, &zone) in self.zoning.grid().as_slice().iter().enumerate() {
            let Some(zone_index) = zone.developable_index() else {
                continue;
            };
            let tile = TileId::new(index as u32);
            if self.buildings.at(tile).is_none()
                && self.road_access.get(index).copied().unwrap_or(false)
                && self.terrain.is_buildable(tile, self.config.max_slope_cm)
            {
                self.growth_candidates[zone_index].push(index as u32);
            }
        }
        self.growth_cursor = [0; 3];
        self.growth_candidates_dirty = false;
    }

    fn grow(&mut self) {
        if self.growth_candidates_dirty {
            self.rebuild_growth_candidates();
        }
        let threshold = self.config.growth_threshold;
        let span = Fixed::ONE - threshold;

        for zone in ZoneType::DEVELOPABLE {
            let Some(zone_index) = zone.developable_index() else {
                continue;
            };
            let demand = self.demand.get(zone);
            if demand <= threshold {
                continue;
            }
            let excess = if span.is_zero() {
                Fixed::ONE
            } else {
                (demand - threshold).div(span).clamp01()
            };
            let Some(kind) = BuildingKind::from_zone(zone) else {
                continue;
            };
            let quota = growth_quota(excess, self.buildings.count(kind), &self.config);
            if quota == 0 {
                continue;
            }

            // Coût : O(quota + entrées périmées), chaque entrée n'étant lue
            // qu'une fois par reconstruction. Liste épuisée = arrêt immédiat.
            let mut spawned = 0u32;
            while spawned < quota {
                let position = self.growth_cursor[zone_index];
                let Some(&raw) = self.growth_candidates[zone_index].get(position) else {
                    break;
                };
                self.growth_cursor[zone_index] = position + 1;
                let tile = TileId::new(raw);
                let cursor = raw as usize;

                if self.zoning.get(tile) != Some(zone) {
                    continue;
                }
                if self.buildings.at(tile).is_some() {
                    continue;
                }
                if !self.road_access.get(cursor).copied().unwrap_or(false) {
                    continue;
                }
                if !self.terrain.is_buildable(tile, self.config.max_slope_cm) {
                    continue;
                }

                let land_value = self.land_value.get(tile);
                let capacity = capacity_for(kind, land_value);
                let spawned_id = if self.config.building_construction {
                    self.buildings.spawn_construction(tile, kind, capacity, self.tick)
                } else {
                    self.buildings.spawn(tile, kind, capacity)
                };
                if let Some(id) = spawned_id {
                    self.events.push(SimEvent::BuildingBuilt {
                        id,
                        tile,
                        kind,
                        capacity,
                    });
                    spawned += 1;
                }
            }
        }
    }

    // -------------------------------------------------------------- vues

    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            tick: self.tick,
            day: self.calendar.day(self.tick),
            month: self.calendar.month(self.tick),
            hour: self.calendar.hour_of_day(self.tick),
            population: self.census.population,
            housing_capacity: self.census.housing_capacity,
            jobs: self.census.jobs,
            labor_force: self.census.labor_force,
            employed: self.census.employed,
            unemployed: self.census.unemployed,
            unemployment: self.census.unemployment_ratio(),
            vacancy: self.census.vacancy_ratio(),
            happiness: self.census.happiness,
            demand: self.demand,
            buildings: self.buildings.live_count(),
            road_segments: self.roads.live_segments(),
            funds_cents: self.treasury.funds_cents,
            average_land_value: self.land_value.average(),
        }
    }

    #[must_use]
    pub fn zoning_view(&self) -> &Grid<ZoneType> {
        self.zoning.grid()
    }

    #[must_use]
    pub fn land_value_view(&self) -> &Grid<Fixed> {
        self.land_value.values()
    }

    #[must_use]
    pub fn road_access_view(&self) -> &[bool] {
        &self.road_access
    }

    /// Valeur normalisée `[0, 1]` d'un overlay pour une tuile, ou `None` si
    /// l'overlay n'a pas (encore) de donnée dans cette itération.
    #[must_use]
    pub fn overlay_cell(&self, overlay: Overlay, tile: TileId) -> Option<Fixed> {
        match overlay {
            Overlay::Zone => self.zoning.get(tile).map(|zone| match zone {
                ZoneType::Unzoned => Fixed::ZERO,
                ZoneType::Residential => Fixed::from_ratio(1, 3),
                ZoneType::Commercial => Fixed::from_ratio(2, 3),
                ZoneType::Industrial => Fixed::ONE,
            }),
            Overlay::LandValue => Some(self.land_value.get(tile)),
            Overlay::RoadAccess => Some(if self.road_access.get(tile.index()).copied().unwrap_or(false) {
                Fixed::ONE
            } else {
                Fixed::ZERO
            }),
            _ => None,
        }
    }
}

/// Quota de constructions par tick pour une zone, à excès de demande donné.
///
/// ```text
/// base  = max(min_per_tick, ceil(stock × growth_rate_of_stock))
/// quota = min(max_per_tick, ceil(excess × base))
/// ```
///
/// Le terme `stock × taux` rend la croissance **composée** : à taux 1 %/tick,
/// une ville de 60 000 bâtiments peut produire 600 logements par tick (plafonné
/// à `growth_max_per_tick_per_zone`), alors qu'une ville naissante en produit 1.
/// C'est le comportement attendu d'une simulation de 1 M d'habitants : lente à
/// démarrer, réactive une fois dense, bornée en coût de tick.
#[must_use]
pub fn growth_quota(excess: Fixed, stock: u32, config: &SimConfig) -> u32 {
    let base = config
        .growth_rate_of_stock
        .mul(Fixed::from_int(stock as i32))
        .ceil_to_int()
        .max(config.growth_min_per_tick_per_zone as i32);
    let quota = excess.mul(Fixed::from_int(base)).ceil_to_int().max(0);
    (quota as u32).min(config.growth_max_per_tick_per_zone)
}

/// Capacité d'un bâtiment neuf, dérivée de la valeur foncière de sa tuile.
///
/// La valeur foncière est donc *déjà* un multiplicateur de densité : les
/// meilleures parcelles produisent plus de logements/emplois, ce qui enclenche
/// la boucle densité ↔ valeur (renforcée à l'itération 4 avec les niveaux 2/3).
fn capacity_for(kind: BuildingKind, land_value: Fixed) -> u32 {
    let lv = land_value.clamp01();
    let capacity = match kind {
        BuildingKind::Household => 6 + lv.mul(Fixed::from_int(42)).ceil_to_int(),
        BuildingKind::Shop => 4 + lv.mul(Fixed::from_int(24)).ceil_to_int(),
        BuildingKind::Factory => 8 + lv.mul(Fixed::from_int(22)).ceil_to_int(),
    };
    capacity.max(1) as u32
}

impl Default for World {
    fn default() -> Self {
        Self::new(SimConfig::default(), 0, Terrain::flat(32, 32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scénarios instantanés : les chantiers ont leurs propres tests.
    fn config() -> SimConfig {
        SimConfig { road_construction: false, building_construction: false, ..SimConfig::default() }
    }

    #[test]
    fn growth_quota_is_compound_and_capped() {
        let c = config();
        // Ville naissante : plancher de 1.
        assert_eq!(growth_quota(Fixed::ONE, 0, &c), 1);
        // Composé sous le plafond : 1 % du stock.
        assert_eq!(growth_quota(Fixed::ONE, 1_000, &c), 10);
        assert_eq!(growth_quota(Fixed::ONE, 3_000, &c), 30);
        // Le plafond mord dès ~6 400 bâtiments par zone (1 % = 64).
        assert_eq!(growth_quota(Fixed::ONE, 10_000, &c), 64);
        assert_eq!(
            growth_quota(Fixed::ONE, 1_000_000, &c),
            c.growth_max_per_tick_per_zone
        );
        // Sans excès de demande, rien ne se construit.
        assert_eq!(growth_quota(Fixed::ZERO, 10_000, &c), 0);
    }

    #[test]
    fn growth_quota_is_monotone() {
        let c = config();
        let mut previous = 0;
        for stock in [0, 100, 1_000, 5_000, 20_000, 100_000] {
            let quota = growth_quota(Fixed::from_permille(800), stock, &c);
            assert!(quota >= previous, "stock {stock}: {quota} < {previous}");
            previous = quota;
        }
    }

    /// Petite ville mixte, desservie, peuplée.
    fn populated_world() -> World {
        let mut world = World::new(config(), 1, Terrain::flat(24, 24));
        let m = |t: i32| Fixed::from_int(t * crate::TILE_SIZE_M + crate::TILE_SIZE_M / 2);
        world.queue(Command::BuildRoad {
            from: Point::new(m(1), m(10)),
            to: Point::new(m(22), m(10)),
            spec: crate::road::RoadSpec::TWO_WAY,
        });
        for (rows, zone) in [
            ((11, 12), ZoneType::Residential),
            ((9, 9), ZoneType::Commercial),
            ((8, 8), ZoneType::Industrial),
        ] {
            world.queue(Command::ZoneRect {
                rect: Rect::new(2, rows.0, 21, rows.1),
                zone,
            });
        }
        for _ in 0..1_500 {
            world.tick();
        }
        assert!(world.census().population > 0);
        world
    }

    #[test]
    fn bankruptcy_drives_residents_out_even_when_happy() {
        let mut world = populated_world();
        assert!(world.census().happiness > world.config().vacate_happiness);
        let before = world.census().population;

        world.treasury.funds_cents = -1;
        for _ in 0..20 {
            world.tick();
        }
        assert!(
            world.census().population < before,
            "faillite sans effet : {before} -> {}",
            world.census().population
        );
    }

    #[test]
    fn saturated_zones_exhaust_their_candidate_lists() {
        let mut world = populated_world();
        // Le résidentiel est saturé (la demande R reste haute) : le curseur est
        // en bout de liste et `grow` s'arrête en O(1) au lieu de balayer la carte.
        let residential = ZoneType::Residential.developable_index().unwrap();
        assert!(world.demand.residential > world.config.growth_threshold);
        assert_eq!(
            world.growth_cursor[residential],
            world.growth_candidates[residential].len()
        );
        assert!(!world.growth_candidates_dirty);
        for _ in 0..50 {
            world.tick();
        }
        assert_eq!(
            world.growth_cursor[residential],
            world.growth_candidates[residential].len()
        );
    }

    #[test]
    fn large_maps_get_scaled_budgets() {
        let small = World::new(config(), 0, Terrain::flat(64, 64));
        assert_eq!(small.config().land_value_budget_per_tick, 4_096);

        // 2048² = 4,2 M de tuiles : budget ≈ tuiles/400 = 10 485 par tick,
        // soit un balayage complet en ~20 s, pas 51 s.
        let big = World::new(config(), 0, Terrain::flat(2048, 2048));
        assert_eq!(big.config().land_value_budget_per_tick, 2048 * 2048 / 400);
        let seconds = (2048 * 2048) as f64 / (big.config().land_value_budget_per_tick as f64 * 20.0);
        assert!(seconds <= 45.0, "balayage complet en {seconds} s");
    }
}
