//! Interface du viewer : barre d'informations, barre d'outils, fantômes de
//! construction, infobulle et fil d'actualité.
//!
//! # Principes
//! - **Aucun texte en dur** : tout passe par les fichiers de langue
//!   (`Texts`, `city_ui::text`).
//! - **Aucune règle de jeu** : les outils (`city_ui::tools`) produisent des
//!   commandes ; la simulation reste seule juge et ses refus s'affichent dans
//!   le fil d'actualité.
//! - **Survol maison** : chaque bouton teste le curseur contre sa boîte
//!   calculée (`ComputedNode::contains_point`), sans système de picking. Un
//!   clic sur un panneau n'atteint jamais le monde.

use std::collections::VecDeque;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::prelude::*;
use bevy::text::{FontSize, FontSource, FontWeight};
use bevy::ui::{ComputedNode, UiGlobalTransform};

use city_ui::text;
use city_ui::tools::{Ghost, Tool, ToolController, ROUNDABOUT_RADII_M};
use city_ui::Arg;
use sim_core::{Command, Fixed, Overlay, RoadSpec, SegmentId, TileId, World, ZoneType};

use crate::nature::ground_altitude_m;
use crate::scene::RoadStrip;
use crate::viewer::{urban_mesh_public, Sim, Texts};
use crate::{roads, urban};

/// Police de l'interface (Inter, OFL — `assets/fonts`).
pub(crate) const FONT_TTF: &[u8] = include_bytes!("../assets/fonts/InterVariable.ttf");

const PANEL: Color = Color::srgba(0.06, 0.08, 0.11, 0.86);
const BUTTON: Color = Color::srgba(0.15, 0.18, 0.23, 0.95);
const BUTTON_HOVER: Color = Color::srgba(0.23, 0.28, 0.35, 0.98);
const BUTTON_ACTIVE: Color = Color::srgba(0.16, 0.42, 0.74, 1.0);
const TEXT: Color = Color::srgb(0.95, 0.96, 0.98);
const TEXT_DIM: Color = Color::srgb(0.62, 0.66, 0.72);
const TEXT_ERROR: Color = Color::srgb(1.0, 0.45, 0.4);

#[derive(Debug)]
pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Tools>().init_resource::<Feed>().init_resource::<PointerOverHud>();
        app.insert_resource(ToolTest::from_env());
        // Diagnostic de performance (`CITYMAYOR_DISABLE=hud`).
        if std::env::var("CITYMAYOR_DISABLE").is_ok_and(|list| list.split(',').any(|f| f.trim() == "hud")) {
            return;
        }
        app
            .add_systems(OnEnter(crate::screens::AppState::InGame), setup_hud)
            .add_systems(
                Update,
                (
                    hud_hover,
                    hud_buttons,
                    hud_keys,
                    tool_input,
                    tool_test,
                    update_ghost,
                    update_stats,
                    update_feed,
                    update_tooltip,
                    update_panels,
                    update_overlay,
                )
                    .chain()
                    .run_if(in_state(crate::screens::AppState::InGame)),
            );
    }
}

/// Outil courant.
#[derive(Resource, Default, Debug)]
pub struct Tools {
    pub controller: ToolController,
    /// Catégorie dont le sous-menu est ouvert.
    open: Option<Category>,
    /// Paramètres mémorisés de chaque outil.
    road: usize,
    curved: bool,
    roundabout: usize,
    /// Point au sol sous le curseur (m, plan de la carte).
    cursor: Option<[f32; 2]>,
    ghost: Option<Ghost>,
    /// Vue d'information affichée sur le sol.
    overlay: Option<Overlay>,
    budget_open: bool,
    /// Bâtiment ou route dont le panneau d'infos est ouvert.
    selected: Option<Selected>,
    /// Position du curseur à l'appui du bouton gauche (clic ou glisser ?).
    press_at: Option<Vec2>,
}

/// Objet inspecté.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Selected {
    Building(TileId),
    Road(SegmentId),
}

/// Panneaux à lignes « libellé — valeur ».
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PanelKind {
    Budget,
    Info,
}

/// Lignes par panneau.
const PANEL_ROWS: usize = 8;

#[derive(Component)]
struct PanelRoot(PanelKind);

#[derive(Component)]
struct PanelTitle(PanelKind);

#[derive(Component)]
struct PanelRow(PanelKind, usize);

#[derive(Component)]
struct RowLabel(PanelKind, usize);

#[derive(Component)]
struct RowValue(PanelKind, usize);

#[derive(Component)]
struct TaxValue(usize);

#[derive(Component)]
struct Legend;

#[derive(Component)]
struct LegendText(usize);

#[derive(Component)]
struct LegendSwatch(usize);

#[derive(Component)]
struct OverlayMesh;

/// Le curseur est-il sur un panneau (le clic n'atteint pas le monde) ?
#[derive(Resource, Default, Debug)]
pub struct PointerOverHud(pub bool);

/// Messages récents (texte, âge en secondes, erreur ?).
#[derive(Resource, Default, Debug)]
pub struct Feed {
    items: VecDeque<(String, f32, bool)>,
}

