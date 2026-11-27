//! Écrans hors partie : démarrage du studio, menu principal (continuer,
//! nouvelle partie, charger, réglages, quitter), écran de chargement — non
//! passable — et menu pause.
//!
//! # Déroulé
//! `Splash` (logo Danux Game Studio, passable d'un clic) → `MainMenu` →
//! `Loading` (génération ou rejeu de la partie dans un fil d'exécution à
//! part, puis construction de la scène) → `InGame`. Depuis la partie, le
//! menu pause (Échap) sauvegarde et ramène au menu principal ; la scène est
//! alors entièrement démontée (`cleanup_game`).
//!
//! Tous les textes viennent de `locales/<langue>/menu.ftl`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy::text::{FontSize, FontSource, FontWeight};
use bevy::ui::{ComputedNode, UiGlobalTransform, UiScale};
use bevy::window::{MonitorSelection, WindowMode};

use city_ui::Arg;
use sim_core::SaveGame;

use crate::hud::{self, HudFont};
use crate::session::{self, Session, Settings, UI_SCALES};
use crate::viewer::{Sim, Texts};

const MENU_ART: &[u8] = include_bytes!("../assets/menu/menu.jpg");
const LOADING_ART: &[u8] = include_bytes!("../assets/menu/chargement.jpg");

const TEXT: Color = Color::srgb(0.96, 0.97, 0.99);
const TEXT_DIM: Color = Color::srgb(0.70, 0.74, 0.80);
const ACCENT: Color = Color::srgb(0.98, 0.72, 0.22);
const SHADE: Color = Color::srgba(0.03, 0.04, 0.06, 0.72);
const ROW_HOVER: Color = Color::srgba(1.0, 1.0, 1.0, 0.10);

/// Durée minimale de l'écran de chargement (s) : il ne clignote jamais.
const MIN_LOADING_S: f32 = 4.0;
/// Durée de l'écran du studio (s).
const SPLASH_S: f32 = 3.4;

/// État de l'application.
#[derive(States, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AppState {
    /// Chargement des polices et images (avant tout écran).
    #[default]
    Boot,
    Splash,
    MainMenu,
    Loading,
    InGame,
}

/// Ce que l'écran de chargement doit préparer.
#[derive(Resource, Clone, Debug)]
pub enum LoadRequest {
    New { scenario: String, ticks: u64 },
    Load(PathBuf),
}

/// Méta-données de la partie en cours (pour la sauvegarder).
#[derive(Resource, Clone, Debug)]
pub struct SessionMeta {
    pub scenario: String,
    pub skip: usize,
    pub name: String,
}

/// Marqueur : la scène 3D doit être construite (un passage de `setup`).
#[derive(Resource, Debug)]
pub struct BuildScene;

/// Réglages du joueur, appliqués et enregistrés dès qu'ils changent.
#[derive(Resource, Clone, Debug)]
pub struct PlayerSettings(pub Settings);

/// Écran où aller après `Boot`.
#[derive(Resource, Debug)]
struct StartState(AppState);

#[derive(Resource, Debug)]
struct MenuArt {
    menu: Handle<Image>,
    loading: Handle<Image>,
}

/// Page affichée à droite du menu principal.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
enum MenuPage {
    #[default]
    None,
    Load,
    Settings,
}

/// Sauvegardes trouvées sur le disque (la plus récente d'abord).
#[derive(Resource, Debug, Default)]
struct SaveList(Vec<(PathBuf, SaveGame)>);

/// Dernière erreur à montrer sur le menu (chargement raté…).
#[derive(Resource, Debug, Default)]
struct MenuError(Option<String>);

#[derive(Resource, Debug, Default)]
pub struct PauseMenu {
    pub open: bool,
    /// Échap non utilisé par les outils : ouvrir ou fermer le menu pause.
    pub toggle_requested: bool,
    resume_speed: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuAction {
    Continue,
    NewGame,
    LoadPage,
    LoadSave(usize),
    SettingsPage,
    Quit,
    ToggleFullscreen,
    ToggleShadows,
    CycleUiScale,
    CycleLanguage,
    Resume,
    Save,
    MainMenu,
}

#[derive(Component)]
struct MenuButton {
    action: MenuAction,
    hovered: bool,
}

#[derive(Component)]
struct MenuCamera;

#[derive(Component)]
struct SplashText;

#[derive(Component)]
struct SplashLine;

#[derive(Component)]
struct Backdrop;

#[derive(Component)]
struct PagePanel;

#[derive(Component)]
struct ErrorText;

#[derive(Component)]
struct PauseRoot;

#[derive(Component)]
struct LoadingBar;

#[derive(Component)]
struct LoadingLabel;

#[derive(Component)]
struct LoadingPercent;

#[derive(Component)]
struct LoadingTip;

/// Travail de l'écran de chargement.
#[derive(Resource)]
struct LoadingJob {
    progress: Arc<AtomicU32>,
    result: Arc<Mutex<Option<Result<Session, String>>>>,
    elapsed: f32,
    shown: f32,
    replay: bool,
    /// 0 : génération/rejeu, 1 : construction de la scène, 2 : prêt.
    phase: u8,
}

impl std::fmt::Debug for LoadingJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadingJob").field("phase", &self.phase).finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct ScreensPlugin {
    /// État de départ (`Loading` pour les tours de captures et les tests).
    pub start: AppState,
    pub request: Option<LoadRequest>,
}

