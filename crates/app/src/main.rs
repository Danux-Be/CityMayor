//! Hôte `citymayor` — démonstration headless du vertical slice.
//!
//! Ce binaire ne fait *que* ce qu'un vrai hôte fera à l'itération 2 :
//! 1. construire un `World` ;
//! 2. empiler des commandes (comme le ferait l'UI) ;
//! 3. avancer la simulation avec une horloge à pas fixe ;
//! 4. afficher les snapshots (comme le ferait le HUD).
//!
//! Il ne connaît ni moteur de rendu ni ECS : c'est la preuve que `sim-core`
//! tourne seul, en test d'intégration comme en CI.

use std::time::Duration;

use sim_core::{
    tile_center_m, Command, Fixed, Overlay, Point, Rect, RoadSpec, SimConfig, SimEvent, Terrain,
    World, ZoneType, TILE_SIZE_M,
};

fn main() {
    // Calendrier accéléré pour voir plusieurs règlements budgétaires en 30 s de
    // démo. En jeu : 1200 ticks/jour (60 s de jeu réel) et 30 jours/mois.
    let config = SimConfig {
        ticks_per_day: 200,
        days_per_month: 1,
        ..SimConfig::default()
    };

    let terrain = Terrain::flat(48, 48);
    let mut world = World::new(config, 0x0C17_CAFE, terrain);

    // --- Plan de ville minimal -------------------------------------------------
    // Une avenue horizontale + une rue verticale : tout le reste dépend de ces
    // deux segments pour être desservi (et donc constructible).
    let road = |world: &mut World, ax: i32, ay: i32, bx: i32, by: i32, spec: RoadSpec| {
        world.queue(Command::BuildRoad {
            from: Point::new(tile_center_m(ax), tile_center_m(ay)),
            to: Point::new(tile_center_m(bx), tile_center_m(by)),
            spec,
        });
    };
    road(&mut world, 2, 10, 44, 10, RoadSpec::AVENUE);
    road(&mut world, 10, 10, 10, 40, RoadSpec::TWO_WAY);

    let zone = |world: &mut World, x0: u32, y0: u32, x1: u32, y1: u32, zone: ZoneType| {
        world.queue(Command::ZoneRect {
            rect: Rect::new(x0, y0, x1, y1),
            zone,
        });
    };
    zone(&mut world, 12, 11, 38, 13, ZoneType::Residential);
    zone(&mut world, 11, 20, 12, 34, ZoneType::Residential);
    zone(&mut world, 16, 8, 34, 9, ZoneType::Commercial);
    zone(&mut world, 11, 32, 14, 40, ZoneType::Industrial);

    println!(
        "Carte {}x{} tuiles de {TILE_SIZE_M} m — tick {:.0} Hz, {} ticks/jour",
        world.terrain().width(),
        world.terrain().height(),
        1.0 / 0.05,
        world.config().ticks_per_day
    );
    print_header();

    let mut clock = sim_core::FixedStepClock::from_hz(config.tick_hz);
    let frame = Duration::from_millis(16);

    for frame_index in 0..2_000u32 {
        // Le seul endroit où l'hôte parle temps mural ; le monde n'en saura rien.
        let batch = clock.advance(frame);
        for _ in 0..batch.ticks {
            let report = world.tick();
            for event in &report.events {
                match event {
                    SimEvent::BudgetSettled { report } => println!(
                        "  $ budget mois {} : +{:.0} / -{:.0} = {:+} -> {:.0} $",
                        world.treasury().month_count,
                        report.income_cents as f32 / 100.0,
                        report.expenses_cents as f32 / 100.0,
                        report.net_cents / 100,
                        report.funds_cents as f32 / 100.0
                    ),
                    SimEvent::CommandRejected { error } => {
                        println!("  ! commande refusée : {error}");
                    }
                    _ => {}
                }
            }
        }
        if frame_index % 250 == 0 {
            print_snapshot(&world);
        }
    }

    print_snapshot(&world);
    print_overlay_sample(&world);
}

fn print_header() {
    println!(
        "  {:>7} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>9}",
        "tick", "pop", "capac", "emplois", "chom.", "bonheur", "demR", "demC", "demI", "fonds($)"
    );
}

fn print_snapshot(world: &World) {
    let s = world.snapshot();
    println!(
        "  {:>7} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>9}",
        s.tick.raw(),
        s.population,
        s.housing_capacity,
        s.jobs,
        s.unemployed,
        s.happiness.to_f32(),
        s.demand.residential.to_f32(),
        s.demand.commercial.to_f32(),
        s.demand.industrial.to_f32(),
        s.funds_cents / 100,
    );
}

fn print_overlay_sample(world: &World) {
    let snapshot = world.snapshot();
    println!(
        "\nBâtiments: {}, segments: {}, valeur foncière moyenne (zonée): {}",
        snapshot.buildings,
        snapshot.road_segments,
        snapshot.average_land_value
    );
    println!(
        "Jours écoulés: {}, mois: {}, heure: {}h",
        snapshot.day, snapshot.month, snapshot.hour
    );

    let mut implemented = Vec::new();
    let mut planned = Vec::new();
    for overlay in Overlay::ALL {
        if overlay.is_implemented() {
            implemented.push(overlay.label());
        } else {
            planned.push(overlay.label());
        }
    }
    println!("Overlays prêts      : {}", implemented.join(", "));
    println!("Overlays à venir    : {}", planned.join(", "));

    // Échantillon d'overlay : la tuile sous le premier logement construit.
    let sample = world.buildings().iter().next().map(|(_, b)| b.tile);
    if let Some(tile) = sample {
        let zone = world.overlay_cell(Overlay::Zone, tile).unwrap_or(Fixed::ZERO);
        let value = world
            .overlay_cell(Overlay::LandValue, tile)
            .unwrap_or(Fixed::ZERO);
        let access = world
            .overlay_cell(Overlay::RoadAccess, tile)
            .unwrap_or(Fixed::ZERO);
        println!(
            "Échantillon tuile {tile} : zone={zone}, valeur={value}, desserte={access}"
        );
    }
}