impl Feed {
    pub fn push(&mut self, text: String, error: bool) {
        self.items.push_front((text, 0.0, error));
        self.items.truncate(5);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Category {
    Roads,
    Roundabout,
    Zoning,
    Bulldoze,
    InfoViews,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Speed(u8),
    Category(Category),
    Road(usize),
    Curved(bool),
    Roundabout(usize),
    Zone(ZoneType),
    Overlay(Option<Overlay>),
    ToggleBudget,
    /// Taux (0 : habitants, 1 : entreprises), variation en points.
    Tax(usize, i8),
    CloseInfo,
}

#[derive(Component)]
struct HudButton {
    action: Action,
    hovered: bool,
}

/// Panneau qui intercepte la souris.
#[derive(Component)]
pub(crate) struct Blocker;

#[derive(Component)]
struct SubPanel(Category);

#[derive(Component, Clone, Copy)]
enum Stat {
    Population,
    Funds,
    Date,
}

#[derive(Component)]
struct DemandFill(usize);

#[derive(Component)]
struct Tooltip;

#[derive(Component)]
struct FeedLine(usize);

#[derive(Component)]
struct GhostMesh;

/// Police de l'interface, chargée au démarrage (`screens`).
#[derive(Resource, Debug)]
pub(crate) struct HudFont(pub(crate) Handle<Font>);

#[derive(Resource)]
struct GhostMaterials {
    valid: Handle<StandardMaterial>,
    invalid: Handle<StandardMaterial>,
    zones: [Handle<StandardMaterial>; 4],
    bulldoze: Handle<StandardMaterial>,
}

fn font(font: &HudFont, size: f32, weight: u16) -> TextFont {
    TextFont {
        font: FontSource::Handle(font.0.clone()),
        font_size: FontSize::Px(size),
        weight: FontWeight(weight),
        ..default()
    }
}

fn ghost_material(materials: &mut Assets<StandardMaterial>, rgb: [f32; 3]) -> Handle<StandardMaterial> {
    materials.add(StandardMaterial {
        base_color: Color::srgba(rgb[0], rgb[1], rgb[2], 0.5),
        emissive: LinearRgba::rgb(rgb[0] * 0.4, rgb[1] * 0.4, rgb[2] * 0.4),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None,
        // Le fantôme passe devant les arbres et les toits : on doit voir ce
        // qu'on construit, même sous la canopée.
        depth_bias: 1.0e9,
        ..default()
    })
}

#[allow(clippy::too_many_lines)]
fn setup_hud(
    mut commands: Commands,
    hud_font: Res<HudFont>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    texts: Res<Texts>,
) {
    let hud_font = HudFont(hud_font.0.clone());
    let t = |key: &str| texts.text(key);

    // --- fantômes de construction
    commands.insert_resource(GhostMaterials {
        valid: ghost_material(&mut materials, [0.3, 0.95, 0.5]),
        invalid: ghost_material(&mut materials, [1.0, 0.3, 0.25]),
        zones: [
            ghost_material(&mut materials, [0.7, 0.7, 0.7]),
            ghost_material(&mut materials, [0.35, 0.9, 0.35]),
            ghost_material(&mut materials, [0.3, 0.6, 1.0]),
            ghost_material(&mut materials, [1.0, 0.8, 0.2]),
        ],
        bulldoze: ghost_material(&mut materials, [1.0, 0.25, 0.2]),
    });
    commands.spawn((
        GhostMesh,
        Mesh3d(meshes.add(Mesh::new(bevy::mesh::PrimitiveTopology::TriangleList, RenderAssetUsages::default()))),
        MeshMaterial3d(materials.add(StandardMaterial::default())),
        Transform::default(),
        Visibility::Hidden,
        NoFrustumCulling,
    ));

    // --- barre du haut : statistiques, demande, vitesse
    let stat_block = |commands: &mut ChildSpawnerCommands, label: String, stat: Stat| {
        commands
            .spawn(Node { flex_direction: FlexDirection::Column, margin: UiRect::horizontal(Val::Px(14.0)), ..default() })
            .with_children(|c| {
                c.spawn((Text::new(label), font(&hud_font, 11.0, 500), TextColor(TEXT_DIM)));
                c.spawn((Text::new("—"), font(&hud_font, 17.0, 600), TextColor(TEXT), stat));
            });
    };
    commands
        .spawn((
            Blocker,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Px(54.0),
                padding: UiRect::axes(Val::Px(18.0), Val::Px(6.0)),
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(PANEL),
        ))
        .with_children(|bar| {
            bar.spawn((Text::new(t("app-title")), font(&hud_font, 21.0, 700), TextColor(TEXT)));
            bar.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, ..default() })
                .with_children(|stats| {
                    stat_block(stats, t("hud-population"), Stat::Population);
                    stat_block(stats, t("hud-funds"), Stat::Funds);
                    stat_block(stats, t("hud-date"), Stat::Date);
                    // Demande R / C / I : trois jauges verticales.
                    stats
                        .spawn(Node { flex_direction: FlexDirection::Column, margin: UiRect::horizontal(Val::Px(14.0)), ..default() })
                        .with_children(|d| {
                            d.spawn((Text::new(t("hud-demand")), font(&hud_font, 11.0, 500), TextColor(TEXT_DIM)));
                            d.spawn(Node { flex_direction: FlexDirection::Row, column_gap: Val::Px(6.0), align_items: AlignItems::End, ..default() })
                                .with_children(|row| {
                                    let colors = [Color::srgb(0.35, 0.85, 0.4), Color::srgb(0.35, 0.6, 1.0), Color::srgb(1.0, 0.78, 0.25)];
                                    let labels = ["hud-demand-residential", "hud-demand-commercial", "hud-demand-industrial"];
                                    for i in 0..3 {
                                        row.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::End, column_gap: Val::Px(3.0), ..default() })
                                            .with_children(|g| {
                                                g.spawn((Text::new(t(labels[i])), font(&hud_font, 11.0, 600), TextColor(TEXT_DIM)));
                                                g.spawn((
                                                    Node { width: Val::Px(9.0), height: Val::Px(22.0), align_items: AlignItems::End, border_radius: BorderRadius::all(Val::Px(2.0)), ..default() },
                                                    BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.12)),
                                                ))
                                                .with_children(|bar| {
                                                    bar.spawn((
                                                        DemandFill(i),
                                                        Node { width: Val::Percent(100.0), height: Val::Percent(0.0), border_radius: BorderRadius::all(Val::Px(2.0)), ..default() },
                                                        BackgroundColor(colors[i]),
                                                    ));
                                                });
                                            });
                                    }
                                });
                        });
                });
            bar.spawn(Node { flex_direction: FlexDirection::Row, column_gap: Val::Px(6.0), ..default() }).with_children(|speeds| {
                spawn_button(speeds, &hud_font, t("hud-budget"), Action::ToggleBudget, 90.0);
                speeds.spawn(Node { width: Val::Px(14.0), ..default() });
                for (speed, key) in [(0u8, "hud-speed-label-pause"), (1, "hud-speed-label-normal"), (2, "hud-speed-label-fast"), (3, "hud-speed-label-fastest")] {
                    spawn_button(speeds, &hud_font, t(key), Action::Speed(speed), 44.0);
                }
            });
        });

    // --- sous-menus (au-dessus de la barre d'outils)
    let sub = |commands: &mut Commands, category: Category, build: &dyn Fn(&mut ChildSpawnerCommands)| {
        commands
            .spawn(Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(92.0),
                left: Val::Px(0.0),
                width: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                ..default()
            })
            .with_children(|wrap| {
                wrap.spawn((
                    Blocker,
                    SubPanel(category),
                    Node {
                        display: Display::None,
                        flex_direction: FlexDirection::Row,
                        flex_wrap: FlexWrap::Wrap,
                        justify_content: JustifyContent::Center,
                        max_width: Val::Px(1180.0),
                        padding: UiRect::all(Val::Px(8.0)),
                        column_gap: Val::Px(6.0),
                        row_gap: Val::Px(6.0),
                        border_radius: BorderRadius::all(Val::Px(12.0)),
                        ..default()
                    },
                    BackgroundColor(PANEL),
                ))
                .with_children(|panel| build(panel));
            });
    };
    sub(&mut commands, Category::Roads, &|p| {
        spawn_button(p, &hud_font, t("tool-straight"), Action::Curved(false), 0.0);
        spawn_button(p, &hud_font, t("tool-curved"), Action::Curved(true), 0.0);
        for (i, (key, _)) in RoadSpec::CATALOGUE.iter().enumerate() {
            spawn_button(p, &hud_font, t(key), Action::Road(i), 0.0);
        }
    });
    sub(&mut commands, Category::Roundabout, &|p| {
        for (i, key) in ["tool-roundabout-small", "tool-roundabout-medium", "tool-roundabout-large"].iter().enumerate() {
            spawn_button(p, &hud_font, t(key), Action::Roundabout(i), 0.0);
        }
    });
    sub(&mut commands, Category::InfoViews, &|p| {
        for overlay in Overlay::ALL.into_iter().filter(|o| o.is_implemented()) {
            spawn_button(p, &hud_font, t(overlay.message_key()), Action::Overlay(Some(overlay)), 0.0);
        }
        spawn_button(p, &hud_font, t("overlay-off"), Action::Overlay(None), 0.0);
    });
    sub(&mut commands, Category::Zoning, &|p| {
        for zone in [ZoneType::Residential, ZoneType::Commercial, ZoneType::Industrial, ZoneType::Unzoned] {
            spawn_button(p, &hud_font, t(zone.message_key()), Action::Zone(zone), 0.0);
        }
    });

    // --- barre d'outils
    commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(18.0),
            left: Val::Px(0.0),
            width: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            ..default()
        })
        .with_children(|wrap| {
            wrap.spawn((
                Blocker,
                Node {
                    flex_direction: FlexDirection::Row,
                    padding: UiRect::all(Val::Px(8.0)),
                    column_gap: Val::Px(8.0),
                    border_radius: BorderRadius::all(Val::Px(14.0)),
                    ..default()
                },
                BackgroundColor(PANEL),
            ))
            .with_children(|bar| {
                for (category, key) in [
                    (Category::Roads, "tool-roads"),
                    (Category::Roundabout, "tool-roundabout"),
                    (Category::Zoning, "tool-zoning"),
                    (Category::Bulldoze, "tool-bulldoze"),
                    (Category::InfoViews, "tool-info-views"),
                ] {
                    spawn_button(bar, &hud_font, t(key), Action::Category(category), 118.0);
                }
            });
        });

    // --- aide, infobulle, fil d'actualité
    commands.spawn((
        Text::new(t("hud-help")),
        font(&hud_font, 12.0, 400),
        TextColor(TEXT_DIM),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(12.0),
            bottom: Val::Px(8.0),
            padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
            border_radius: BorderRadius::all(Val::Px(6.0)),
            ..default()
        },
        BackgroundColor(PANEL),
    ));
    commands.spawn((
        Tooltip,
        Text::new(""),
        font(&hud_font, 13.0, 500),
        TextColor(TEXT),
        Node {
            position_type: PositionType::Absolute,
            padding: UiRect::axes(Val::Px(8.0), Val::Px(5.0)),
            border_radius: BorderRadius::all(Val::Px(6.0)),
            display: Display::None,
            ..default()
        },
        BackgroundColor(PANEL),
    ));
    commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            top: Val::Px(64.0),
            right: Val::Px(14.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::End,
            row_gap: Val::Px(4.0),
            ..default()
        })
        .with_children(|feed| {
            for i in 0..5 {
                feed.spawn((
                    FeedLine(i),
                    Text::new(""),
                    font(&hud_font, 13.0, 500),
                    TextColor(TEXT),
                    Node { padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)), border_radius: BorderRadius::all(Val::Px(6.0)), display: Display::None, ..default() },
                    BackgroundColor(PANEL),
                ));
            }
        });
    // --- panneaux : budget (à gauche), infos (à droite), légende
    spawn_panel(&mut commands, &hud_font, PanelKind::Budget, |panel| {
        for (i, key) in ["budget-tax-residential", "budget-tax-business"].iter().enumerate() {
            panel
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    justify_content: JustifyContent::SpaceBetween,
                    align_items: AlignItems::Center,
                    margin: UiRect::top(Val::Px(6.0)),
                    ..default()
                })
                .with_children(|row| {
                    row.spawn((Text::new(t(key)), font(&hud_font, 13.0, 500), TextColor(TEXT)));
                    row.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: Val::Px(6.0), ..default() })
                        .with_children(|ctl| {
                            spawn_button(ctl, &hud_font, "−".into(), Action::Tax(i, -1), 30.0);
                            ctl.spawn((TaxValue(i), Text::new(""), font(&hud_font, 14.0, 700), TextColor(TEXT), Node { min_width: Val::Px(38.0), justify_content: JustifyContent::Center, ..default() }));
                            spawn_button(ctl, &hud_font, "+".into(), Action::Tax(i, 1), 30.0);
                        });
                });
        }
        panel.spawn((
            Text::new(t("budget-tax-hint")),
            font(&hud_font, 11.0, 400),
            TextColor(TEXT_DIM),
            Node { margin: UiRect::top(Val::Px(8.0)), max_width: Val::Px(300.0), ..default() },
        ));
    });
    spawn_panel(&mut commands, &hud_font, PanelKind::Info, |_| {});
    commands
        .spawn((
            Legend,
            Blocker,
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(14.0),
                bottom: Val::Px(92.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(10.0)),
                row_gap: Val::Px(6.0),
                border_radius: BorderRadius::all(Val::Px(10.0)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(PANEL),
        ))
        .with_children(|legend| {
            legend.spawn((LegendText(0), Text::new(""), font(&hud_font, 14.0, 700), TextColor(TEXT)));
            legend.spawn(Node { flex_direction: FlexDirection::Row, column_gap: Val::Px(2.0), ..default() }).with_children(|row| {
                for i in 0..5 {
                    row.spawn((LegendSwatch(i), Node { width: Val::Px(34.0), height: Val::Px(10.0), ..default() }, BackgroundColor(Color::NONE)));
                }
            });
            legend.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, column_gap: Val::Px(30.0), ..default() }).with_children(|row| {
                row.spawn((LegendText(1), Text::new(""), font(&hud_font, 11.0, 500), TextColor(TEXT_DIM)));
                row.spawn((LegendText(2), Text::new(""), font(&hud_font, 11.0, 500), TextColor(TEXT_DIM)));
            });
        });
    commands.spawn((
        OverlayMesh,
        Mesh3d(meshes.add(Mesh::new(bevy::mesh::PrimitiveTopology::TriangleList, RenderAssetUsages::default()))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::WHITE,
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            cull_mode: None,
            ..default()
        })),
        Transform::default(),
        Visibility::Hidden,
        NoFrustumCulling,
    ));
    drop(hud_font);
}