impl Plugin for ScreensPlugin {
    fn build(&self, app: &mut App) {
        let settings = Settings::load(&Settings::path());
        // La première transition d'état a lieu avant `Startup` : on démarre en
        // `Boot`, et `load_assets` passe à l'écran voulu une fois les polices
        // et images prêtes.
        app.insert_state(AppState::Boot)
            .insert_resource(StartState(self.start))
            .insert_resource(PlayerSettings(settings))
            .init_resource::<MenuPage>()
            .init_resource::<SaveList>()
            .init_resource::<MenuError>()
            .init_resource::<PauseMenu>()
            .add_systems(Startup, (load_assets, apply_settings).chain())
            .add_systems(OnEnter(AppState::Splash), (ensure_menu_camera, spawn_splash).chain())
            .add_systems(Update, splash.run_if(in_state(AppState::Splash)))
            .add_systems(OnEnter(AppState::MainMenu), (ensure_menu_camera, refresh_saves, spawn_main_menu).chain())
            .add_systems(
                Update,
                (menu_hover, menu_clicks, rebuild_page, animate_backdrop, show_error)
                    .chain()
                    .run_if(in_state(AppState::MainMenu)),
            )
            .add_systems(OnEnter(AppState::Loading), (ensure_menu_camera, start_loading).chain())
            .add_systems(Update, loading.run_if(in_state(AppState::Loading)))
            .add_systems(OnEnter(AppState::InGame), remove_menu_camera)
            .add_systems(OnExit(AppState::InGame), cleanup_game)
            .add_systems(
                Update,
                (pause_menu, menu_hover, menu_clicks).chain().run_if(in_state(AppState::InGame)),
            )
            .add_systems(Update, apply_settings.run_if(resource_changed::<PlayerSettings>));
        if let Some(request) = &self.request {
            app.insert_resource(request.clone());
        }
        if std::env::var_os("CITYMAYOR_SCREENS").is_some() {
            app.init_resource::<ScreensTest>().add_systems(Update, screens_test);
        }
    }
}

fn font(handle: &HudFont, size: f32, weight: u16) -> TextFont {
    TextFont { font: FontSource::Handle(handle.0.clone()), font_size: FontSize::Px(size), weight: FontWeight(weight), ..default() }
}

fn load_assets(
    mut commands: Commands,
    mut fonts: ResMut<Assets<Font>>,
    mut images: ResMut<Assets<Image>>,
    start: Res<StartState>,
    mut next: ResMut<NextState<AppState>>,
) {
    next.set(start.0);
    commands.insert_resource(HudFont(fonts.add(Font::from_bytes(hud::FONT_TTF.to_vec()))));
    let decode = |bytes: &[u8]| {
        Image::from_buffer(
            bytes,
            bevy::image::ImageType::Extension("jpg"),
            bevy::image::CompressedImageFormats::NONE,
            true,
            bevy::image::ImageSampler::linear(),
            bevy::asset::RenderAssetUsages::RENDER_WORLD,
        )
        .expect("image de menu embarquée illisible")
    };
    commands.insert_resource(MenuArt { menu: images.add(decode(MENU_ART)), loading: images.add(decode(LOADING_ART)) });
}

fn apply_settings(settings: Res<PlayerSettings>, mut windows: Query<&mut Window>, mut scale: ResMut<UiScale>, mut lights: Query<&mut DirectionalLight>) {
    let s = &settings.0;
    for mut window in &mut windows {
        let mode = if s.fullscreen { WindowMode::BorderlessFullscreen(MonitorSelection::Current) } else { WindowMode::Windowed };
        if window.mode != mode {
            window.mode = mode;
        }
    }
    scale.0 = f32::from(s.ui_scale_percent) / 100.0;
    for mut light in &mut lights {
        light.shadow_maps_enabled = s.shadows;
    }
    let _ = s.save(&Settings::path());
}

fn ensure_menu_camera(mut commands: Commands, cameras: Query<(), With<MenuCamera>>) {
    if cameras.is_empty() {
        commands.spawn((MenuCamera, Camera2d));
    }
}

fn remove_menu_camera(mut commands: Commands, cameras: Query<Entity, With<MenuCamera>>) {
    for camera in &cameras {
        commands.entity(camera).despawn();
    }
}

// ------------------------------------------------------------------ studio

