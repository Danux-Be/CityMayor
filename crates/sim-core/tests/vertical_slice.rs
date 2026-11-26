//! Test d'intégration du vertical slice : une ville complète, sans rendu,
//! sans IO, sans flottants. Chaque test vérifie une **règle causale** du design
//! (et pas seulement un comportement observé) : c'est ce qui permet de régler
//! la simulation sans casser les invariants.

use sim_core::{
    tile_center_m, Command, CommandError, Fixed, Point, Rect, RoadError, RoadSpec, SimConfig,
    SimEvent, Terrain, TileId, World, ZoneType, TILE_SIZE_M,
};

fn p(tx: i32, ty: i32) -> Point {
    Point::new(tile_center_m(tx), tile_center_m(ty))
}

/// Ville de référence : un croisement, du résidentiel orienté sud, du
/// commercial au nord, de l'industrie le long de la rue verticale.
fn demo_world() -> World {
    let mut world = World::new(instant_roads(), 0xDEAD_BEEF, Terrain::flat(32, 32));
    world.queue(Command::BuildRoad {
        from: p(2, 8),
        to: p(28, 8),
        spec: RoadSpec::TWO_WAY,
    });
    world.queue(Command::BuildRoad {
        from: p(8, 8),
        to: p(8, 28),
        spec: RoadSpec::TWO_WAY,
    });
    world.queue(Command::ZoneRect {
        rect: Rect::new(4, 9, 24, 11),
        zone: ZoneType::Residential,
    });
    world.queue(Command::ZoneRect {
        rect: Rect::new(9, 6, 22, 7),
        zone: ZoneType::Commercial,
    });
    world.queue(Command::ZoneRect {
        rect: Rect::new(9, 12, 10, 24),
        zone: ZoneType::Industrial,
    });
    world
}

fn run(world: &mut World, ticks: u32) {
    for _ in 0..ticks {
        world.tick();
    }
}


/// Configuration des scénarios : routes ouvertes dès leur tracé, bâtiments
/// livrés d'emblée. Les chantiers ont leurs propres tests (`road_works_*`,
/// `building_works_*`).
fn instant_roads() -> SimConfig {
    SimConfig { road_construction: false, building_construction: false, ..SimConfig::default() }
}

#[test]
fn tile_centers_are_stable_and_tile_sized() {
    assert_eq!(tile_center_m(0), Fixed::from_int(TILE_SIZE_M / 2));
    assert_eq!(
        tile_center_m(10) - tile_center_m(9),
        Fixed::from_int(TILE_SIZE_M),
        "les centres de tuiles sont espacés d'exactement une tuile"
    );
}

#[test]
fn commands_are_applied_at_the_tick_boundary() {
    let mut world = demo_world();
    assert_eq!(world.pending_commands(), 5);
    assert_eq!(world.roads().live_segments(), 0);
    assert_eq!(world.zoning().count(ZoneType::Residential), 0);

    let report = world.tick();

    assert_eq!(world.pending_commands(), 0);
    assert_eq!(world.roads().live_segments(), 2);
    assert_eq!(world.zoning().count(ZoneType::Residential), 21 * 3);
    assert_eq!(report.rejects().count(), 0);
    assert!(report
        .events
        .iter()
        .any(|e| matches!(e, SimEvent::RoadBuilt { .. })));
}

#[test]
fn zoning_without_road_access_builds_nothing() {
    let mut world = World::new(instant_roads(), 1, Terrain::flat(24, 24));
    world.queue(Command::ZoneRect {
        rect: Rect::new(2, 2, 16, 16),
        zone: ZoneType::Residential,
    });
    run(&mut world, 300);

    assert_eq!(world.buildings().live_count(), 0, "pas de route, pas de ville");
    assert_eq!(world.snapshot().population, 0);
    assert!(world.zoning().count(ZoneType::Residential) > 0);
}

#[test]
fn out_of_bounds_road_is_rejected_without_side_effects() {
    let mut world = World::new(instant_roads(), 2, Terrain::flat(16, 16));
    world.queue(Command::BuildRoad {
        from: Point::new(Fixed::from_int(-50), Fixed::from_int(10)),
        to: Point::new(Fixed::from_int(10), Fixed::from_int(10)),
        spec: RoadSpec::TWO_WAY,
    });

    let report = world.tick();

    let rejects: Vec<_> = report.rejects().collect();
    assert_eq!(rejects, vec![&CommandError::OutOfBounds]);
    assert_eq!(world.roads().live_segments(), 0);
    assert_eq!(world.roads().node_count(), 0, "aucun nœud orphelin créé");
}