/// Panneau à titre et lignes « libellé — valeur » (+ contenu propre).
fn spawn_panel(commands: &mut Commands, hud_font: &HudFont, kind: PanelKind, extra: impl FnOnce(&mut ChildSpawnerCommands)) {
    let (side, top) = match kind {
        PanelKind::Budget => (UiRect { left: Val::Px(14.0), right: Val::Auto, ..default() }, 64.0),
        PanelKind::Info => (UiRect { left: Val::Auto, right: Val::Px(14.0), ..default() }, 230.0),
    };
    commands
        .spawn((
            PanelRoot(kind),
            Blocker,
            Node {
                position_type: PositionType::Absolute,
                left: side.left,
                right: side.right,
                top: Val::Px(top),
                width: Val::Px(330.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(14.0)),
                row_gap: Val::Px(3.0),
                border_radius: BorderRadius::all(Val::Px(12.0)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(PANEL),
        ))
        .with_children(|panel| {
            panel
                .spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, margin: UiRect::bottom(Val::Px(6.0)), ..default() })
                .with_children(|head| {
                    head.spawn((PanelTitle(kind), Text::new(""), font(hud_font, 17.0, 700), TextColor(TEXT)));
                    let close = if kind == PanelKind::Budget { Action::ToggleBudget } else { Action::CloseInfo };
                    spawn_button(head, hud_font, "×".into(), close, 30.0);
                });
            for i in 0..PANEL_ROWS {
                panel
                    .spawn((PanelRow(kind, i), Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, display: Display::None, ..default() }))
                    .with_children(|row| {
                        row.spawn((RowLabel(kind, i), Text::new(""), font(hud_font, 13.0, 500), TextColor(TEXT_DIM)));
                        row.spawn((RowValue(kind, i), Text::new(""), font(hud_font, 13.0, 600), TextColor(TEXT)));
                    });
            }
            extra(panel);
        });
}