fn spawn_splash(mut commands: Commands, ui_font: Res<HudFont>, texts: Res<Texts>) {
    commands
        .spawn((
            DespawnOnExit(AppState::Splash),
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(14.0),
                ..default()
            },
            BackgroundColor(Color::BLACK),
        ))
        .with_children(|root| {
            root.spawn((SplashText, Text::new(texts.text("splash-studio")), font(&ui_font, 92.0, 800), TextColor(TEXT.with_alpha(0.0))));
            root.spawn((SplashLine, Node { width: Val::Px(260.0), height: Val::Px(2.0), ..default() }, BackgroundColor(ACCENT.with_alpha(0.0))));
            root.spawn((SplashText, Text::new(texts.text("splash-studio-sub")), font(&ui_font, 22.0, 300), TextColor(TEXT.with_alpha(0.0))));
            root.spawn((SplashText, Text::new(texts.text("splash-presents")), font(&ui_font, 14.0, 400), TextColor(TEXT_DIM.with_alpha(0.0)), Node { margin: UiRect::top(Val::Px(40.0)), ..default() }));
        });
}

#[allow(clippy::type_complexity)]
fn splash(
    time: Res<Time>,
    mut elapsed: Local<f32>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut next: ResMut<NextState<AppState>>,
    mut words: Query<&mut TextColor, With<SplashText>>,
    mut line: Query<(&mut BackgroundColor, &mut Node), With<SplashLine>>,
) {
    *elapsed += time.delta_secs();
    let t = *elapsed;
    // Fondu d'entrée (0,9 s), maintien, fondu de sortie (0,8 s).
    let alpha = (t / 0.9).min(1.0).min(((SPLASH_S - t) / 0.8).max(0.0));
    for mut color in &mut words {
        color.0 = color.0.with_alpha(alpha);
    }
    for (mut color, mut node) in &mut line {
        color.0 = ACCENT.with_alpha(alpha);
        // La ligne se dessine de gauche à droite.
        node.width = Val::Px(260.0 * ((t - 0.3) / 0.9).clamp(0.0, 1.0));
    }
    let skip = t > 0.4 && (mouse.just_pressed(MouseButton::Left) || keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::Space) || keys.just_pressed(KeyCode::Enter));
    if t >= SPLASH_S || skip {
        *elapsed = 0.0;
        next.set(AppState::MainMenu);
    }
}

// ------------------------------------------------------------- menu principal

fn refresh_saves(mut list: ResMut<SaveList>) {
    list.0 = session::list_saves(&session::saves_dir());
}

/// Date civile (UTC) d'un horodatage Unix : (année, mois, jour, heure, minute).
fn civil(unix: u64) -> (i64, u32, u32, u32, u32) {
    let days = (unix / 86_400) as i64;
    let secs = unix % 86_400;
    // Algorithme de Howard Hinnant (jours → date grégorienne).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day, (secs / 3600) as u32, ((secs % 3600) / 60) as u32)
}

fn saved_date(texts: &Texts, unix: u64) -> String {
    let (y, mo, d, h, mi) = civil(unix);
    texts.format(
        "date-saved",
        &[
            ("year", Arg::from(y)),
            ("month", Arg::from(format!("{mo:02}"))),
            ("day", Arg::from(format!("{d:02}"))),
            ("hour", Arg::from(format!("{h:02}"))),
            ("minute", Arg::from(format!("{mi:02}"))),
        ],
    )
}

/// Date de jeu d'une sauvegarde (mêmes règles que la barre du haut).
fn game_date(texts: &Texts, tick: u64) -> String {
    let config = sim_core::SimConfig::default();
    let day = tick / config.ticks_per_day;
    let hour = (tick % config.ticks_per_day) * 24 / config.ticks_per_day;
    texts.format(
        "hud-date-value",
        &[
            ("month", Arg::from(day / config.days_per_month + 1)),
            ("day", Arg::from(day % config.days_per_month + 1)),
            ("hour", Arg::from(hour)),
        ],
    )
}

fn menu_button(parent: &mut ChildSpawnerCommands, ui_font: &HudFont, label: String, detail: Option<String>, action: MenuAction, size: f32) {
    parent
        .spawn((
            MenuButton { action, hovered: false },
            Node {
                flex_direction: FlexDirection::Column,
                padding: UiRect::new(Val::Px(18.0), Val::Px(28.0), Val::Px(8.0), Val::Px(8.0)),
                border: UiRect::left(Val::Px(4.0)),
                border_radius: BorderRadius::right(Val::Px(8.0)),
                ..default()
            },
            BackgroundColor(Color::NONE),
            BorderColor::all(Color::NONE),
        ))
        .with_children(|b| {
            b.spawn((Text::new(label), font(ui_font, size, 600), TextColor(TEXT)));
            if let Some(detail) = detail {
                b.spawn((Text::new(detail), font(ui_font, 13.0, 400), TextColor(TEXT_DIM)));
            }
        });
}

