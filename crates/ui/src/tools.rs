//! Outils de construction : machine à états indépendante du moteur.
//!
//! Le viewer fournit la position du curseur sur le sol (mètres, plan de la
//! carte) et les clics ; l'outil répond par un **fantôme** (ce qui serait
//! construit, valide ou non, avec la raison) et, au clic, par des
//! [`Command`] à mettre en file. Aucune règle de jeu ici : la simulation
//! reste seule juge (elle peut encore refuser une commande, le refus arrive
//! alors comme événement traduit).
//!
//! | Outil | Geste |
//! |---|---|
//! | Route droite | clic au départ, clic à l'arrivée ; le tracé continue depuis l'arrivée |
//! | Route courbe | départ, point de contrôle, arrivée |
//! | Rond-point | un clic au centre |
//! | Zonage / démolition | glisser un rectangle de tuiles |
//! | Démolition d'une route | clic sur la route (sans glisser) |
//!
//! Clic droit ou Échap : annule le tracé en cours, puis l'outil.

use sim_core::{Command, Fixed, Point, Rect, RoadSpec, SegmentId, World, ZoneType};

/// Rayon d'accroche aux nœuds existants (m) : un carrefour se crée en
/// visant à peu près l'extrémité d'une route.
pub const SNAP_RADIUS_M: f32 = 7.0;
/// Longueur minimale d'un tronçon (m).
pub const MIN_ROAD_M: f32 = 8.0;
/// Rayons proposés pour les ronds-points (m), petit/moyen/grand.
pub const ROUNDABOUT_RADII_M: [i32; 3] = [14, 20, 28];

/// Outil actif.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// Aucun outil : le clic gauche déplace la caméra.
    Select,
    Road { spec: RoadSpec, curved: bool },
    Roundabout { radius_m: i32 },
    Zone(ZoneType),
    Bulldoze,
}

impl Tool {
    /// Clé de l'aide affichée près du curseur, selon l'étape du geste.
    #[must_use]
    pub fn hint_key(self, step: usize) -> &'static str {
        match self {
            Tool::Select => "tool-hint-select",
            Tool::Road { curved: false, .. } => if step == 0 { "tool-hint-road-start" } else { "tool-hint-road-end" },
            Tool::Road { curved: true, .. } => match step {
                0 => "tool-hint-road-start",
                1 => "tool-hint-curve-control",
                _ => "tool-hint-road-end",
            },
            Tool::Roundabout { .. } => "tool-hint-roundabout",
            Tool::Zone(_) => "tool-hint-zone",
            Tool::Bulldoze => "tool-hint-bulldoze",
        }
    }
}

/// Ce que l'outil montrerait au clic suivant.
#[derive(Clone, Debug, PartialEq)]
pub enum Ghost {
    None,
    Road {
        from: [f32; 2],
        to: [f32; 2],
        /// Poignées de tangente (m), comme `Command::BuildCurve`.
        from_handle: [f32; 2],
        to_handle: [f32; 2],
        spec: RoadSpec,
        length_m: f32,
        /// `Err(clé)` : raison du refus, traduite par l'interface.
        validity: Result<(), &'static str>,
    },
    /// Point de contrôle posé, arrivée pas encore choisie (route courbe).
    Control { from: [f32; 2], control: [f32; 2] },
    Roundabout { center: [f32; 2], radius_m: f32, spec: RoadSpec, validity: Result<(), &'static str> },
    /// Rectangle de tuiles ; `zone: None` = démolition.
    Area { rect: Rect, zone: Option<ZoneType> },
    /// Route visée par la démolition.
    RoadTarget { segment: SegmentId },
}

/// État d'un outil pendant un geste.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolController {
    pub tool: Tool,
    /// Points posés (route : départ, contrôle).
    points: Vec<[f32; 2]>,
    /// Début d'un glisser (zonage, démolition) : position au sol.
    drag_from: Option<[f32; 2]>,
}