fn spawn_button(parent: &mut ChildSpawnerCommands, hud_font: &HudFont, label: String, action: Action, min_width: f32) {
    parent
        .spawn((
            HudButton { action, hovered: false },
            Node {
                min_width: Val::Px(min_width),
                height: Val::Px(34.0),
                padding: UiRect::horizontal(Val::Px(12.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(Val::Px(8.0)),
                ..default()
            },
            BackgroundColor(BUTTON),
        ))
        .with_children(|b| {
            b.spawn((Text::new(label), font(hud_font, 14.0, 600), TextColor(TEXT)));
        });
}

/// Curseur en pixels physiques de la fenêtre.
fn physical_cursor(windows: &Query<&Window>) -> Option<Vec2> {
    windows.iter().next().and_then(Window::physical_cursor_position)
}

fn hud_hover(
    windows: Query<&Window>,
    blockers: Query<(&ComputedNode, &UiGlobalTransform, &Node), With<Blocker>>,
    mut buttons: Query<(&mut HudButton, &ComputedNode, &UiGlobalTransform)>,
    mut over: ResMut<PointerOverHud>,
) {
    let cursor = physical_cursor(&windows);
    over.0 = cursor.is_some_and(|c| {
        blockers.iter().any(|(node, transform, style)| style.display != Display::None && node.contains_point(*transform, c))
    });
    for (mut button, node, transform) in &mut buttons {
        let hovered = cursor.is_some_and(|c| node.contains_point(*transform, c)) && node.size().x > 0.0;
        if button.hovered != hovered {
            button.hovered = hovered;
        }
    }
}

fn is_active(action: Action, tools: &Tools, sim: &Sim) -> bool {
    match action {
        Action::Speed(s) => sim.speed == s,
        Action::Category(c) => tools.open == Some(c) || (c == Category::Bulldoze && tools.controller.tool == Tool::Bulldoze),
        Action::Road(i) => matches!(tools.controller.tool, Tool::Road { .. }) && tools.road == i,
        Action::Curved(c) => matches!(tools.controller.tool, Tool::Road { .. }) && tools.curved == c,
        Action::Roundabout(i) => matches!(tools.controller.tool, Tool::Roundabout { .. }) && tools.roundabout == i,
        Action::Zone(z) => tools.controller.tool == Tool::Zone(z),
        Action::Overlay(o) => tools.overlay == o,
        Action::ToggleBudget => tools.budget_open,
        Action::Tax(..) | Action::CloseInfo => false,
    }
}

fn apply(action: Action, tools: &mut Tools, sim: &mut Sim) {
    match action {
        Action::Speed(s) => sim.speed = s,
        Action::Category(c) => {
            if tools.open == Some(c) || (c == Category::Bulldoze && tools.controller.tool == Tool::Bulldoze) {
                tools.open = None;
                tools.controller.set_tool(Tool::Select);
                return;
            }
            tools.open = (c != Category::Bulldoze).then_some(c);
            let tool = match c {
                Category::InfoViews => {
                    if tools.overlay.is_none() {
                        tools.overlay = Some(Overlay::LandValue);
                    }
                    Tool::Select
                }
                Category::Roads => road_tool(tools),
                Category::Roundabout => Tool::Roundabout { radius_m: ROUNDABOUT_RADII_M[tools.roundabout] },
                Category::Zoning => Tool::Zone(ZoneType::Residential),
                Category::Bulldoze => Tool::Bulldoze,
            };
            tools.controller.set_tool(tool);
        }
        Action::Road(i) => {
            tools.road = i;
            let tool = road_tool(tools);
            tools.controller.set_tool(tool);
        }
        Action::Curved(c) => {
            tools.curved = c;
            let tool = road_tool(tools);
            tools.controller.set_tool(tool);
        }
        Action::Roundabout(i) => {
            tools.roundabout = i;
            tools.controller.set_tool(Tool::Roundabout { radius_m: ROUNDABOUT_RADII_M[i] });
        }
        Action::Zone(z) => tools.controller.set_tool(Tool::Zone(z)),
        Action::Overlay(o) => tools.overlay = o,
        Action::ToggleBudget => tools.budget_open = !tools.budget_open,
        Action::Tax(index, delta) => {
            let mut rates = sim.world.treasury().tax_percent;
            rates[index] = rates[index].saturating_add_signed(delta).min(sim_core::MAX_TAX_PERCENT);
            sim.world.queue(Command::SetTaxes { residential_percent: rates[0], business_percent: rates[1] });
        }
        Action::CloseInfo => tools.selected = None,
    }
}

fn road_tool(tools: &Tools) -> Tool {
    Tool::Road { spec: RoadSpec::CATALOGUE[tools.road].1, curved: tools.curved }
}

fn hud_buttons(
    mouse: Res<ButtonInput<MouseButton>>,
    mut tools: ResMut<Tools>,
    mut sim: ResMut<Sim>,
    mut buttons: Query<(&HudButton, &mut BackgroundColor)>,
    mut panels: Query<(&SubPanel, &mut Node)>,
) {
    let clicked = mouse.just_released(MouseButton::Left);
    for (button, _) in &buttons {
        if clicked && button.hovered {
            apply(button.action, &mut tools, &mut sim);
            break;
        }
    }
    for (button, mut color) in &mut buttons {
        let wanted = if is_active(button.action, &tools, &sim) {
            BUTTON_ACTIVE
        } else if button.hovered {
            BUTTON_HOVER
        } else {
            BUTTON
        };
        if color.0 != wanted {
            color.0 = wanted;
        }
    }
    for (panel, mut node) in &mut panels {
        let display = if tools.open == Some(panel.0) { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
}

fn hud_keys(keys: Res<ButtonInput<KeyCode>>, mut sim: ResMut<Sim>, mut tools: ResMut<Tools>, mut pause: ResMut<crate::screens::PauseMenu>) {
    // Échap : d'abord annuler le geste, l'outil, le sous-menu ; sinon pause.
    if keys.just_pressed(KeyCode::Escape) {
        let busy = pause.open || tools.controller.tool != Tool::Select || tools.open.is_some() || tools.selected.is_some();
        if pause.open || !busy {
            pause.toggle_requested = true;
            return;
        }
        if tools.selected.is_some() && tools.controller.tool == Tool::Select && tools.open.is_none() {
            tools.selected = None;
            return;
        }
    }
    if pause.open {
        return;
    }
    if keys.just_pressed(KeyCode::Space) {
        sim.speed = if sim.speed == 0 { sim.resume_speed.max(1) } else { 0 };
    }
    for (key, speed) in [(KeyCode::Digit1, 1u8), (KeyCode::Digit2, 2), (KeyCode::Digit3, 3)] {
        if keys.just_pressed(key) {
            sim.speed = speed;
        }
    }
    if sim.speed > 0 {
        sim.resume_speed = sim.speed;
    }
    if keys.just_pressed(KeyCode::Escape) && !tools.controller.cancel() {
        tools.open = None;
    }
    if tools.controller.tool == Tool::Select && keys.just_pressed(KeyCode::Escape) {
        tools.open = None;
    }
}

/// Point du sol sous le curseur, dans la carte jouable : on borne le rayon à
/// l'emprise de la carte (hors carte, le relief procédural est coûteux et les
/// outils n'y construisent pas), on avance par pas croissants, puis on affine
/// par dichotomie.
fn ground_under_cursor(world: &World, ray: Ray3d) -> Option<[f32; 2]> {
    let (w, h) = crate::nature::play_size_m(world);
    let (o, d) = (ray.origin, *ray.direction);
    // Intervalle du rayon au-dessus du rectangle [0, w] × [0, h].
    let mut range = (0.0f32, 20_000.0f32);
    for (origin, dir, max) in [(o.x, d.x, w), (o.z, d.z, h)] {
        if dir.abs() < 1e-6 {
            if origin < 0.0 || origin > max {
                return None;
            }
            continue;
        }
        let (a, b) = ((0.0 - origin) / dir, (max - origin) / dir);
        range = (range.0.max(a.min(b)), range.1.min(a.max(b)));
    }
    if range.0 >= range.1 {
        return None;
    }
    let height = |t: f32| {
        let p = ray.get_point(t);
        p.y - ground_altitude_m(world, p.x, p.z)
    };
    let (mut t0, mut t) = (range.0, range.0);
    while t <= range.1 {
        if height(t) <= 0.0 {
            let mut t1 = t;
            for _ in 0..20 {
                let mid = (t0 + t1) * 0.5;
                if height(mid) > 0.0 { t0 = mid } else { t1 = mid }
            }
            let p = ray.get_point(t1);
            return Some([p.x, p.z]);
        }
        t0 = t;
        t += (t * 0.01).max(1.0);
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn tool_input(
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mouse: Res<ButtonInput<MouseButton>>,
    over: Res<PointerOverHud>,
    mut sim: ResMut<Sim>,
    mut tools: ResMut<Tools>,
    mut right_drag: Local<f32>,
    motion: Res<bevy::input::mouse::AccumulatedMouseMotion>,
    test: Res<ToolTest>,
) {
    if test.active {
        return;
    }
    let cursor = windows.iter().next().and_then(Window::cursor_position);
    if tools.controller.tool == Tool::Select {
        tools.cursor = None;
        tools.ghost = None;
        // Sans outil : un clic (sans glisser) inspecte un bâtiment ou une route.
        if mouse.just_pressed(MouseButton::Left) && !over.0 {
            tools.press_at = cursor;
        }
        if mouse.just_released(MouseButton::Left) {
            let pressed = tools.press_at.take();
            if let (Some(from), Some(to)) = (pressed, cursor) {
                if from.distance(to) < 5.0 && !over.0 {
                    let ground = cameras.iter().next().and_then(|(camera, transform)| {
                        camera.viewport_to_world(transform, to).ok().and_then(|ray| ground_under_cursor(&sim.world, ray))
                    });
                    tools.selected = ground.and_then(|g| pick(&sim.world, g));
                }
            }
        }
        return;
    }
    let ground = cursor.and_then(|c| {
        let (camera, transform) = cameras.iter().next()?;
        let ray = camera.viewport_to_world(transform, c).ok()?;
        ground_under_cursor(&sim.world, ray)
    });
    let tools = &mut *tools;
    tools.cursor = if over.0 { None } else { ground };
    if tools.controller.tool == Tool::Select {
        tools.ghost = None;
        return;
    }
    // Clic droit sans glisser : annuler (le glisser tourne la caméra).
    if mouse.pressed(MouseButton::Right) {
        *right_drag += motion.delta.length();
    }
    if mouse.just_released(MouseButton::Right) {
        if *right_drag < 6.0 && !tools.controller.cancel() {
            tools.open = None;
        }
        if tools.controller.tool == Tool::Select {
            tools.open = None;
        }
        *right_drag = 0.0;
    }
    if over.0 {
        tools.ghost = None;
        return;
    }
    if mouse.just_pressed(MouseButton::Left) {
        tools.controller.press(ground);
    }
    if mouse.just_released(MouseButton::Left) {
        let world = &sim.world;
        let commands = tools.controller.release(world, ground);
        for command in commands {
            sim.world.queue(command);
        }
    }
    tools.ghost = Some(tools.controller.ghost(&sim.world, ground));
}

/// Bâtiment sur la tuile visée, sinon route sous le point.
fn pick(world: &World, ground: [f32; 2]) -> Option<Selected> {
    let size = world.config().tile_size_m as f32;
    let tile = world.zoning().grid().tile_at((ground[0] / size) as u32, (ground[1] / size) as u32);
    if let Some(tile) = tile.filter(|t| world.buildings().at(*t).is_some()) {
        return Some(Selected::Building(tile));
    }
    city_ui::tools::road_under(world, ground).map(Selected::Road)
}

/// Contenu d'un panneau : titre et lignes (libellé, valeur).
fn panel_content(kind: PanelKind, tools: &Tools, world: &World, texts: &Texts) -> Option<(String, Vec<(String, String)>)> {
    let pct = |v: f32| texts.format("info-percent", &[("percent", Arg::from((v * 100.0).round()))]);
    match kind {
        PanelKind::Budget => {
            if !tools.budget_open {
                return None;
            }
            let estimate = world.budget_estimate();
            let signed = |cents: i64| {
                let sign = if cents < 0 { "−" } else { "+" };
                texts.format("money-signed", &[("sign", Arg::from(sign)), ("value", Arg::from(text::number(texts, cents.abs() / 100)))])
            };
            let last = world.treasury().last_breakdown;
            Some((
                texts.text("budget-title"),
                vec![
                    (texts.text("budget-funds"), text::money(texts, world.treasury().funds_cents)),
                    (texts.text("budget-this-month"), String::new()),
                    (texts.text("budget-residential-tax"), signed(estimate.residential_tax_cents)),
                    (texts.text("budget-business-tax"), signed(estimate.business_tax_cents)),
                    (texts.text("budget-road-upkeep"), signed(-estimate.road_upkeep_cents)),
                    (texts.text("budget-building-upkeep"), signed(-estimate.building_upkeep_cents)),
                    (texts.text("budget-net"), signed(estimate.net_cents())),
                    (texts.text("budget-last-month"), signed(last.net_cents())),
                ],
            ))
        }
        PanelKind::Info => match tools.selected? {
            Selected::Building(tile) => {
                let id = world.buildings().at(tile)?;
                let b = world.buildings().get(id)?;
                let (stage, _) = b.stage(world.current_tick());
                let progress = b.construction_progress(world.current_tick());
                let land = world.overlay_cell(Overlay::LandValue, tile).map_or(0.0, |v| v.to_f32());
                let mut rows = vec![
                    (texts.text("info-type"), text::building_kind(texts, b.kind)),
                    (texts.text("info-status"), text::construction(texts, stage, progress)),
                    (texts.text("info-capacity"), text::number(texts, i64::from(b.capacity))),
                ];
                if b.is_household() {
                    rows.push((texts.text("info-residents"), text::number(texts, i64::from(b.occupants))));
                } else {
                    rows.push((texts.text("info-jobs"), text::number(texts, i64::from(if b.complete { b.capacity } else { 0 }))));
                }
                rows.push((texts.text("info-land-value"), pct(land)));
                Some((texts.text("info-building"), rows))
            }
            Selected::Road(segment) => {
                let seg = world.roads().segment(segment)?;
                let now = world.current_tick();
                let status = if seg.is_open(now) {
                    texts.text("info-open")
                } else {
                    texts.format("road-under-construction", &[("percent", Arg::from((seg.construction_progress(now).to_f32() * 100.0).round()))])
                };
                Some((
                    text::road_type(texts, seg.spec),
                    vec![
                        (texts.text("info-status"), status),
                        (texts.text("info-length"), texts.format("unit-meters", &[("value", Arg::from(seg.length_m.to_f32().round()))])),
                        (texts.text("info-speed"), texts.format("road-speed-limit", &[("speed", Arg::from(seg.spec.speed_kph))])),
                        (
                            texts.text("info-lanes"),
                            texts.format("info-lanes-value", &[("forward", Arg::from(seg.spec.forward_lanes)), ("backward", Arg::from(seg.spec.backward_lanes))]),
                        ),
                    ],
                ))
            }
        },
    }
}

/// Couleurs de la légende et du sol pour une vue : (bas → haut).
fn overlay_ramp(overlay: Overlay) -> [[f32; 4]; 5] {
    match overlay {
        // Zonage : non zoné (transparent), R, C, I.
        Overlay::Zone => [
            [0.0, 0.0, 0.0, 0.0],
            [0.3, 0.85, 0.35, 0.5],
            [0.3, 0.55, 1.0, 0.5],
            [1.0, 0.78, 0.2, 0.5],
            [0.0, 0.0, 0.0, 0.0],
        ],
        Overlay::RoadAccess => [
            [0.95, 0.3, 0.25, 0.35],
            [0.95, 0.3, 0.25, 0.35],
            [0.95, 0.3, 0.25, 0.35],
            [0.3, 0.9, 0.4, 0.45],
            [0.3, 0.9, 0.4, 0.45],
        ],
        _ => [
            [0.85, 0.15, 0.12, 0.5],
            [0.95, 0.5, 0.15, 0.5],
            [0.98, 0.85, 0.2, 0.5],
            [0.55, 0.85, 0.25, 0.5],
            [0.15, 0.7, 0.3, 0.5],
        ],
    }
}

/// Couleur d'une tuile pour une valeur de vue `[0, 1]`.
fn overlay_color(overlay: Overlay, value: f32) -> [f32; 4] {
    let ramp = overlay_ramp(overlay);
    match overlay {
        Overlay::Zone => ramp[((value * 3.0).round() as usize).min(3)],
        Overlay::RoadAccess => if value > 0.5 { ramp[4] } else { ramp[0] },
        _ => {
            let t = value.clamp(0.0, 1.0) * 4.0;
            let (i, f) = ((t.floor() as usize).min(3), t - t.floor().min(3.0));
            let (a, b) = (ramp[i], ramp[i + 1]);
            [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f, a[3] + (b[3] - a[3]) * f]
        }
    }
}

// Requêtes Bevy disjointes (`Without<…>`) : leur type est long par nature.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_panels(
    tools: Res<Tools>,
    sim: Res<Sim>,
    texts: Res<Texts>,
    mut roots: Query<(&PanelRoot, &mut Node), (Without<PanelRow>, Without<Legend>)>,
    mut titles: Query<(&PanelTitle, &mut Text), (Without<RowLabel>, Without<RowValue>, Without<TaxValue>, Without<LegendText>)>,
    mut rows: Query<(&PanelRow, &mut Node), (Without<PanelRoot>, Without<Legend>)>,
    mut labels: Query<(&RowLabel, &mut Text), (Without<PanelTitle>, Without<RowValue>, Without<TaxValue>, Without<LegendText>)>,
    mut values: Query<(&RowValue, &mut Text), (Without<PanelTitle>, Without<RowLabel>, Without<TaxValue>, Without<LegendText>)>,
    mut taxes: Query<(&TaxValue, &mut Text), (Without<PanelTitle>, Without<RowLabel>, Without<RowValue>, Without<LegendText>)>,
    mut legend: Query<&mut Node, (With<Legend>, Without<PanelRoot>, Without<PanelRow>)>,
    mut legend_texts: Query<(&LegendText, &mut Text), (Without<PanelTitle>, Without<RowLabel>, Without<RowValue>, Without<TaxValue>)>,
    mut swatches: Query<(&LegendSwatch, &mut BackgroundColor)>,
) {
    let world = &sim.world;
    for kind in [PanelKind::Budget, PanelKind::Info] {
        let content = panel_content(kind, &tools, world, &texts);
        for (root, mut node) in &mut roots {
            if root.0 == kind {
                let display = if content.is_some() { Display::Flex } else { Display::None };
                if node.display != display {
                    node.display = display;
                }
            }
        }
        let Some((title, lines)) = content else { continue };
        let set = |text: &mut Text, value: &str| {
            if text.0 != value {
                text.0 = value.to_owned();
            }
        };
        for (t, mut text) in &mut titles {
            if t.0 == kind {
                set(&mut text, &title);
            }
        }
        for (row, mut node) in &mut rows {
            if row.0 == kind {
                let display = if row.1 < lines.len() { Display::Flex } else { Display::None };
                if node.display != display {
                    node.display = display;
                }
            }
        }
        for (label, mut text) in &mut labels {
            if label.0 == kind {
                if let Some((l, _)) = lines.get(label.1) {
                    set(&mut text, l);
                }
            }
        }
        for (value, mut text) in &mut values {
            if value.0 == kind {
                if let Some((_, v)) = lines.get(value.1) {
                    set(&mut text, v);
                }
            }
        }
    }
    let rates = world.treasury().tax_percent;
    for (tax, mut text) in &mut taxes {
        let value = texts.format("budget-tax-value", &[("percent", Arg::from(rates[tax.0]))]);
        if text.0 != value {
            text.0 = value;
        }
    }
    // Légende de la vue d'information.
    let Ok(mut legend_node) = legend.single_mut() else { return };
    let Some(overlay) = tools.overlay else {
        legend_node.display = Display::None;
        return;
    };
    legend_node.display = Display::Flex;
    let (low, high) = match overlay {
        Overlay::RoadAccess => ("legend-not-served", "legend-served"),
        Overlay::Zone => ("legend-zones", "legend-empty"),
        _ => ("legend-low", "legend-high"),
    };
    for (lt, mut text) in &mut legend_texts {
        let value = match lt.0 {
            0 => texts.text(overlay.message_key()),
            1 => texts.text(low),
            _ => texts.text(high),
        };
        if text.0 != value {
            text.0 = value;
        }
    }
    let ramp = overlay_ramp(overlay);
    for (swatch, mut color) in &mut swatches {
        let c = ramp[swatch.0];
        // Zonage : trois cases R, C, I ; les autres vues : un dégradé.
        let wanted = if overlay == Overlay::Zone && (swatch.0 == 0 || swatch.0 == 4) {
            Color::NONE
        } else {
            Color::srgba(c[0], c[1], c[2], 1.0)
        };
        if color.0 != wanted {
            color.0 = wanted;
        }
    }
}

/// Vue d'information : une tuile colorée par case, posée sur le relief.
/// Reconstruite au changement de vue, puis toutes les deux secondes.
fn update_overlay(
    time: Res<Time>,
    tools: Res<Tools>,
    sim: Res<Sim>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut overlay: Query<(&Mesh3d, &mut Visibility), With<OverlayMesh>>,
    mut state: Local<(Option<Overlay>, f32)>,
) {
    let Ok((mesh, mut visibility)) = overlay.single_mut() else { return };
    state.1 += time.delta_secs();
    if state.0 == tools.overlay && state.1 < 2.0 {
        return;
    }
    state.0 = tools.overlay;
    state.1 = 0.0;
    let Some(kind) = tools.overlay else {
        *visibility = Visibility::Hidden;
        return;
    };
    let world = &sim.world;
    let grid = world.zoning().grid();
    let size = world.config().tile_size_m as f32;
    let (mut positions, mut colors, mut indices) = (Vec::new(), Vec::new(), Vec::new());
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let Some(tile) = grid.tile_at(x, y) else { continue };
            let Some(value) = world.overlay_cell(kind, tile) else { continue };
            let color = overlay_color(kind, value.to_f32());
            if color[3] <= 0.0 {
                continue;
            }
            let base = positions.len() as u32;
            for (cx, cz) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                let (px, pz) = ((x as f32 + cx) * size, (y as f32 + cz) * size);
                positions.push([px, ground_altitude_m(world, px, pz) + 0.7, pz]);
                colors.push(color);
            }
            indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
        }
    }
    let normals = vec![[0.0, 1.0, 0.0]; positions.len()];
    let mut data = Mesh::new(bevy::mesh::PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    data.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    data.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    data.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    data.insert_indices(bevy::mesh::Indices::U32(indices));
    if let Some(mut target) = meshes.get_mut(&mesh.0) {
        *target = data;
    }
    *visibility = Visibility::Visible;
}

/// Ruban de route fantôme le long d'une courbe (m), au-dessus du sol.
fn ghost_strip(world: &World, from: [f32; 2], to: [f32; 2], fh: [f32; 2], th: [f32; 2], spec: RoadSpec) -> RoadStrip {
    let c0 = [from[0] + fh[0], from[1] + fh[1]];
    let c1 = [to[0] - th[0], to[1] - th[1]];
    let points = (0..=24)
        .map(|k| {
            let t = k as f32 / 24.0;
            let u = 1.0 - t;
            let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
            let x = w[0] * from[0] + w[1] * c0[0] + w[2] * c1[0] + w[3] * to[0];
            let z = w[0] * from[1] + w[1] * c0[1] + w[2] * c1[1] + w[3] * to[1];
            [x, ground_altitude_m(world, x, z) + 0.9, z]
        })
        .collect();
    RoadStrip {
        points,
        half_width_m: roads::Profile::of(spec).half_width(),
        spec,
        nodes: [u32::MAX - 1, u32::MAX],
        progress: 1.0,
        ring_center: None,
    }
}

fn update_ghost(
    tools: Res<Tools>,
    sim: Res<Sim>,
    materials: Res<GhostMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut ghost: Query<(&Mesh3d, &mut MeshMaterial3d<StandardMaterial>, &mut Visibility), With<GhostMesh>>,
    mut last: Local<Option<Ghost>>,
) {
    let Ok((mesh, mut material, mut visibility)) = ghost.single_mut() else { return };
    if *last == tools.ghost {
        return;
    }
    last.clone_from(&tools.ghost);
    let world = &sim.world;
    let (data, handle) = match &tools.ghost {
        Some(Ghost::Road { from, to, from_handle, to_handle, spec, validity, .. }) => {
            let strip = ghost_strip(world, *from, *to, *from_handle, *to_handle, *spec);
            let handle = if validity.is_ok() { &materials.valid } else { &materials.invalid };
            (roads::road_mesh(world, &[strip]), handle.clone())
        }
        Some(Ghost::Control { from, control }) => {
            let strip = ghost_strip(world, *from, *control, [0.0, 0.0], [0.0, 0.0], RoadSpec::GRAVEL);
            (roads::road_mesh(world, &[strip]), materials.valid.clone())
        }
        Some(Ghost::Roundabout { center, radius_m, spec, validity }) => {
            let mut network = sim_core::RoadNetwork::new();
            let point = city_ui::tools::point(*center);
            let mut strips = Vec::new();
            if let Ok(segments) = network.add_roundabout(point, Fixed::from_int(*radius_m as i32), *spec) {
                for id in segments {
                    let Some(polyline) = network.segment_polyline(id, 8) else { continue };
                    let points = polyline
                        .iter()
                        .map(|p| {
                            let (x, z) = (p.x.to_f32(), p.y.to_f32());
                            [x, ground_altitude_m(world, x, z) + 0.9, z]
                        })
                        .collect();
                    strips.push(RoadStrip {
                        points,
                        half_width_m: roads::Profile::of(*spec).half_width(),
                        spec: *spec,
                        nodes: [u32::MAX - 1 - id.raw(), u32::MAX - 2 - id.raw()],
                        progress: 1.0,
                        ring_center: None,
                    });
                }
            }
            let handle = if validity.is_ok() { &materials.valid } else { &materials.invalid };
            (roads::road_mesh(world, &strips), handle.clone())
        }
        Some(Ghost::Area { rect, zone }) => {
            let mut data = urban::UrbanMesh::default();
            let size = world.config().tile_size_m as f32;
            let (x0, y0, x1, y1) = (rect.min_x, rect.min_y, rect.max_x.min(rect.min_x + 96), rect.max_y.min(rect.min_y + 96));
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let corner = |cx: f32, cz: f32| [cx, ground_altitude_m(world, cx, cz) + 0.5, cz];
                    let (ax, az) = (x as f32 * size + 0.4, y as f32 * size + 0.4);
                    let (bx, bz) = (ax + size - 0.8, az + size - 0.8);
                    let quad = [corner(ax, az), corner(bx, az), corner(bx, bz), corner(ax, bz)];
                    data.surface(&quad, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], urban::Layer::Marking, [1.0; 3], [0.0, 1.0, 0.0]);
                }
            }
            let handle = match zone {
                Some(z) => materials.zones[*z as usize].clone(),
                None => materials.bulldoze.clone(),
            };
            (data, handle)
        }
        Some(Ghost::RoadTarget { segment }) => {
            let strips: Vec<RoadStrip> = crate::scene::roads(world)
                .into_iter()
                .filter(|s| {
                    world.roads().segment(*segment).is_some_and(|seg| s.nodes == [seg.a.raw(), seg.b.raw()])
                })
                .map(|mut s| {
                    for p in &mut s.points {
                        p[1] += 0.6;
                    }
                    s.nodes = [u32::MAX - 1, u32::MAX];
                    s.ring_center = None;
                    s
                })
                .collect();
            (roads::road_mesh(world, &strips), materials.bulldoze.clone())
        }
        _ => (urban::UrbanMesh::default(), materials.valid.clone()),
    };
    if data.is_empty() {
        *visibility = Visibility::Hidden;
        return;
    }
    if let Some(mut target) = meshes.get_mut(&mesh.0) {
        *target = urban_mesh_public(data);
    }
    material.0 = handle;
    *visibility = Visibility::Visible;
}