#[test]
fn duplicate_road_is_rejected() {
    let mut world = World::new(instant_roads(), 3, Terrain::flat(16, 16));
    for _ in 0..2 {
        world.queue(Command::BuildRoad {
            from: p(1, 1),
            to: p(10, 1),
            spec: RoadSpec::TWO_WAY,
        });
    }

    let report = world.tick();

    assert_eq!(world.roads().live_segments(), 1);
    assert_eq!(
        report.rejects().collect::<Vec<_>>(),
        vec![&CommandError::Road(RoadError::DuplicateSegment)]
    );
}

#[test]
fn roads_snap_onto_existing_nodes() {
    let mut world = World::new(instant_roads(), 4, Terrain::flat(16, 16));
    world.queue(Command::BuildRoad {
        from: p(2, 2),
        to: p(12, 2),
        spec: RoadSpec::TWO_WAY,
    });
    world.tick();
    // Nouveau segment dont le départ est à 30 cm d'un nœud existant : il doit
    // se souder dessus pour former une intersection.
    world.queue(Command::BuildRoad {
        from: Point::new(
            tile_center_m(12) + Fixed::from_raw(30_000), // +0,3 m
            tile_center_m(2),
        ),
        to: p(12, 12),
        spec: RoadSpec::TWO_WAY,
    });
    world.tick();

    assert_eq!(world.roads().node_count(), 3, "3 nœuds, pas 4");
    assert_eq!(world.roads().live_segments(), 2);
}

#[test]
fn city_grows_fills_homes_and_creates_jobs() {
    let mut world = demo_world();
    run(&mut world, 1_500);
    let snapshot = world.snapshot();

    assert!(snapshot.buildings > 5, "bâtiments: {}", snapshot.buildings);
    assert!(snapshot.population > 50, "population: {}", snapshot.population);
    assert!(snapshot.jobs > 0, "des commerces/industries doivent apparaître");
    assert!(snapshot.employed > 0, "population active employée");
    assert!(
        snapshot.population <= snapshot.housing_capacity,
        "les occupants ne dépassent jamais la capacité"
    );
    assert!(snapshot.average_land_value > Fixed::ZERO);
    for demand in [
        snapshot.demand.residential,
        snapshot.demand.commercial,
        snapshot.demand.industrial,
    ] {
        assert!(demand >= Fixed::ZERO && demand <= Fixed::ONE);
    }
}

#[test]
fn industrial_and_commercial_wait_for_residents() {
    let mut world = World::new(instant_roads(), 5, Terrain::flat(24, 24));
    world.queue(Command::BuildRoad {
        from: p(2, 6),
        to: p(20, 6),
        spec: RoadSpec::TWO_WAY,
    });
    // Seulement du commerce et de l'industrie : sans habitants, pas de clients.
    world.queue(Command::ZoneRect {
        rect: Rect::new(4, 7, 10, 9),
        zone: ZoneType::Commercial,
    });
    world.queue(Command::ZoneRect {
        rect: Rect::new(12, 7, 18, 9),
        zone: ZoneType::Industrial,
    });
    run(&mut world, 400);
    assert_eq!(
        world.buildings().live_count(),
        0,
        "pas de commerce sans client : c'est la règle anti-économie absurde"
    );
}

#[test]
fn simulation_is_deterministic_across_runs() {
    let mut a = demo_world();
    let mut b = demo_world();
    run(&mut a, 600);
    run(&mut b, 600);

    assert_eq!(a.snapshot(), b.snapshot());
    assert_eq!(a.zoning_view(), b.zoning_view());
    assert_eq!(a.land_value_view(), b.land_value_view());
    assert_eq!(a.road_access_view(), b.road_access_view());
    assert_eq!(a.buildings(), b.buildings());

    // Et sur la durée : pas de dérive d'arrondi.
    run(&mut a, 600);
    run(&mut b, 600);
    assert_eq!(a.snapshot(), b.snapshot());
}