fn spawn_main_menu(mut commands: Commands, ui_font: Res<HudFont>, texts: Res<Texts>, art: Res<MenuArt>, saves: Res<SaveList>, mut page: ResMut<MenuPage>) {
    *page = MenuPage::None;
    let t = |key: &str| texts.text(key);
    commands
        .spawn((DespawnOnExit(AppState::MainMenu), Node { width: Val::Percent(100.0), height: Val::Percent(100.0), overflow: Overflow::clip(), ..default() }, BackgroundColor(Color::BLACK)))
        .with_children(|root| {
            // Vue de la ville, en lent travelling avant.
            root.spawn((
                Backdrop,
                ImageNode::new(art.menu.clone()),
                Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
            ));
            // Colonne de gauche, fondue sur l'image.
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    bottom: Val::Px(0.0),
                    width: Val::Px(520.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::new(Val::Px(72.0), Val::Px(24.0), Val::Px(96.0), Val::Px(40.0)),
                    ..default()
                },
                BackgroundColor(SHADE),
            ))
            .with_children(|col| {
                col.spawn((Text::new(t("app-title")), font(&ui_font, 64.0, 800), TextColor(TEXT)));
                col.spawn((Node { width: Val::Px(120.0), height: Val::Px(3.0), margin: UiRect::vertical(Val::Px(10.0)), ..default() }, BackgroundColor(ACCENT)));
                col.spawn((Text::new(t("app-tagline")), font(&ui_font, 16.0, 400), TextColor(TEXT_DIM), Node { margin: UiRect::bottom(Val::Px(56.0)), ..default() }));
                if let Some((_, save)) = saves.0.first() {
                    let detail = texts.format("menu-continue-detail", &[("name", Arg::from(save.name.clone())), ("date", Arg::from(saved_date(&texts, save.saved_at)))]);
                    menu_button(col, &ui_font, t("menu-continue"), Some(detail), MenuAction::Continue, 30.0);
                }
                menu_button(col, &ui_font, t("menu-new-game"), None, MenuAction::NewGame, 30.0);
                menu_button(col, &ui_font, t("menu-load-game"), None, MenuAction::LoadPage, 30.0);
                menu_button(col, &ui_font, t("menu-settings"), None, MenuAction::SettingsPage, 30.0);
                menu_button(col, &ui_font, t("menu-quit"), None, MenuAction::Quit, 30.0);
                col.spawn((ErrorText, Text::new(""), font(&ui_font, 14.0, 500), TextColor(Color::srgb(1.0, 0.5, 0.45)), Node { margin: UiRect::top(Val::Px(18.0)), max_width: Val::Px(400.0), ..default() }));
                col.spawn(Node { flex_grow: 1.0, ..default() });
                col.spawn((Text::new(texts.format("menu-version", &[("version", Arg::from(env!("CARGO_PKG_VERSION")))])), font(&ui_font, 12.0, 400), TextColor(TEXT_DIM)));
            });
            // Bord de la colonne fondu dans l'image (pas d'arête franche).
            let shade = SHADE.to_srgba();
            for i in 0..12 {
                let fade = 1.0 - (i as f32 + 1.0) / 13.0;
                root.spawn((
                    Node { position_type: PositionType::Absolute, left: Val::Px(520.0 + i as f32 * 14.0), top: Val::Px(0.0), bottom: Val::Px(0.0), width: Val::Px(14.0), ..default() },
                    BackgroundColor(Color::srgba(shade.red, shade.green, shade.blue, shade.alpha * fade * fade)),
                ));
            }
            root.spawn((
                Text::new(t("menu-studio")),
                font(&ui_font, 13.0, 600),
                TextColor(TEXT.with_alpha(0.8)),
                Node { position_type: PositionType::Absolute, right: Val::Px(28.0), bottom: Val::Px(20.0), ..default() },
            ));
            // Page de droite (chargement, réglages), remplie à la demande.
            root.spawn((
                PagePanel,
                Node {
                    position_type: PositionType::Absolute,
                    right: Val::Percent(6.0),
                    top: Val::Percent(14.0),
                    width: Val::Px(560.0),
                    max_height: Val::Percent(72.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(26.0)),
                    row_gap: Val::Px(6.0),
                    border_radius: BorderRadius::all(Val::Px(14.0)),
                    overflow: Overflow::clip(),
                    display: Display::None,
                    ..default()
                },
                BackgroundColor(SHADE),
            ));
        });
}

fn animate_backdrop(time: Res<Time>, mut backdrop: Query<&mut Node, With<Backdrop>>) {
    // Travelling lent et continu (40 s par aller-retour) : la ville respire.
    let k = (time.elapsed_secs() * std::f32::consts::TAU / 40.0).sin() * 0.5 + 0.5;
    let zoom = 104.0 + 6.0 * k;
    for mut node in &mut backdrop {
        node.width = Val::Percent(zoom);
        node.height = Val::Percent(zoom);
        node.left = Val::Percent(-(zoom - 100.0) * 0.3);
        node.top = Val::Percent(-(zoom - 100.0) * 0.5);
    }
}