fn update_stats(
    sim: Res<Sim>,
    texts: Res<Texts>,
    mut stats: Query<(&Stat, &mut Text)>,
    mut fills: Query<(&DemandFill, &mut Node)>,
) {
    let snapshot = sim.world.snapshot();
    let days_per_month = sim.world.config().days_per_month.max(1);
    for (stat, mut text) in &mut stats {
        let value = match stat {
            Stat::Population => text::inhabitants(&texts, snapshot.population),
            Stat::Funds => text::money(&texts, snapshot.funds_cents),
            // Le calendrier compte depuis 0 ; on affiche mois et jour du mois depuis 1.
            Stat::Date => texts.format(
                "hud-date-value",
                &[
                    ("month", Arg::from(snapshot.month + 1)),
                    ("day", Arg::from(snapshot.day % days_per_month + 1)),
                    ("hour", Arg::from(snapshot.hour)),
                ],
            ),
        };
        if text.0 != value {
            text.0 = value;
        }
    }
    let demand = [snapshot.demand.residential, snapshot.demand.commercial, snapshot.demand.industrial];
    for (fill, mut node) in &mut fills {
        let height = Val::Percent(demand[fill.0].to_f32().clamp(0.0, 1.0) * 100.0);
        if node.height != height {
            node.height = height;
        }
    }
}