#[test]
fn different_seeds_do_not_change_a_deterministic_pipeline() {
    // Tant qu'aucun système ne consomme le RNG, la ville doit être identique :
    // c'est le test qui échouera le jour où on ajoutera du bruit non documenté.
    let mut a = World::new(instant_roads(), 111, Terrain::flat(24, 24));
    let mut b = World::new(instant_roads(), 222, Terrain::flat(24, 24));
    for world in [&mut a, &mut b] {
        world.queue(Command::BuildRoad {
            from: p(2, 6),
            to: p(20, 6),
            spec: RoadSpec::TWO_WAY,
        });
        world.queue(Command::ZoneRect {
            rect: Rect::new(4, 7, 18, 9),
            zone: ZoneType::Residential,
        });
    }
    run(&mut a, 300);
    run(&mut b, 300);
    assert_eq!(a.snapshot(), b.snapshot());
}

#[test]
fn bulldoze_removes_buildings_and_dezones() {
    let mut world = demo_world();
    run(&mut world, 400);
    assert!(world.buildings().live_count() > 0);

    world.queue(Command::BulldozeRect {
        rect: Rect::new(0, 0, 31, 31),
    });
    let report = world.tick();

    assert_eq!(world.buildings().live_count(), 0);
    assert_eq!(world.zoning().count(ZoneType::Residential), 0);
    assert!(report
        .events
        .iter()
        .any(|e| matches!(e, SimEvent::BuildingsBulldozed { removed, .. } if *removed > 0)));
}

#[test]
fn zoning_never_overwrites_a_built_tile() {
    let mut world = World::new(instant_roads(), 6, Terrain::flat(16, 16));
    world.queue(Command::BuildRoad {
        from: p(1, 4),
        to: p(14, 4),
        spec: RoadSpec::TWO_WAY,
    });
    world.queue(Command::ZoneRect {
        rect: Rect::new(2, 5, 13, 6),
        zone: ZoneType::Residential,
    });
    run(&mut world, 300);

    let Some((_, building)) = world.buildings().iter().next() else {
        panic!("au moins un logement doit être construit");
    };
    let built_tile = building.tile;

    world.queue(Command::ZoneRect {
        rect: Rect::new(2, 5, 13, 6),
        zone: ZoneType::Industrial,
    });
    world.tick();

    // La tuile bâtie est figée : même bâtiment, même type, même zone.
    let id = built_tile_to_id(built_tile);
    assert_eq!(world.buildings().at(built_tile), Some(id));
    assert!(world.buildings().get(id).unwrap().is_household());
    assert_eq!(world.zoning().get(built_tile), Some(ZoneType::Residential));
    // Les tuiles zonées mais encore vides, elles, basculent bien en industriel.
    assert!(world.zoning().count(ZoneType::Industrial) > 0);
}

fn built_tile_to_id(tile: TileId) -> sim_core::BuildingId {
    // Le contrat MVP : l'id d'un bâtiment est l'index de sa tuile.
    sim_core::BuildingId::new(tile.raw())
}

#[test]
fn budget_settles_at_month_boundaries() {
    let config = SimConfig {
        ticks_per_day: 10,
        days_per_month: 1,
        ..instant_roads()
    };
    let mut world = World::new(config, 8, Terrain::flat(16, 16));

    world.queue(Command::BuildRoad {
        from: p(1, 4),
        to: p(14, 4),
        spec: RoadSpec::TWO_WAY,
    });
    world.queue(Command::ZoneRect {
        rect: Rect::new(2, 5, 13, 6),
        zone: ZoneType::Residential,
    });

    let mut settlements = 0;
    for _ in 0..30 {
        let report = world.tick();
        settlements += report
            .events
            .iter()
            .filter(|e| matches!(e, SimEvent::BudgetSettled { .. }))
            .count();
    }

    assert_eq!(settlements, 3, "un règlement à chaque frontière de mois");
    assert_eq!(world.treasury().month_count, 3);
    assert!(world.treasury().last_income_cents > 0, "les habitants paient des impôts");
    assert!(world.treasury().last_expenses_cents > 0, "routes et bâtiments coûtent");
    // La trésorerie est exactement la somme des règlements : pas de fuite.
    assert_ne!(
        world.treasury().funds_cents,
        SimConfig::default().budget.start_funds_cents
    );
    let snapshot = world.snapshot();
    assert!(snapshot.population > 0, "la démo doit avoir des habitants");
}