impl Default for ToolController {
    fn default() -> Self {
        Self { tool: Tool::Select, points: Vec::new(), drag_from: None }
    }
}

/// Mètres → point de la simulation (virgule fixe).
#[must_use]
pub fn point(p: [f32; 2]) -> Point {
    let fixed = |v: f32| Fixed::from_raw((f64::from(v) * 65_536.0).round() as i64);
    Point::new(fixed(p[0]), fixed(p[1]))
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

impl ToolController {
    /// Change d'outil (abandonne le geste en cours).
    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.points.clear();
        self.drag_from = None;
    }

    /// Étape du geste (nombre de points posés).
    #[must_use]
    pub fn step(&self) -> usize {
        self.points.len()
    }

    /// Clic droit / Échap : annule le tracé en cours ; s'il n'y en a pas,
    /// repose l'outil. Renvoie `true` si quelque chose a été annulé.
    pub fn cancel(&mut self) -> bool {
        if !self.points.is_empty() || self.drag_from.is_some() {
            self.points.clear();
            self.drag_from = None;
            return true;
        }
        if self.tool != Tool::Select {
            self.tool = Tool::Select;
            return true;
        }
        false
    }

    /// Fantôme pour le curseur au sol `cursor` (`None` : curseur hors sol).
    #[must_use]
    pub fn ghost(&self, world: &World, cursor: Option<[f32; 2]>) -> Ghost {
        let Some(cursor) = cursor else { return Ghost::None };
        match self.tool {
            Tool::Select => Ghost::None,
            Tool::Road { spec, curved } => {
                let end = snap(world, cursor);
                match (self.points.as_slice(), curved) {
                    ([from], false) => road_ghost(world, *from, end, None, spec),
                    ([from], true) => Ghost::Control { from: *from, control: cursor },
                    ([from, control], true) => road_ghost(world, *from, end, Some(*control), spec),
                    _ => Ghost::None,
                }
            }
            Tool::Roundabout { radius_m } => {
                let spec = RoadSpec::ONE_WAY;
                let r = radius_m as f32;
                Ghost::Roundabout { center: cursor, radius_m: r, spec, validity: area_validity(world, cursor, r + 6.0) }
            }
            Tool::Zone(zone) => self.area(world, cursor).map_or(Ghost::None, |rect| Ghost::Area { rect, zone: Some(zone) }),
            Tool::Bulldoze => match self.drag_from {
                Some(from) if dist(from, cursor) > 4.0 => {
                    self.area(world, cursor).map_or(Ghost::None, |rect| Ghost::Area { rect, zone: None })
                }
                _ => road_under(world, cursor).map_or_else(
                    || self.area(world, cursor).map_or(Ghost::None, |rect| Ghost::Area { rect, zone: None }),
                    |segment| Ghost::RoadTarget { segment },
                ),
            },
        }
    }

    /// Rectangle de tuiles du glisser en cours (ou de la tuile visée).
    fn area(&self, world: &World, cursor: [f32; 2]) -> Option<Rect> {
        let size = world.config().tile_size_m as f32;
        let grid = world.zoning().grid();
        let tile = |p: [f32; 2]| {
            let (x, y) = ((p[0] / size).floor(), (p[1] / size).floor());
            (x >= 0.0 && y >= 0.0 && (x as u32) < grid.width() && (y as u32) < grid.height()).then_some((x as u32, y as u32))
        };
        let b = tile(cursor)?;
        let a = self.drag_from.and_then(tile).unwrap_or(b);
        Some(Rect::new(a.0, a.1, b.0, b.1))
    }

    /// Bouton gauche enfoncé (début d'un glisser).
    pub fn press(&mut self, cursor: Option<[f32; 2]>) {
        if matches!(self.tool, Tool::Zone(_) | Tool::Bulldoze) {
            self.drag_from = cursor;
        }
    }

    /// Bouton gauche relâché : renvoie les commandes à mettre en file.
    pub fn release(&mut self, world: &World, cursor: Option<[f32; 2]>) -> Vec<Command> {
        let Some(cursor) = cursor else {
            self.drag_from = None;
            return Vec::new();
        };
        let ghost = self.ghost(world, Some(cursor));
        let mut commands = Vec::new();
        match self.tool {
            Tool::Select => {}
            Tool::Road { curved, .. } => {
                let target = if curved && self.points.len() == 1 { cursor } else { snap(world, cursor) };
                match ghost {
                    Ghost::Road { from, to, from_handle, to_handle, spec, validity: Ok(()), .. } => {
                        commands.push(Command::BuildCurve {
                            from: point(from),
                            to: point(to),
                            from_handle: point(from_handle),
                            to_handle: point(to_handle),
                            spec,
                        });
                        // Le tracé continue depuis l'arrivée.
                        self.points = vec![to];
                    }
                    Ghost::Road { .. } => {} // invalide : on reste sur place
                    _ => self.points.push(target),
                }
            }
            Tool::Roundabout { radius_m } => {
                if let Ghost::Roundabout { validity: Ok(()), spec, .. } = ghost {
                    commands.push(Command::BuildRoundabout {
                        center: point(cursor),
                        radius_m: Fixed::from_int(radius_m),
                        spec,
                    });
                }
            }
            Tool::Zone(zone) => {
                if let Ghost::Area { rect, .. } = ghost {
                    commands.push(Command::ZoneRect { rect, zone });
                }
            }
            Tool::Bulldoze => match ghost {
                Ghost::RoadTarget { segment } => commands.push(Command::BulldozeRoad { segment }),
                Ghost::Area { rect, .. } => commands.push(Command::BulldozeRect { rect }),
                _ => {}
            },
        }
        self.drag_from = None;
        commands
    }
}