fn show_error(error: Res<MenuError>, mut texts: Query<&mut Text, With<ErrorText>>) {
    if !error.is_changed() {
        return;
    }
    for mut text in &mut texts {
        text.0 = error.0.clone().unwrap_or_default();
    }
}

#[allow(clippy::too_many_arguments)]
fn rebuild_page(
    mut commands: Commands,
    page: Res<MenuPage>,
    settings: Res<PlayerSettings>,
    saves: Res<SaveList>,
    ui_font: Res<HudFont>,
    texts: Res<Texts>,
    mut panels: Query<(Entity, &mut Node), With<PagePanel>>,
) {
    if !page.is_changed() && !settings.is_changed() {
        return;
    }
    let Ok((panel, mut node)) = panels.single_mut() else { return };
    commands.entity(panel).despawn_related::<Children>();
    node.display = if *page == MenuPage::None { Display::None } else { Display::Flex };
    let t = |key: &str| texts.text(key);
    commands.entity(panel).with_children(|p| match *page {
        MenuPage::None => {}
        MenuPage::Load => {
            p.spawn((Text::new(t("load-title")), font(&ui_font, 28.0, 700), TextColor(TEXT), Node { margin: UiRect::bottom(Val::Px(12.0)), ..default() }));
            if saves.0.is_empty() {
                p.spawn((Text::new(t("load-empty")), font(&ui_font, 15.0, 400), TextColor(TEXT_DIM)));
            }
            for (i, (_, save)) in saves.0.iter().enumerate().take(8) {
                let detail = texts.format(
                    "load-entry-detail",
                    &[("game_date", Arg::from(game_date(&texts, save.tick))), ("date", Arg::from(saved_date(&texts, save.saved_at)))],
                );
                menu_button(p, &ui_font, save.name.clone(), Some(detail), MenuAction::LoadSave(i), 20.0);
            }
        }
        MenuPage::Settings => {
            let s = &settings.0;
            let on_off = |v: bool| if v { t("settings-on") } else { t("settings-off") };
            p.spawn((Text::new(t("settings-title")), font(&ui_font, 28.0, 700), TextColor(TEXT), Node { margin: UiRect::bottom(Val::Px(12.0)), ..default() }));
            let rows = [
                (t("settings-fullscreen"), on_off(s.fullscreen), MenuAction::ToggleFullscreen),
                (t("settings-shadows"), on_off(s.shadows), MenuAction::ToggleShadows),
                (t("settings-ui-scale"), texts.format("settings-percent", &[("value", Arg::from(s.ui_scale_percent))]), MenuAction::CycleUiScale),
                (t("settings-language"), t(&format!("language-{}", s.language)), MenuAction::CycleLanguage),
            ];
            for (label, value, action) in rows {
                menu_button(p, &ui_font, label, Some(value), action, 20.0);
            }
        }
    });
}