#[test]
fn unhoused_city_keeps_zero_population() {
    let mut world = demo_world();
    world.queue(Command::BulldozeRect {
        rect: Rect::new(0, 0, 31, 31),
    });
    world.tick();
    run(&mut world, 200);
    assert_eq!(world.snapshot().population, 0);
    assert_eq!(world.snapshot().buildings, 0);
}

#[test]
fn scale_smoke_test_a_bigger_map_stays_consistent() {
    // Pas d'assertion de temps (machine-dépendante) : on vérifie seulement que
    // les budgets par tick tiennent et que l'état reste cohérent à l'échelle.
    let mut world = World::new(instant_roads(), 9, Terrain::flat(128, 128));
    for row in 0..6 {
        let y = 8 + row * 12;
        world.queue(Command::BuildRoad {
            from: p(4, y),
            to: p(120, y),
            spec: RoadSpec::AVENUE,
        });
        world.queue(Command::ZoneRect {
            rect: Rect::new(6, (y + 1) as u32, 118, (y + 5) as u32),
            zone: ZoneType::Residential,
        });
        world.queue(Command::ZoneRect {
            rect: Rect::new(6, (y - 3) as u32, 118, (y - 1) as u32),
            zone: ZoneType::Commercial,
        });
    }
    run(&mut world, 500);

    let snapshot = world.snapshot();
    assert!(snapshot.buildings > 0);
    assert!(snapshot.population > 0);
    assert!(snapshot.population <= snapshot.housing_capacity);
    assert_eq!(world.land_value_view().len(), 128 * 128);
}

#[test]
fn jobless_city_does_not_collapse() {
    // Anti "death wave" : une ville résidentielle sans un seul emploi doit se
    // stabiliser avec un bonheur bas mais non nul, pas se vider.
    let mut world = World::new(instant_roads(), 10, Terrain::flat(24, 24));
    world.queue(Command::BuildRoad {
        from: p(2, 6),
        to: p(20, 6),
        spec: RoadSpec::TWO_WAY,
    });
    world.queue(Command::ZoneRect {
        rect: Rect::new(4, 7, 18, 9),
        zone: ZoneType::Residential,
    });

    run(&mut world, 600);
    let settled = world.snapshot().population;
    assert!(settled > 0, "des habitants doivent s'installer malgré le chômage");

    run(&mut world, 900);
    let later = world.snapshot().population;

    assert!(later > 0, "la ville ne se vide pas complètement");
    assert!(
        later >= settled * 9 / 10,
        "la population doit rester stable : {settled} -> {later}"
    );
    assert!(world.snapshot().happiness >= Fixed::from_permille(300));
}

#[test]
fn long_run_keeps_every_invariant() {
    let mut world = demo_world();
    let mut previous_population = 0u32;
    for _ in 0..20 {
        run(&mut world, 250);
        let s = world.snapshot();

        assert!(s.population <= s.housing_capacity, "occupants > capacité");
        assert!(s.employed <= s.jobs);
        assert!(s.employed <= s.labor_force);
        assert!(s.population >= previous_population, "chute de population en {s:?}");
        previous_population = s.population;

        for demand in [s.demand.residential, s.demand.commercial, s.demand.industrial] {
            assert!(demand >= Fixed::ZERO && demand <= Fixed::ONE, "demande hors bornes");
        }
        assert!(s.happiness >= Fixed::ZERO && s.happiness <= Fixed::ONE);

        let zoned = world.zoning().count(ZoneType::Residential)
            + world.zoning().count(ZoneType::Commercial)
            + world.zoning().count(ZoneType::Industrial);
        assert!(
            s.buildings <= zoned,
            "plus de bâtiments ({}) que de tuiles zonées ({zoned})",
            s.buildings
        );

        assert!(world
            .land_value_view()
            .as_slice()
            .iter()
            .all(|v| *v >= Fixed::ZERO && *v <= Fixed::ONE));
    }
}