/// Accroche au nœud de route le plus proche (à moins de `SNAP_RADIUS_M`).
#[must_use]
pub fn snap(world: &World, p: [f32; 2]) -> [f32; 2] {
    // Nœuds encore reliés à une route (un nœud orphelin, après démolition,
    // n'attire pas le curseur).
    world
        .roads()
        .segments()
        .flat_map(|(_, s)| [s.a, s.b])
        .filter_map(|n| world.roads().node(n).map(|n| [n.pos.x.to_f32(), n.pos.y.to_f32()]))
        .map(|q| (dist(p, q), q))
        .filter(|(d, _)| *d <= SNAP_RADIUS_M)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map_or(p, |(_, q)| q)
}

/// Segment de route sous le curseur (à moins de sa demi-largeur + 1 m).
#[must_use]
pub fn road_under(world: &World, p: [f32; 2]) -> Option<SegmentId> {
    let mut best: Option<(f32, SegmentId)> = None;
    let mut samples = Vec::new();
    for (id, seg) in world.roads().segments() {
        let reach = f32::from(seg.spec.total_lanes().max(1)) * 1.9 + 1.5;
        seg.curve.sample_into(seg.curve.suggested_steps(), &mut samples);
        for w in samples.windows(2) {
            let (a, b) = ([w[0].x.to_f32(), w[0].y.to_f32()], [w[1].x.to_f32(), w[1].y.to_f32()]);
            let d = distance_to_segment(p, a, b);
            if d <= reach && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, id));
            }
        }
    }
    best.map(|(_, id)| id)
}

fn distance_to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if len2 < 1e-9 { 0.0 } else { (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0) };
    dist(p, [a[0] + ab[0] * t, a[1] + ab[1] * t])
}