fn update_feed(time: Res<Time>, mut feed: ResMut<Feed>, mut lines: Query<(&FeedLine, &mut Text, &mut TextColor, &mut Node)>) {
    let dt = time.delta_secs();
    for item in &mut feed.items {
        item.1 += dt;
    }
    feed.items.retain(|item| item.1 < 9.0);
    for (line, mut text, mut color, mut node) in &mut lines {
        match feed.items.get(line.0) {
            Some((message, age, error)) => {
                if text.0 != *message {
                    text.0.clone_from(message);
                }
                let fade = (9.0 - age).clamp(0.0, 1.0);
                let base = if *error { TEXT_ERROR } else { TEXT };
                color.0 = base.with_alpha(fade);
                node.display = Display::Flex;
            }
            None => node.display = Display::None,
        }
    }
}

fn update_tooltip(
    windows: Query<&Window>,
    tools: Res<Tools>,
    texts: Res<Texts>,
    over: Res<PointerOverHud>,
    mut tooltip: Query<(&mut Text, &mut TextColor, &mut Node), With<Tooltip>>,
) {
    let Ok((mut text, mut color, mut node)) = tooltip.single_mut() else { return };
    let cursor = windows.iter().next().and_then(Window::cursor_position);
    let (Some(cursor), false, false) = (cursor, over.0, tools.controller.tool == Tool::Select) else {
        node.display = Display::None;
        return;
    };
    let hint = texts.text(tools.controller.tool.hint_key(tools.controller.step()));
    let (message, error) = match &tools.ghost {
        Some(Ghost::Road { spec, length_m, validity, .. }) => {
            let details = texts.format(
                "tool-road-details",
                &[("name", Arg::from(text::road_type(&texts, *spec))), ("length", Arg::from(length_m.round()))],
            );
            match validity {
                Ok(()) => (format!("{details}\n{hint}"), false),
                Err(key) => (format!("{details}\n{}", texts.text(key)), true),
            }
        }
        Some(Ghost::Roundabout { validity: Err(key), .. }) => (texts.text(key), true),
        _ => (hint, false),
    };
    if text.0 != message {
        text.0 = message;
    }
    color.0 = if error { TEXT_ERROR } else { TEXT };
    // Près du bord droit ou bas, l'infobulle passe de l'autre côté du curseur.
    let (width, height) = windows.iter().next().map_or((f32::MAX, f32::MAX), |w| (w.width(), w.height()));
    node.left = Val::Px(if cursor.x > width - 320.0 { cursor.x - 300.0 } else { cursor.x + 18.0 });
    node.top = Val::Px(if cursor.y > height - 140.0 { cursor.y - 60.0 } else { cursor.y + 18.0 });
    node.display = Display::Flex;
}