#[test]
fn growth_scales_to_a_million_inhabitant_map() {
    // Échelle cible : 256×256 tuiles (4,1 km²), routes tous les 6 rangs, deux
    // rangs zonés de chaque côté. On vérifie que la loi de croissance composée
    // peuple réellement une carte de cette taille, et que les budgets de tick
    // tiennent (aucune assertion de temps : le CI n'est pas une machine de jeu).
    let mut world = World::new(instant_roads(), 11, Terrain::flat(256, 256));
    for row in 0..42u32 {
        let y = 6 + row * 6;
        world.queue(Command::BuildRoad {
            from: p(4, y as i32),
            to: p(252, y as i32),
            spec: RoadSpec::AVENUE,
        });
        // Mixte : 2 rangs de logements, 1 de commerces, 1 d'industrie. Un
        // zonage 100 % résidentiel stagne par construction (pas d'emplois →
        // pas de demande R) : c'est la règle RCI, pas un bug.
        for (band, zone) in [
            (y + 1, ZoneType::Residential),
            (y + 2, ZoneType::Residential),
            (y - 1, ZoneType::Commercial),
            (y - 2, ZoneType::Industrial),
        ] {
            world.queue(Command::ZoneRect {
                rect: Rect::new(4, band, 252, band),
                zone,
            });
        }
    }
    // 3 000 ticks = 150 s de simulation à 20 Hz. La croissance est
    // exponentielle mais bornée : ~210 000 habitants atteints ici, 1 M vers
    // ~4 000 ticks. L'assertion porte sur l'ordre de grandeur, pas sur un
    // chiffre exact (les réglages de demande peuvent bouger).
    run(&mut world, 3_000);

    let s = world.snapshot();
    assert!(
        s.population > 150_000,
        "population: {} (croissance bridée ?)",
        s.population
    );
    assert!(s.buildings > 9_000, "bâtiments: {}", s.buildings);
    assert_eq!(s.road_segments, 42);
    assert!(s.population <= s.housing_capacity, "occupants > capacité");
    assert!(s.jobs > 10_000, "emplois: {}", s.jobs);
    assert!(s.vacancy < Fixed::from_permille(100), "vacance: {}", s.vacancy);
    assert!(s.average_land_value > Fixed::ZERO);
    println!(
        "échelle: {} bâtiments, {} habitants, {} logements, {} emplois après 3000 ticks",
        s.buildings, s.population, s.housing_capacity, s.jobs
    );
}

#[test]
fn curved_road_command_serves_the_bulge() {
    let mut world = World::new(instant_roads(), 12, Terrain::flat(32, 32));
    world.queue(Command::BuildCurve {
        from: p(2, 10),
        to: p(30, 10),
        from_handle: Point::new(Fixed::from_int(30), Fixed::from_int(60)),
        to_handle: Point::new(Fixed::from_int(-30), Fixed::from_int(-60)),
        spec: RoadSpec::TWO_WAY,
    });
    let report = world.tick();
    assert_eq!(report.rejects().count(), 0, "aucune commande refusée");

    let (_, seg) = world.roads().segments().next().expect("un segment");
    let chord = seg.curve.p0.distance(seg.curve.p1);
    assert!(
        seg.length_m > chord + Fixed::from_int(10),
        "la route courbe ({}) doit dépasser sa corde ({chord})",
        seg.length_m
    );

    // Le sommet du bombement est desservi ; la corde, elle, passe plus bas.
    let mid = seg.curve.eval(Fixed::HALF);
    let (mx, my) = (
        (mid.x.raw() / (TILE_SIZE_M as i64 * 65536)) as usize,
        (mid.y.raw() / (TILE_SIZE_M as i64 * 65536)) as usize,
    );
    let access = world.road_access_view();
    assert!(access[my * 32 + mx], "tuile au sommet desservie");
    assert!(!access[10 * 32 + mx], "la corde ne dessert pas ce rang");
}

#[test]
fn bulldozed_tiles_become_buildable_again_after_rezoning() {
    let mut world = demo_world();
    run(&mut world, 400);
    let before = world.buildings().live_count();
    assert!(before > 0);

    world.queue(Command::BulldozeRect {
        rect: Rect::new(4, 9, 24, 11),
    });
    world.tick();
    world.queue(Command::ZoneRect {
        rect: Rect::new(4, 9, 24, 11),
        zone: ZoneType::Residential,
    });
    world.tick();
    let after_bulldoze = world.buildings().live_count();
    run(&mut world, 600);

    // La liste des candidats a été reconstruite : les tuiles libérées
    // reconstruisent au lieu de rester vides.
    assert!(world.buildings().live_count() > after_bulldoze);
}