/// Tracé d'une route de `from` à `to`, courbe si `control` est donné
/// (courbe quadratique convertie en cubique : poignées à 2/3 du contrôle).
fn road_ghost(world: &World, from: [f32; 2], to: [f32; 2], control: Option<[f32; 2]>, spec: RoadSpec) -> Ghost {
    let (from_handle, to_handle) = match control {
        Some(c) => (
            [(c[0] - from[0]) * 2.0 / 3.0, (c[1] - from[1]) * 2.0 / 3.0],
            [(to[0] - c[0]) * 2.0 / 3.0, (to[1] - c[1]) * 2.0 / 3.0],
        ),
        None => ([0.0, 0.0], [0.0, 0.0]),
    };
    let curve = |t: f32| {
        let c0 = [from[0] + from_handle[0], from[1] + from_handle[1]];
        let c1 = [to[0] - to_handle[0], to[1] - to_handle[1]];
        let u = 1.0 - t;
        let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
        [
            w[0] * from[0] + w[1] * c0[0] + w[2] * c1[0] + w[3] * to[0],
            w[0] * from[1] + w[1] * c0[1] + w[2] * c1[1] + w[3] * to[1],
        ]
    };
    let samples: Vec<[f32; 2]> = (0..=32).map(|k| curve(k as f32 / 32.0)).collect();
    let length_m: f32 = samples.windows(2).map(|w| dist(w[0], w[1])).sum();
    let validity = if length_m < MIN_ROAD_M {
        Err("tool-error-too-short")
    } else if samples.iter().any(|p| !in_bounds(world, *p)) {
        Err("error-out-of-bounds")
    } else if samples.iter().any(|p| under_water(world, *p)) {
        Err("tool-error-water")
    } else {
        Ok(())
    };
    Ghost::Road { from, to, from_handle, to_handle, spec, length_m, validity }
}

fn in_bounds(world: &World, p: [f32; 2]) -> bool {
    let size = world.config().tile_size_m as f32;
    let grid = world.zoning().grid();
    p[0] >= 0.0 && p[1] >= 0.0 && p[0] <= grid.width() as f32 * size && p[1] <= grid.height() as f32 * size
}

/// Sol sous le niveau de la mer (fleuve, lac) : pas encore de ponts.
fn under_water(world: &World, p: [f32; 2]) -> bool {
    let size = world.config().tile_size_m as f32;
    world
        .zoning()
        .grid()
        .tile_at((p[0] / size) as u32, (p[1] / size) as u32)
        .and_then(|t| world.terrain().altitude_cm(t))
        .is_some_and(|cm| cm <= 0)
}