fn menu_hover(
    windows: Query<&Window>,
    mut buttons: Query<(&mut MenuButton, &ComputedNode, &UiGlobalTransform, &mut BackgroundColor, &mut BorderColor)>,
) {
    let cursor = windows.iter().next().and_then(Window::physical_cursor_position);
    for (mut button, node, transform, mut color, mut border) in &mut buttons {
        let hovered = cursor.is_some_and(|c| node.size().x > 0.0 && node.contains_point(*transform, c));
        button.hovered = hovered;
        let (bg, edge) = if hovered { (ROW_HOVER, ACCENT) } else { (Color::NONE, Color::NONE) };
        if color.0 != bg {
            color.0 = bg;
            *border = BorderColor::all(edge);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn menu_clicks(
    mut commands: Commands,
    mouse: Res<ButtonInput<MouseButton>>,
    buttons: Query<&MenuButton>,
    mut page: ResMut<MenuPage>,
    saves: Res<SaveList>,
    mut settings: ResMut<PlayerSettings>,
    mut next: ResMut<NextState<AppState>>,
    mut exit: MessageWriter<AppExit>,
    mut pause: ResMut<PauseMenu>,
    sim: Option<ResMut<Sim>>,
    meta: Option<Res<SessionMeta>>,
    texts: Res<Texts>,
    feed: Option<ResMut<hud::Feed>>,
    mut error: ResMut<MenuError>,
) {
    if !mouse.just_released(MouseButton::Left) {
        return;
    }
    let Some(action) = buttons.iter().find(|b| b.hovered).map(|b| b.action) else { return };
    match action {
        MenuAction::Continue => {
            if let Some((path, _)) = saves.0.first() {
                commands.insert_resource(LoadRequest::Load(path.clone()));
                error.0 = None;
                next.set(AppState::Loading);
            }
        }
        MenuAction::NewGame => {
            // Nouvelle partie : carte vierge (la démo pré-construite reste pour
            // le tour de captures). Quelques ticks pour poser la route d'entrée.
            commands.insert_resource(LoadRequest::New { scenario: "blank".into(), ticks: 2 });
            error.0 = None;
            next.set(AppState::Loading);
        }
        MenuAction::LoadSave(i) => {
            if let Some((path, _)) = saves.0.get(i) {
                commands.insert_resource(LoadRequest::Load(path.clone()));
                error.0 = None;
                next.set(AppState::Loading);
            }
        }
        MenuAction::LoadPage => *page = if *page == MenuPage::Load { MenuPage::None } else { MenuPage::Load },
        MenuAction::SettingsPage => *page = if *page == MenuPage::Settings { MenuPage::None } else { MenuPage::Settings },
        MenuAction::Quit => {
            exit.write(AppExit::Success);
        }
        MenuAction::ToggleFullscreen => settings.0.fullscreen = !settings.0.fullscreen,
        MenuAction::ToggleShadows => settings.0.shadows = !settings.0.shadows,
        MenuAction::CycleUiScale => {
            let i = UI_SCALES.iter().position(|v| *v == settings.0.ui_scale_percent).unwrap_or(1);
            settings.0.ui_scale_percent = UI_SCALES[(i + 1) % UI_SCALES.len()];
        }
        MenuAction::CycleLanguage => {
            // Une seule langue livrée pour l'instant : le réglage reste prêt.
            let dir = std::path::Path::new("locales");
            let languages = city_i18n::available_languages(dir);
            let i = languages.iter().position(|l| *l == settings.0.language).unwrap_or(0);
            settings.0.language = languages.get((i + 1) % languages.len().max(1)).cloned().unwrap_or_else(|| "en".into());
        }
        MenuAction::Resume => pause.toggle_requested = true,
        MenuAction::Save => {
            if let (Some(sim), Some(meta)) = (sim.as_ref(), meta.as_ref()) {
                let saved = session::save_world(&sim.world, &meta.scenario, meta.skip, &meta.name, &session::saves_dir());
                if let Some(mut feed) = feed {
                    match saved {
                        Ok(_) => feed.push(texts.text("pause-saved"), false),
                        Err(_) => feed.push(texts.text("pause-save-failed"), true),
                    }
                }
            }
            pause.toggle_requested = true;
        }
        MenuAction::MainMenu => next.set(AppState::MainMenu),
    }
}

// ------------------------------------------------------------- chargement

fn start_loading(mut commands: Commands, request: Option<Res<LoadRequest>>, ui_font: Res<HudFont>, texts: Res<Texts>, art: Res<MenuArt>) {
    let request = request.map_or(LoadRequest::New { scenario: "demo".into(), ticks: session::NEW_GAME_TICKS }, |r| r.clone());
    let progress = Arc::new(AtomicU32::new(0));
    let result = Arc::new(Mutex::new(None));
    let replay = matches!(request, LoadRequest::Load(_));
    {
        let (progress, result) = (progress.clone(), result.clone());
        let name = texts.text("default-city-name");
        // La génération et la simulation tournent à part : l'écran reste fluide.
        std::thread::spawn(move || {
            let mut report = |p: f32| progress.store(p.to_bits(), Ordering::Relaxed);
            let outcome = match request {
                LoadRequest::New { scenario, ticks } => session::new_game(&scenario, &name, ticks, &mut report),
                LoadRequest::Load(path) => std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|text| SaveGame::from_text(&text))
                    .and_then(|save| session::load(&save, &mut report)),
            };
            if let Ok(mut slot) = result.lock() {
                *slot = Some(outcome);
            }
        });
    }
    commands.insert_resource(LoadingJob { progress, result, elapsed: 0.0, shown: 0.0, replay, phase: 0 });

    let t = |key: &str| texts.text(key);
    commands
        .spawn((DespawnOnExit(AppState::Loading), Node { width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() }, BackgroundColor(Color::BLACK)))
        .with_children(|root| {
            root.spawn((ImageNode::new(art.loading.clone()), Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() }));
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    right: Val::Px(0.0),
                    bottom: Val::Px(0.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::new(Val::Percent(8.0), Val::Percent(8.0), Val::Px(28.0), Val::Px(44.0)),
                    row_gap: Val::Px(10.0),
                    ..default()
                },
                BackgroundColor(SHADE),
            ))
            .with_children(|band| {
                band.spawn((Text::new(t("app-title")), font(&ui_font, 40.0, 800), TextColor(TEXT)));
                band.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, ..default() }).with_children(|row| {
                    row.spawn((LoadingLabel, Text::new(t("loading-generating")), font(&ui_font, 16.0, 500), TextColor(TEXT_DIM)));
                    row.spawn((LoadingPercent, Text::new("0%"), font(&ui_font, 16.0, 700), TextColor(TEXT)));
                });
                band.spawn((Node { width: Val::Percent(100.0), height: Val::Px(6.0), border_radius: BorderRadius::all(Val::Px(3.0)), ..default() }, BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.14))))
                    .with_children(|track| {
                        track.spawn((LoadingBar, Node { width: Val::Percent(0.0), height: Val::Percent(100.0), border_radius: BorderRadius::all(Val::Px(3.0)), ..default() }, BackgroundColor(ACCENT)));
                    });
                band.spawn((LoadingTip, Text::new(t("tip-1")), font(&ui_font, 14.0, 400), TextColor(TEXT_DIM), Node { margin: UiRect::top(Val::Px(8.0)), ..default() }));
            });
        });
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn loading(
    mut commands: Commands,
    time: Res<Time>,
    job: Option<ResMut<LoadingJob>>,
    texts: Res<Texts>,
    build: Option<Res<BuildScene>>,
    mut next: ResMut<NextState<AppState>>,
    mut error: ResMut<MenuError>,
    mut bar: Query<&mut Node, With<LoadingBar>>,
    mut label: Query<&mut Text, (With<LoadingLabel>, Without<LoadingPercent>, Without<LoadingTip>)>,
    mut percent: Query<&mut Text, (With<LoadingPercent>, Without<LoadingLabel>, Without<LoadingTip>)>,
    mut tip: Query<&mut Text, (With<LoadingTip>, Without<LoadingLabel>, Without<LoadingPercent>)>,
) {
    let Some(mut job) = job else { return };
    job.elapsed += time.delta_secs();
    let raw = f32::from_bits(job.progress.load(Ordering::Relaxed));
    // Phase 0 : la partie se prépare à part.
    if job.phase == 0 {
        let done = job.result.lock().ok().and_then(|mut slot| slot.take());
        match done {
            Some(Ok(session)) => {
                let tour_active = std::env::var_os("CITYMAYOR_TOUR").is_some();
                commands.insert_resource(crate::viewer::Tour::new(tour_active, &session.world));
                commands.insert_resource(SessionMeta { scenario: session.scenario.clone(), skip: session.skip, name: session.name.clone() });
                commands.insert_resource(Sim::new(session.world));
                commands.insert_resource(BuildScene);
                job.phase = 1;
            }
            Some(Err(reason)) => {
                error.0 = Some(texts.format("error-load-failed", &[("reason", Arg::from(reason))]));
                commands.remove_resource::<LoadingJob>();
                next.set(AppState::MainMenu);
                return;
            }
            None => {}
        }
    } else if job.phase == 1 && build.is_none() {
        // `setup` a construit la scène (il retire `BuildScene`).
        job.phase = 2;
    }
    let target = match job.phase {
        0 => raw * 0.85,
        1 => 0.92,
        _ => 1.0,
    };
    // La barre rejoint sa cible en douceur, jamais en arrière.
    let shown = job.shown + (target - job.shown) * (time.delta_secs() * 4.0).min(1.0);
    job.shown = shown.max(job.shown);
    let key = match job.phase {
        0 if raw < 0.03 => "loading-generating",
        0 if job.replay => "loading-replaying",
        0 => "loading-simulating",
        1 => "loading-building",
        _ => "loading-ready",
    };
    for mut node in &mut bar {
        node.width = Val::Percent(job.shown * 100.0);
    }
    let set = |text: &mut Text, value: String| {
        if text.0 != value {
            text.0 = value;
        }
    };
    for mut text in &mut label {
        set(&mut text, texts.text(key));
    }
    for mut text in &mut percent {
        set(&mut text, format!("{:.0}%", job.shown * 100.0));
    }
    let tip_index = (job.elapsed / 5.0) as usize % 8 + 1;
    for mut text in &mut tip {
        set(&mut text, texts.text(&format!("tip-{tip_index}")));
    }
    // Non passable : on n'écoute aucune touche, et l'écran reste au moins
    // `MIN_LOADING_S` (pas de clignotement sur une machine rapide).
    if job.phase == 2 && job.elapsed >= MIN_LOADING_S && job.shown > 0.97 {
        commands.remove_resource::<LoadingJob>();
        next.set(AppState::InGame);
    }
}