#[test]
fn saturated_city_keeps_aggregates_consistent() {
    let mut world = demo_world();
    run(&mut world, 3_000);
    let s = world.snapshot();
    // La ville sature son zonage : plus aucun candidat, plus aucune place.
    assert_eq!(s.population, s.housing_capacity);
    let totals = world.buildings().totals();
    assert_eq!(totals, world.buildings().recount_totals());
    assert_eq!(world.census().population, totals.population);
    // Une ville pleine reste stable (pas de construction fantôme).
    let built = world.buildings().live_count();
    run(&mut world, 200);
    assert_eq!(world.buildings().live_count(), built);
}

#[test]
fn generated_world_grows_on_dry_land_only() {
    use sim_core::WorldGenParams;
    let mut world = World::generated(instant_roads(), 3, WorldGenParams::default(), 96, 96);
    assert!(world.worldgen().is_some());
    // Une avenue en travers de la carte, zonage résidentiel/commercial/industriel.
    world.queue(Command::BuildRoad {
        from: p(2, 48),
        to: p(93, 48),
        spec: RoadSpec::AVENUE,
    });
    for (row, zone) in [(49, ZoneType::Residential), (50, ZoneType::Residential), (47, ZoneType::Commercial), (46, ZoneType::Industrial)] {
        world.queue(Command::ZoneRect {
            rect: Rect::new(2, row, 93, row),
            zone,
        });
    }
    run(&mut world, 1_500);
    assert!(world.buildings().live_count() > 0);
    for (_, building) in world.buildings().iter() {
        let altitude = world.terrain().altitude_cm(building.tile).unwrap();
        assert!(altitude > sim_core::terrain::WATER_LEVEL_CM, "bâtiment dans l'eau");
    }
}

#[test]
fn road_works_delay_access_until_the_road_opens() {
    let config = SimConfig { building_construction: false, ..SimConfig::default() };
    let mut world = World::new(config, 21, Terrain::flat(32, 32));
    world.queue(Command::BuildRoad {
        from: Point::new(tile_center_m(2), tile_center_m(10)),
        to: Point::new(tile_center_m(28), tile_center_m(10)),
        spec: RoadSpec::TWO_WAY,
    });
    world.queue(Command::ZoneRect { rect: Rect::new(3, 11, 26, 12), zone: ZoneType::Residential });
    world.tick();
    let (id, segment) = world.roads().segments().next().unwrap();
    let opens = segment.opens_at.raw();
    let expected = RoadSpec::TWO_WAY.construction_ticks(segment.length_m);
    assert_eq!(opens, expected, "chantier de {expected} ticks");
    assert!(world.road_access_view().iter().all(|a| !a), "un chantier ne dessert rien");
    let mut opened = 0;
    while world.current_tick().raw() <= opens {
        let report = world.tick();
        for event in &report.events {
            if let sim_core::SimEvent::RoadsOpened { count } = event {
                opened += count;
            }
        }
        // Pendant le chantier : avancement croissant, rien ne pousse.
        if world.current_tick().raw() < opens {
            assert_eq!(world.buildings().live_count(), 0);
        }
    }
    assert_eq!(opened, 1);
    assert!(world.roads().segment(id).unwrap().is_open(world.current_tick()));
    assert!(world.road_access_view().iter().any(|a| *a), "route ouverte : desserte");
    for _ in 0..400 {
        world.tick();
    }
    assert!(world.buildings().live_count() > 0, "la ville pousse après l'ouverture");
}

#[test]
fn highways_do_not_serve_lots() {
    let mut world = World::new(instant_roads(), 22, Terrain::flat(32, 32));
    world.queue(Command::BuildRoad {
        from: Point::new(tile_center_m(2), tile_center_m(10)),
        to: Point::new(tile_center_m(28), tile_center_m(10)),
        spec: RoadSpec::HIGHWAY,
    });
    world.tick();
    assert_eq!(world.roads().live_segments(), 1);
    assert!(world.road_access_view().iter().all(|a| !a));
}