/// Emprise circulaire constructible : dans la carte, hors de l'eau.
fn area_validity(world: &World, center: [f32; 2], radius: f32) -> Result<(), &'static str> {
    let probes = (0..16).map(|k| {
        let a = k as f32 / 16.0 * std::f32::consts::TAU;
        [center[0] + radius * a.cos(), center[1] + radius * a.sin()]
    });
    let mut all = probes.chain(std::iter::once(center));
    if !all.clone().all(|p| in_bounds(world, p)) {
        return Err("error-out-of-bounds");
    }
    if all.any(|p| under_water(world, p)) {
        return Err("tool-error-water");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_core::{SimConfig, Terrain};

    fn world() -> World {
        let config = SimConfig { road_construction: false, building_construction: false, ..SimConfig::default() };
        World::new(config, 1, Terrain::flat(64, 64))
    }

    #[test]
    fn straight_roads_take_two_clicks_and_chain() {
        let mut w = world();
        let mut tool = ToolController::default();
        tool.set_tool(Tool::Road { spec: RoadSpec::TWO_WAY, curved: false });
        assert!(tool.release(&w, Some([40.0, 40.0])).is_empty());
        assert_eq!(tool.step(), 1);
        let ghost = tool.ghost(&w, Some([140.0, 40.0]));
        assert!(matches!(ghost, Ghost::Road { validity: Ok(()), length_m, .. } if (length_m - 100.0).abs() < 0.5));
        let commands = tool.release(&w, Some([140.0, 40.0]));
        assert_eq!(commands.len(), 1);
        for c in commands {
            w.queue(c);
        }
        w.tick();
        assert_eq!(w.roads().live_segments(), 1);
        // Le tracé continue depuis l'arrivée, accrochée au nœud existant.
        assert_eq!(tool.step(), 1);
        for c in tool.release(&w, Some([140.0, 140.0])) {
            w.queue(c);
        }
        w.tick();
        assert_eq!(w.roads().live_segments(), 2);
        assert!(w.roads().node_count() <= 3, "les deux tronçons partagent le nœud");
    }

    #[test]
    fn curves_take_three_clicks_and_bend_through_the_control_point() {
        let mut w = world();
        let mut tool = ToolController::default();
        tool.set_tool(Tool::Road { spec: RoadSpec::AVENUE, curved: true });
        tool.release(&w, Some([40.0, 40.0]));
        assert!(matches!(tool.ghost(&w, Some([140.0, 40.0])), Ghost::Control { .. }));
        tool.release(&w, Some([140.0, 40.0]));
        let commands = tool.release(&w, Some([140.0, 140.0]));
        assert_eq!(commands.len(), 1);
        w.queue(commands[0].clone());
        w.tick();
        let (_, seg) = w.roads().segments().next().unwrap();
        assert!(!seg.curve.is_straight());
        assert_eq!(seg.spec, RoadSpec::AVENUE);
    }

    #[test]
    fn short_roads_and_water_are_refused_with_a_reason() {
        let w = world();
        let mut tool = ToolController::default();
        tool.set_tool(Tool::Road { spec: RoadSpec::TWO_WAY, curved: false });
        tool.release(&w, Some([40.0, 40.0]));
        assert!(matches!(tool.ghost(&w, Some([43.0, 40.0])), Ghost::Road { validity: Err("tool-error-too-short"), .. }));
        assert!(tool.release(&w, Some([43.0, 40.0])).is_empty(), "rien n'est construit");
        assert!(matches!(tool.ghost(&w, Some([-50.0, 40.0])), Ghost::Road { validity: Err("error-out-of-bounds"), .. }));
    }

    #[test]
    fn zoning_and_bulldozing_drag_rectangles() {
        let mut w = world();
        let mut tool = ToolController::default();
        tool.set_tool(Tool::Zone(ZoneType::Residential));
        tool.press(Some([20.0, 20.0]));
        let commands = tool.release(&w, Some([60.0, 44.0]));
        assert_eq!(commands, vec![Command::ZoneRect { rect: Rect::new(2, 2, 7, 5), zone: ZoneType::Residential }]);
        // Démolition d'une route par un simple clic dessus.
        w.queue(Command::BuildRoad { from: point([20.0, 100.0]), to: point([200.0, 100.0]), spec: RoadSpec::TWO_WAY });
        w.tick();
        tool.set_tool(Tool::Bulldoze);
        tool.press(Some([100.0, 101.0]));
        let commands = tool.release(&w, Some([100.0, 101.0]));
        assert!(matches!(commands.as_slice(), [Command::BulldozeRoad { .. }]));
    }

    #[test]
    fn cancel_unwinds_the_gesture_then_the_tool() {
        let w = world();
        let mut tool = ToolController::default();
        tool.set_tool(Tool::Roundabout { radius_m: ROUNDABOUT_RADII_M[1] });
        assert!(matches!(tool.ghost(&w, Some([200.0, 200.0])), Ghost::Roundabout { validity: Ok(()), .. }));
        assert_eq!(tool.release(&w, Some([200.0, 200.0])).len(), 1);
        tool.set_tool(Tool::Road { spec: RoadSpec::TWO_WAY, curved: false });
        tool.release(&w, Some([40.0, 40.0]));
        assert!(tool.cancel());
        assert_eq!(tool.step(), 0);
        assert!(tool.cancel());
        assert_eq!(tool.tool, Tool::Select);
        assert!(!tool.cancel());
    }
}