// ------------------------------------------------------------- en partie

#[allow(clippy::too_many_arguments)]
fn pause_menu(
    mut commands: Commands,
    mut pause: ResMut<PauseMenu>,
    mut sim: ResMut<Sim>,
    roots: Query<Entity, With<PauseRoot>>,
    ui_font: Res<HudFont>,
    texts: Res<Texts>,
    tour: Res<crate::viewer::Tour>,
) {
    if !pause.toggle_requested || tour.active {
        pause.toggle_requested = false;
        return;
    }
    pause.toggle_requested = false;
    pause.open = !pause.open;
    if !pause.open {
        for root in &roots {
            commands.entity(root).despawn();
        }
        sim.speed = pause.resume_speed.max(1);
        return;
    }
    pause.resume_speed = sim.speed;
    sim.speed = 0;
    let t = |key: &str| texts.text(key);
    commands
        .spawn((
            PauseRoot,
            hud::Blocker,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
            GlobalZIndex(10),
        ))
        .with_children(|root| {
            root.spawn((
                Node { flex_direction: FlexDirection::Column, padding: UiRect::all(Val::Px(30.0)), row_gap: Val::Px(4.0), min_width: Val::Px(360.0), border_radius: BorderRadius::all(Val::Px(16.0)), ..default() },
                BackgroundColor(SHADE),
            ))
            .with_children(|panel| {
                panel.spawn((Text::new(t("pause-title")), font(&ui_font, 34.0, 800), TextColor(TEXT), Node { margin: UiRect::bottom(Val::Px(16.0)), ..default() }));
                menu_button(panel, &ui_font, t("pause-resume"), None, MenuAction::Resume, 22.0);
                menu_button(panel, &ui_font, t("pause-save"), None, MenuAction::Save, 22.0);
                menu_button(panel, &ui_font, t("pause-main-menu"), None, MenuAction::MainMenu, 22.0);
                menu_button(panel, &ui_font, t("pause-quit"), None, MenuAction::Quit, 22.0);
            });
        });
}