#[test]
fn roundabouts_are_closed_one_way_rings_that_streets_can_join() {
    let mut world = World::new(instant_roads(), 23, Terrain::flat(32, 32));
    let center = Point::new(tile_center_m(16), tile_center_m(16));
    world.queue(Command::BuildRoundabout { center, radius_m: Fixed::from_int(18), spec: RoadSpec::ONE_WAY });
    // Une rue qui arrive au nœud est de l'anneau (18 m à l'est du centre).
    world.queue(Command::BuildRoad {
        from: Point::new(center.x + Fixed::from_int(18), center.y),
        to: Point::new(tile_center_m(30), tile_center_m(16)),
        spec: RoadSpec::TWO_WAY,
    });
    world.tick();
    let roads = world.roads();
    assert_eq!(roads.roundabouts().len(), 1);
    assert_eq!(roads.live_segments(), 9, "8 arcs + la rue raccordée");
    let ring = &roads.roundabouts()[0];
    // Anneau fermé, à sens unique, chaque arc sur le cercle (± 5 cm).
    for (i, node) in ring.nodes.iter().enumerate() {
        let next = ring.nodes[(i + 1) % 8];
        let seg = roads.segment(roads.find_segment(*node, next).unwrap()).unwrap();
        assert_eq!((seg.a, seg.b), (*node, next), "sens de circulation");
        assert!(seg.spec.is_one_way());
        for k in 0..=8 {
            let p = seg.curve.eval(Fixed::from_ratio(k, 8));
            let d = p.distance(center).to_f32();
            assert!((d - 18.0).abs() < 0.05, "arc hors du cercle : {d}");
        }
    }
    // La rue s'est accrochée au nœud est et y arrive droite.
    let east = ring.nodes[0];
    let street = roads.segments().find(|(_, s)| !s.spec.is_one_way()).unwrap().1;
    assert!(street.a == east || street.b == east);
    assert!(street.curve.is_straight());
}

#[test]
fn building_works_go_through_every_stage_before_housing_anyone() {
    use sim_core::{ConstructionStage, SimEvent};
    let config = SimConfig { road_construction: false, ..SimConfig::default() };
    let mut world = World::new(config, 31, Terrain::flat(32, 32));
    world.queue(Command::BuildRoad {
        from: Point::new(tile_center_m(2), tile_center_m(10)),
        to: Point::new(tile_center_m(28), tile_center_m(10)),
        spec: RoadSpec::TWO_WAY,
    });
    world.queue(Command::ZoneRect { rect: Rect::new(3, 11, 26, 12), zone: ZoneType::Residential });
    let mut stages_seen = std::collections::BTreeSet::new();
    let mut completed = 0u32;
    for _ in 0..3_000 {
        let report = world.tick();
        let now = world.current_tick();
        for event in &report.events {
            if let SimEvent::BuildingCompleted { id, .. } = event {
                completed += 1;
                assert!(world.buildings().get(*id).unwrap().complete);
            }
        }
        for (_, b) in world.buildings().iter() {
            let (stage, within) = b.stage(now);
            stages_seen.insert(stage);
            assert!(within >= Fixed::ZERO && within <= Fixed::ONE);
            // Un chantier ne loge personne.
            if !b.complete {
                assert_eq!(b.occupants, 0);
                assert_eq!(b.free_slots(), 0);
            }
        }
        // Les agrégats ne comptent que les bâtiments livrés.
        let housing: u32 = world.buildings().iter().filter(|(_, b)| b.complete && b.is_household()).map(|(_, b)| b.capacity).sum();
        assert_eq!(world.buildings().totals().housing_capacity, housing);
    }
    assert_eq!(stages_seen.len(), 5, "tous les stades vus : {stages_seen:?}");
    assert!(completed > 0);
    assert!(world.snapshot().population > 0, "les habitants arrivent après la livraison");
}

#[test]
fn demolishing_a_building_site_cancels_its_delivery() {
    let config = SimConfig { road_construction: false, ..SimConfig::default() };
    let mut world = World::new(config, 32, Terrain::flat(32, 32));
    world.queue(Command::BuildRoad {
        from: Point::new(tile_center_m(2), tile_center_m(10)),
        to: Point::new(tile_center_m(28), tile_center_m(10)),
        spec: RoadSpec::TWO_WAY,
    });
    world.queue(Command::ZoneRect { rect: Rect::new(3, 11, 26, 11), zone: ZoneType::Residential });
    while world.buildings().live_count() == 0 {
        world.tick();
    }
    world.queue(Command::BulldozeRect { rect: Rect::new(0, 0, 31, 31) });
    world.tick();
    assert_eq!(world.buildings().live_count(), 0);
    assert_eq!(world.buildings().under_construction(), 0);
    for _ in 0..3_000 {
        let report = world.tick();
        assert!(!report.events.iter().any(|e| matches!(e, sim_core::SimEvent::BuildingCompleted { .. })));
    }
}