// ------------------------------------------------------------ test scripté

/// Scénario de vérification des outils (`CITYMAYOR_TOOLTEST=1`) : la souris
/// est simulée (point au sol imposé, converti en pixels par la caméra), le
/// fantôme et les constructions sont capturés, puis on quitte. Vérifie aussi
/// la sélection au sol : pixel → sol doit retrouver le point visé.
#[derive(Resource, Debug, Default)]
pub struct ToolTest {
    pub active: bool,
    step: usize,
    elapsed: f32,
    shot: bool,
    gestured: bool,
    worst_pick_m: f32,
}

impl ToolTest {
    #[must_use]
    pub fn from_env() -> Self {
        Self { active: std::env::var_os("CITYMAYOR_TOOLTEST").is_some(), ..default() }
    }
}

/// Centre de la zone de test (m), au milieu du pâté de `scene::small_block`.
const TEST_X: f32 = 256.0;
const TEST_Z: f32 = 300.0;

/// Étapes : (nom de capture, outil, gestes `(appui?, relâché?, point)`).
/// Un geste scripté : bouton enfoncé ?, relâché ?, point au sol (m).
type Gesture = (bool, bool, [f32; 2]);

/// Panneaux ouverts à une étape du scénario.
#[derive(Clone, Copy, Debug, Default)]
struct TestPanels {
    budget: bool,
    overlay: Option<Overlay>,
    inspect: Option<[f32; 2]>,
}