/// Retour au menu : on démonte toute la scène (maillages, caméra, lumière,
/// interface) et les ressources de la partie.
#[allow(clippy::type_complexity)]
fn cleanup_game(
    mut commands: Commands,
    scene: Query<Entity, Or<(With<Mesh3d>, With<Camera3d>, With<DirectionalLight>)>>,
    ui_roots: Query<Entity, (With<Node>, Without<ChildOf>)>,
    mut pause: ResMut<PauseMenu>,
) {
    for entity in scene.iter().chain(ui_roots.iter()) {
        commands.entity(entity).despawn();
    }
    *pause = PauseMenu::default();
    commands.remove_resource::<Sim>();
    commands.remove_resource::<SessionMeta>();
    commands.insert_resource(hud::Tools::default());
    commands.insert_resource(hud::Feed::default());
    crate::viewer::reset_game_resources(&mut commands);
}

#[cfg(test)]
mod tests {
    use super::civil;

    #[test]
    fn unix_dates_are_civil_dates() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0));
        assert_eq!(civil(951_782_400), (2000, 2, 29, 0, 0));
        assert_eq!(civil(1_795_712_912), (2026, 11, 26, 17, 8));
    }
}

// ------------------------------------------------------------ test scripté

/// Parcours des écrans sans souris (`CITYMAYOR_SCREENS=1`) : studio, menu,
/// réglages, chargement (petit pâté), partie, pause, sauvegarde, retour au
/// menu (« Continuer » apparaît), avec une capture à chaque étape.
#[derive(Resource, Debug, Default)]
pub struct ScreensTest {
    step: usize,
    elapsed: f32,
    shot: bool,
}

#[allow(clippy::too_many_arguments)]
fn screens_test(
    mut commands: Commands,
    time: Res<Time>,
    mut test: ResMut<ScreensTest>,
    state: Res<State<AppState>>,
    mut next: ResMut<NextState<AppState>>,
    mut page: ResMut<MenuPage>,
    mut pause: ResMut<PauseMenu>,
    sim: Option<Res<Sim>>,
    meta: Option<Res<SessionMeta>>,
    mut exit: MessageWriter<AppExit>,
) {
    test.elapsed += time.delta_secs();
    // (état attendu, délai avant capture, nom)
    let steps: [(AppState, f32, &str); 7] = [
        (AppState::Splash, 1.6, "1-studio"),
        (AppState::MainMenu, 2.0, "2-menu"),
        (AppState::MainMenu, 1.0, "3-reglages"),
        (AppState::Loading, 1.5, "4-chargement"),
        (AppState::InGame, 3.0, "5-partie"),
        (AppState::InGame, 1.0, "6-pause"),
        (AppState::MainMenu, 2.0, "7-menu-continuer"),
    ];
    let Some(&(expected, delay, name)) = steps.get(test.step) else {
        exit.write(AppExit::Success);
        return;
    };
    if *state.get() != expected {
        test.elapsed = 0.0;
        return;
    }
    if test.elapsed > delay && !test.shot {
        test.shot = true;
        commands
            .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
            .observe(bevy::render::view::screenshot::save_to_disk(format!("citymayor-ecran-{name}.png")));
    }
    if test.elapsed < delay + 0.6 {
        return;
    }
    // Action pour passer à l'étape suivante.
    match test.step {
        1 => *page = MenuPage::Settings,
        2 => {
            commands.insert_resource(LoadRequest::New { scenario: "block".into(), ticks: 600 });
            next.set(AppState::Loading);
        }
        4 => pause.toggle_requested = true,
        5 => {
            if let (Some(sim), Some(meta)) = (sim, meta) {
                let _ = session::save_world(&sim.world, &meta.scenario, meta.skip, &meta.name, &session::saves_dir());
            }
            next.set(AppState::MainMenu);
        }
        _ => {}
    }
    test.step += 1;
    test.elapsed = 0.0;
    test.shot = false;
}