fn tool_test_panels(step: usize) -> TestPanels {
    match step {
        6 => TestPanels { budget: true, ..default() },
        7 => TestPanels { overlay: Some(Overlay::LandValue), ..default() },
        8 => TestPanels { overlay: Some(Overlay::Zone), inspect: Some([TEST_X + 4.0, 180.0]), ..default() },
        _ => TestPanels::default(),
    }
}

fn tool_test_steps() -> Vec<(&'static str, Tool, Vec<Gesture>)> {
    let road = Tool::Road { spec: RoadSpec::TWO_WAY, curved: false };
    let curve = Tool::Road { spec: RoadSpec::BOULEVARD, curved: true };
    let p = |dx: f32, dz: f32| [TEST_X + dx, TEST_Z + dz];
    vec![
        ("1-route-fantome", road, vec![(true, true, p(-100.0, 0.0)), (false, false, p(100.0, 0.0))]),
        ("2-route-posee", road, vec![(true, true, p(100.0, 0.0))]),
        ("3-courbe-fantome", curve, vec![(true, true, p(-100.0, 70.0)), (true, true, p(0.0, 150.0)), (false, false, p(110.0, 70.0))]),
        ("4-zonage-fantome", Tool::Zone(ZoneType::Residential), vec![(true, false, p(-90.0, 14.0)), (false, false, p(80.0, 30.0))]),
        ("5-rond-point-fantome", Tool::Roundabout { radius_m: ROUNDABOUT_RADII_M[1] }, vec![(false, false, p(0.0, -70.0))]),
        ("6-demolition-fantome", Tool::Bulldoze, vec![(false, false, p(0.0, 1.0))]),
        ("7-budget", Tool::Select, vec![]),
        ("8-vue-valeur-fonciere", Tool::Select, vec![]),
        ("9-infos-batiment", Tool::Select, vec![]),
    ]
}

#[allow(clippy::too_many_arguments)]
fn tool_test(
    mut commands: Commands,
    time: Res<Time>,
    mut test: ResMut<ToolTest>,
    mut tools: ResMut<Tools>,
    mut sim: ResMut<Sim>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut orbit: Query<&mut crate::viewer::OrbitCamera>,
    mut exit: MessageWriter<AppExit>,
) {
    if !test.active {
        return;
    }
    let steps = tool_test_steps();
    let Some((name, tool, gestures)) = steps.get(test.step).cloned() else {
        println!("outils : sélection au sol, écart max {:.2} m", test.worst_pick_m);
        exit.write(AppExit::Success);
        return;
    };
    if test.elapsed == 0.0 {
        for mut camera in &mut orbit {
            camera.look_at(Vec3::new(TEST_X, 0.0, TEST_Z + 30.0), 0.35, 0.85, 300.0);
        }
        if tools.controller.tool != tool {
            tools.controller.set_tool(tool);
        }
        let panels = tool_test_panels(test.step);
        tools.budget_open = panels.budget;
        tools.overlay = panels.overlay;
        tools.selected = panels.inspect.and_then(|g| pick(&sim.world, g));
        if tool == Tool::Select {
            tools.ghost = None;
        }
    }
    test.elapsed += time.delta_secs();
    // Gestes une fois la caméra posée.
    if test.elapsed > 0.6 && !test.gestured {
        test.gestured = true;
        let mut last = None;
        for (press, release, at) in &gestures {
            // Sélection au sol : le point visé, vu à l'écran, doit se retrouver.
            if let Some((camera, transform)) = cameras.iter().next() {
                let ground = Vec3::new(at[0], ground_altitude_m(&sim.world, at[0], at[1]), at[1]);
                if let Ok(pixel) = camera.world_to_viewport(transform, ground) {
                    if let Some(found) = camera.viewport_to_world(transform, pixel).ok().and_then(|ray| ground_under_cursor(&sim.world, ray)) {
                        let error = ((found[0] - at[0]).powi(2) + (found[1] - at[1]).powi(2)).sqrt();
                        test.worst_pick_m = test.worst_pick_m.max(error);
                    }
                }
            }
            if *press {
                tools.controller.press(Some(*at));
            }
            if *release {
                let commands_out = tools.controller.release(&sim.world, Some(*at));
                for command in commands_out {
                    sim.world.queue(command);
                }
            }
            last = Some(*at);
        }
        tools.cursor = last;
        tools.ghost = Some(tools.controller.ghost(&sim.world, last));
    }
    if test.elapsed > 2.4 && !test.shot {
        test.shot = true;
        commands
            .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
            .observe(bevy::render::view::screenshot::save_to_disk(format!("citymayor-outils-{name}.png")));
    }
    if test.elapsed > 3.2 {
        test.step += 1;
        test.elapsed = 0.0;
        test.shot = false;
        test.gestured = false;
    }
}
