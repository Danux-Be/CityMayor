//! Viewer Bevy : la ville dans son paysage.
//!
//! # Contrat de cette couche
//! - **Aucune règle de jeu ici.** Le viewer lit `scene::*` et `nature::*`
//!   (dérivés de `World`) et pousse des ticks. Une règle de ville change dans
//!   `sim-core`, jamais ici.
//! - **Géométrie statique reconstruite à la demande**, pas par frame. Le sol,
//!   l'eau, le ciel et les forêts du décor sont construits une fois ; la ville
//!   (bâtiments, routes, arbres de la zone jouable) quand elle change.
//! - **Rendu à fréquence libre, simulation à 20 Hz** (`FixedStepClock`).
//!
//! # Une carte « infinie »
//! Le paysage (relief généré, forêts, fleuve, montagnes) s'étend à
//! `nature::DECOR_EXTENT_M` autour de la zone jouable. La caméra est bornée
//! (`nature::CameraBounds`) et le brouillard, couleur d'horizon, efface le
//! décor avant sa fin : on ne voit jamais le bord du monde, mais on ne peut
//! pas non plus quitter sa ville.
//!
//! # Commandes
//! - souris : clic droit + glisser = tourner/incliner, clic milieu (ou
//!   gauche) + glisser = déplacer, molette = zoom ;
//! - clavier : WASD/flèches = déplacer, Q/E = tourner, R/F = incliner,
//!   Z/X = zoom, F12 = capture.
//! - `CITYMAYOR_TOUR=1` : enchaîne des captures depuis plusieurs points de vue
//!   puis quitte (vérification visuelle sans interaction).

use bevy::asset::{embedded_asset, RenderAssetUsages};
use bevy::image::{
    CompressedImageFormats, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor,
    ImageType,
};
use bevy::camera::visibility::VisibilityRange;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::light::CascadeShadowConfigBuilder;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
    TextureViewDescriptor, TextureViewDimension,
};
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::shader::ShaderRef;

use sim_core::{FixedStepClock, World, ZoneType};

use std::collections::HashMap;

use crate::{nature, scene, urban};

/// Ticks simulés avant d'ouvrir la fenêtre : la ville est déjà peuplée.
const PRESIMULATION_TICKS: u32 = 2_400;

type TerrainMaterial = ExtendedMaterial<StandardMaterial, TerrainExtension>;
type WaterMaterial = ExtendedMaterial<StandardMaterial, WaterExtension>;
type UrbanMaterial = ExtendedMaterial<StandardMaterial, UrbanExtension>;

/// Extension du matériau standard : six couches de sol dans un tableau de
/// textures, mélangées par `shaders/terrain.wesl`.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
struct TerrainExtension {
    #[texture(100, dimension = "2d_array")]
    #[sampler(101)]
    layers: Handle<Image>,
    #[uniform(102)]
    scales_a: Vec4,
    #[uniform(102)]
    scales_b: Vec4,
}

impl MaterialExtension for TerrainExtension {
    fn fragment_shader() -> ShaderRef {
        "embedded://city_render/shaders/terrain.wesl".into()
    }
}

/// Extension du matériau standard : vagues animées (`shaders/water.wesl`).
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
struct WaterExtension {
    #[uniform(100)]
    waves: Vec4,
}

impl MaterialExtension for WaterExtension {
    fn fragment_shader() -> ShaderRef {
        "embedded://city_render/shaders/water.wesl".into()
    }
}

/// Extension du matériau standard : façades, toits et voirie dans un tableau
/// de textures (`urban::LAYER_NAMES`), couche choisie par sommet
/// (`shaders/urban.wesl`).
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
struct UrbanExtension {
    #[texture(100, dimension = "2d_array")]
    #[sampler(101)]
    layers: Handle<Image>,
}

impl MaterialExtension for UrbanExtension {
    fn fragment_shader() -> ShaderRef {
        "embedded://city_render/shaders/urban.wesl".into()
    }
}

/// Lancée par le binaire `city-viewer`.
pub fn run() {
    let mut world = scene::demo_city();
    for _ in 0..PRESIMULATION_TICKS {
        world.tick();
    }
    let tour = std::env::var_os("CITYMAYOR_TOUR").is_some();

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "CityMayor".into(),
            resolution: (1600u32, 900u32).into(),
            ..default()
        }),
        ..default()
    }));
    embedded_asset!(app, "shaders/terrain.wesl");
    embedded_asset!(app, "shaders/water.wesl");
    embedded_asset!(app, "shaders/urban.wesl");
    app.add_plugins((
        MaterialPlugin::<TerrainMaterial>::default(),
        MaterialPlugin::<WaterMaterial>::default(),
        MaterialPlugin::<UrbanMaterial>::default(),
    ))
    .insert_resource(ClearColor(srgb(nature::HORIZON_COLOR)))
    .insert_resource(Sim::new(world))
    .insert_resource(Handles::default())
    .insert_resource(City::default())
    .insert_resource(Tour::new(tour))
    // `ScreenshotPlugin` est déjà fourni par `DefaultPlugins` en Bevy 0.20.
    .add_systems(Startup, setup)
    .add_systems(
        Update,
        (
            step_simulation,
            tour_step,
            camera_controls,
            follow_sky,
            adapt_fog,
            refresh_city,
            stream_forest,
            (auto_screenshot, screenshot_on_key).chain(),
        )
            .chain(),
    )
    .run();
}

/// Diagnostic de performance : `CITYMAYOR_DISABLE=arbres,ombres,eau,sol,msaa`
/// retire des éléments du rendu pour mesurer leur coût (avec le tour).
fn disabled(feature: &str) -> bool {
    std::env::var("CITYMAYOR_DISABLE").is_ok_and(|list| list.split(',').any(|f| f.trim() == feature))
}

fn srgb(c: [f32; 3]) -> Color {
    Color::srgb(c[0], c[1], c[2])
}

/// État de simulation possédé par la couche de présentation.
#[derive(Resource)]
struct Sim {
    world: World,
    clock: FixedStepClock,
}

impl Sim {
    fn new(world: World) -> Self {
        Self {
            world,
            clock: FixedStepClock::from_hz(20),
        }
    }

    /// Ce qui, en changeant, impose de reconstruire la géométrie de la ville.
    fn current_signature(&self) -> (u32, u32, u32) {
        let zoning = self.world.zoning();
        let zoned = zoning.count(ZoneType::Residential)
            + zoning.count(ZoneType::Commercial)
            + zoning.count(ZoneType::Industrial);
        (
            self.world.buildings().live_count(),
            self.world.roads().live_segments(),
            zoned,
        )
    }
}

#[derive(Resource, Default)]
struct Handles {
    roads: Handle<Mesh>,
    urban: Handle<UrbanMaterial>,
}

/// Côté d'un morceau de ville (tuiles) : un maillage par morceau, reconstruit
/// seulement quand ses bâtiments changent.
const CITY_CHUNK_TILES: u32 = 16;

/// Morceaux de ville construits : empreinte des bâtiments, entité, maillage.
#[derive(Resource, Default)]
struct City {
    roads: urban::RoadIndex,
    road_segments: u32,
    chunks: HashMap<(u32, u32), CityChunk>,
}

/// Empreinte des bâtiments d'un morceau, et son entité si elle a de la géométrie.
type CityChunk = (u64, Option<(Entity, Handle<Mesh>)>);

impl City {
    /// Reconstruit la voirie si le réseau a changé, puis les morceaux dont les
    /// bâtiments (ou la voirie) ont changé. Renvoie les rectangles (m, XZ)
    /// reconstruits : les arbres de la ville n'y sont réévalués que là.
    fn refresh(
        &mut self,
        world: &World,
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        handles: &Handles,
    ) -> Vec<[f32; 4]> {
        let mut changed = Vec::new();
        let size = world.config().tile_size_m as f32;
        let segments = world.roads().live_segments();
        let roads_changed = segments != self.road_segments || self.chunks.is_empty();
        if roads_changed {
            let strips = scene::roads(world);
            self.roads = urban::RoadIndex::new(&strips);
            self.road_segments = segments;
            if let Some(mut mesh) = meshes.get_mut(&handles.roads) {
                *mesh = urban_mesh(urban::road_mesh(world, &strips), RenderAssetUsages::default());
            }
        }
        let grid = world.zoning().grid();
        let (w, h) = (grid.width(), grid.height());
        for cy in (0..h).step_by(CITY_CHUNK_TILES as usize) {
            for cx in (0..w).step_by(CITY_CHUNK_TILES as usize) {
                // Une tuile de marge : la parcelle dépend des voisines.
                let signature = urban::buildings_signature(
                    world,
                    cx.saturating_sub(1),
                    cy.saturating_sub(1),
                    cx + CITY_CHUNK_TILES + 1,
                    cy + CITY_CHUNK_TILES + 1,
                );
                let entry = self.chunks.entry((cx, cy)).or_insert((u64::MAX, None));
                if entry.0 == signature && !roads_changed {
                    continue;
                }
                entry.0 = signature;
                // Marge d'une tuile : les arbres voisins dépendent des parcelles.
                changed.push([
                    cx as f32 * size - size,
                    cy as f32 * size - size,
                    (cx + CITY_CHUNK_TILES) as f32 * size + size,
                    (cy + CITY_CHUNK_TILES) as f32 * size + size,
                ]);
                let data = urban::buildings_in(world, &self.roads, cx, cy, cx + CITY_CHUNK_TILES, cy + CITY_CHUNK_TILES);
                if let Some((entity, handle)) = entry.1.take() {
                    commands.entity(entity).despawn();
                    meshes.remove(&handle);
                }
                if data.is_empty() {
                    continue;
                }
                let handle = meshes.add(urban_mesh(data, RenderAssetUsages::RENDER_WORLD));
                let entity = commands
                    .spawn((Mesh3d(handle.clone()), MeshMaterial3d(handles.urban.clone()), Transform::default()))
                    .id();
                entry.1 = Some((entity, handle));
            }
        }
        changed
    }
}

fn urban_mesh(data: urban::UrbanMesh, usage: RenderAssetUsages) -> Mesh {
    mesh(data.positions, data.normals, Some(data.colors), Some(data.uvs), data.indices, usage)
}

/// Niveaux construits à la demande près de la caméra (`stream_forest`).
const STREAMED_LODS: [nature::TreeLod; 2] = [nature::TreeLod::Model, nature::TreeLod::Reduced];

/// Plage de visibilité (mètres caméra → origine de la cellule) d'un niveau.
fn tree_lod_range(lod: nature::TreeLod) -> VisibilityRange {
    let ((s0, s1), (e0, e1)) = nature::tree_lod_distances(lod);
    VisibilityRange { start_margin: s0..s1, end_margin: e0..e1, use_aabb: false }
}

fn spawn_tree_entity(
    commands: &mut Commands,
    mesh: Handle<Mesh>,
    material: &Handle<StandardMaterial>,
    lod: nature::TreeLod,
    origin: Vec3,
) -> Entity {
    let mut entity = commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(material.clone()),
        Transform::from_translation(origin),
        tree_lod_range(lod),
    ));
    // Les silhouettes lointaines ne portent pas d'ombre (hors cascades).
    if lod == nature::TreeLod::Silhouette {
        entity.insert(bevy::light::NotShadowCaster);
    }
    entity.id()
}

/// Toutes les forêts (décor et ville), par cellule.
///
/// Les silhouettes existent en permanence ; le modèle complet et le modèle
/// allégé sont construits quand la caméra s'approche et libérés quand elle
/// s'éloigne (`nature::streamed_lod_wanted`). Construire le modèle complet
/// partout saturait la mémoire.
#[derive(Resource)]
struct Forest {
    cells: Vec<ForestCell>,
    models: nature::TreeModels,
    material: Handle<StandardMaterial>,
}

struct ForestCell {
    /// Ville : chaque arbre garde sa tuile, pour céder la place aux bâtiments.
    spots: Vec<(nature::TreeSpot, Option<sim_core::TileId>)>,
    /// Arbres actuellement présents (décor : tous).
    current: Vec<nature::TreeSpot>,
    origin: [f32; 3],
    silhouette: Handle<Mesh>,
    /// Entité et mesh construits pour chaque niveau de `STREAMED_LODS`.
    streamed: [Option<(Entity, Handle<Mesh>)>; 2],
}

impl ForestCell {
    fn is_city(&self) -> bool {
        self.spots.first().is_some_and(|(_, tile)| tile.is_some())
    }
}

/// Caméra orbitale amortie et bornée.
#[derive(Component, Debug)]
struct OrbitCamera {
    focus: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
    /// Cibles : l'entrée modifie la cible, la caméra la rejoint en douceur.
    target_focus: Vec3,
    target_yaw: f32,
    target_pitch: f32,
    target_distance: f32,
    bounds: nature::CameraBounds,
}

#[derive(Component)]
struct SkyDome;

fn mesh(
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Option<Vec<[f32; 4]>>,
    uvs: Option<Vec<[f32; 2]>>,
    indices: Vec<u32>,
    usage: RenderAssetUsages,
) -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, usage);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    if let Some(colors) = colors {
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    }
    if let Some(uvs) = uvs {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    }
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// Tableau de textures des six couches du sol.
fn terrain_layers() -> Image {
    const SOURCES: [&[u8]; nature::LAYER_COUNT] = [
        include_bytes!("../assets/textures/0_prairie.jpg"),
        include_bytes!("../assets/textures/1_herbe_seche.jpg"),
        include_bytes!("../assets/textures/2_sous_bois.jpg"),
        include_bytes!("../assets/textures/3_roche.jpg"),
        include_bytes!("../assets/textures/4_sable.jpg"),
        include_bytes!("../assets/textures/5_neige.jpg"),
    ];
    texture_array("terrain_layers", &SOURCES, &nature::LAYER_NAMES)
}

/// Tableau de textures urbaines (façades, toits, voirie), rendues par
/// `tools/blender/make_buildings.py`.
fn urban_layers() -> Image {
    macro_rules! layers {
        ($($name:literal),* $(,)?) => {
            [$(include_bytes!(concat!("../assets/urban/", $name, ".jpg")).as_slice()),*]
        };
    }
    const SOURCES: [&[u8]; urban::LAYER_COUNT] = layers!(
        "00_brique_fenetres", "01_brique", "02_enduit_fenetres", "03_enduit",
        "04_bardage_fenetres", "05_bardage", "06_immeuble", "07_vitrine", "08_bureaux",
        "09_industrie", "10_tuiles", "11_ardoises", "12_toit_tole", "13_toit_plat",
        "14_route_2_voies", "15_route_4_voies", "16_trottoir", "17_beton", "18_pelouse",
        "19_porte",
    );
    texture_array("urban_layers", &SOURCES, &urban::LAYER_NAMES)
}

/// Tableau de textures JPEG 1024², mipmaps calculés au chargement.
fn texture_array(label: &'static str, sources: &[&[u8]], names: &[&str]) -> Image {
    const SIZE: u32 = 1024;
    let mip_levels = SIZE.ilog2() + 1;
    let mut data = Vec::new();
    for (index, bytes) in sources.iter().enumerate() {
        let decoded = Image::from_buffer(
            bytes,
            ImageType::Extension("jpg"),
            CompressedImageFormats::NONE,
            true,
            ImageSampler::Default,
            RenderAssetUsages::RENDER_WORLD,
        )
        .unwrap_or_else(|e| panic!("texture {} illisible : {e}", names[index]));
        let rgba = decoded
            .convert(TextureFormat::Rgba8UnormSrgb)
            .expect("conversion RGBA8");
        let mut level = rgba.data.expect("pixels");
        assert_eq!(level.len(), (SIZE * SIZE * 4) as usize, "les couches doivent faire 1024²");
        let mut size = SIZE;
        data.extend_from_slice(&level);
        while size > 1 {
            level = downsample(&level, size);
            size /= 2;
            data.extend_from_slice(&level);
        }
    }

    Image {
        data: Some(data),
        texture_descriptor: TextureDescriptor {
            label: Some(label),
            size: Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: sources.len() as u32,
            },
            mip_level_count: mip_levels,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        },
        texture_view_descriptor: Some(TextureViewDescriptor {
            dimension: Some(TextureViewDimension::D2Array),
            ..default()
        }),
        sampler: ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::Repeat,
            address_mode_v: ImageAddressMode::Repeat,
            address_mode_w: ImageAddressMode::Repeat,
            mag_filter: ImageFilterMode::Linear,
            min_filter: ImageFilterMode::Linear,
            mipmap_filter: ImageFilterMode::Linear,
            anisotropy_clamp: 16,
            ..default()
        }),
        asset_usage: RenderAssetUsages::RENDER_WORLD,
        ..default()
    }
}

/// Atlas des arbres avec mipmaps qui préservent la couverture du feuillage
/// (`nature::coverage_preserving_mips`) : sans elles, les arbres lointains
/// deviendraient transparents.
fn tree_atlas() -> Image {
    let decoded = Image::from_buffer(
        nature::TREE_ATLAS_PNG,
        ImageType::Extension("png"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::Default,
        RenderAssetUsages::RENDER_WORLD,
    )
    .expect("atlas des arbres illisible");
    let size = decoded.width();
    let rgba = decoded.convert(TextureFormat::Rgba8UnormSrgb).expect("conversion RGBA8");
    let levels = nature::coverage_preserving_mips(
        rgba.data.as_deref().expect("pixels"),
        size,
        nature::TREE_ATLAS_CELL,
        nature::TREE_ALPHA_CUTOFF,
    );
    let mip_level_count = levels.len() as u32;
    Image {
        data: Some(levels.concat()),
        texture_descriptor: TextureDescriptor {
            label: Some("tree_atlas"),
            size: Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            mip_level_count,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        },
        sampler: ImageSampler::Descriptor(ImageSamplerDescriptor {
            mag_filter: ImageFilterMode::Linear,
            min_filter: ImageFilterMode::Linear,
            mipmap_filter: ImageFilterMode::Linear,
            anisotropy_clamp: 8,
            ..default()
        }),
        asset_usage: RenderAssetUsages::RENDER_WORLD,
        ..default()
    }
}

/// Réduction 2×2 (boîte) d'une image RGBA8 carrée.
fn downsample(pixels: &[u8], size: u32) -> Vec<u8> {
    let half = (size / 2).max(1);
    let mut out = vec![0u8; (half * half * 4) as usize];
    for y in 0..half {
        for x in 0..half {
            for c in 0..4 {
                let at = |dx: u32, dy: u32| u32::from(pixels[(((y * 2 + dy) * size + x * 2 + dx) * 4 + c) as usize]);
                let sum = at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1);
                out[((y * half + x) * 4 + c) as usize] = ((sum + 2) / 4) as u8;
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut terrain_materials: ResMut<Assets<TerrainMaterial>>,
    mut water_materials: ResMut<Assets<WaterMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut urban_materials: ResMut<Assets<UrbanMaterial>>,
    sim: Res<Sim>,
    mut handles: ResMut<Handles>,
    mut city: ResMut<City>,
) {
    let world = &sim.world;
    let static_usage = RenderAssetUsages::RENDER_WORLD;

    // --- sol : relief généré, texturé, jusqu'à l'horizon
    let s = nature::LAYER_SCALE_M;
    let terrain_mat = terrain_materials.add(TerrainMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 0.92,
            ..default()
        },
        extension: TerrainExtension {
            layers: images.add(terrain_layers()),
            scales_a: Vec4::new(s[0], s[1], s[2], s[3]),
            scales_b: Vec4::new(s[4], s[5], 0.0, 0.0),
        },
    });
    for chunk in nature::ground_chunks(world, 96).into_iter().filter(|_| !disabled("sol")) {
        let handle = meshes.add(mesh(
            chunk.positions,
            chunk.normals,
            Some(chunk.colors),
            Some(chunk.uvs),
            chunk.indices,
            static_usage,
        ));
        commands.spawn((Mesh3d(handle), MeshMaterial3d(terrain_mat.clone()), Transform::default()));
    }

    // --- eau : un plan au niveau de la mer ; le relief dessine fleuve et lacs
    let (water_positions, water_indices) = nature::water_plane(world, 96);
    let water_normals = vec![[0.0, 1.0, 0.0]; water_positions.len()];
    let water_mat = water_materials.add(WaterMaterial {
        base: StandardMaterial {
            base_color: Color::srgba(0.04, 0.16, 0.21, 0.9),
            perceptual_roughness: 0.06,
            reflectance: 0.6,
            alpha_mode: AlphaMode::Blend,
            ..default()
        },
        extension: WaterExtension {
            waves: Vec4::new(0.16, 1.0, 450.0, 0.0),
        },
    });
    if !disabled("eau") {
    commands.spawn((
        Mesh3d(meshes.add(mesh(water_positions, water_normals, None, None, water_indices, static_usage))),
        MeshMaterial3d(water_mat),
        Transform::default(),
        bevy::light::NotShadowCaster,
    ));
    }

    // --- la ville (reconstruite par morceaux quand elle change)
    handles.urban = urban_materials.add(UrbanMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 0.8,
            reflectance: 0.3,
            ..default()
        },
        extension: UrbanExtension { layers: images.add(urban_layers()) },
    });
    handles.roads = meshes.add(urban_mesh(urban::UrbanMesh::default(), RenderAssetUsages::default()));
    commands.spawn((Mesh3d(handles.roads.clone()), MeshMaterial3d(handles.urban.clone()), Transform::default()));
    city.refresh(world, &mut commands, &mut meshes, &handles);

    // --- forêts du décor (une entité par cellule de 1 km : culling)
    // Arbres : un seul matériau, l'atlas (cartes de feuillage, écorces,
    // imposteurs) détouré par son alpha. Pas de « double_sided » : les deux
    // faces d'une carte gardent la même normale de canopée.
    let tree_mat = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(images.add(tree_atlas())),
        alpha_mode: AlphaMode::Mask(nature::TREE_ALPHA_CUTOFF),
        cull_mode: None,
        double_sided: false,
        perceptual_roughness: 0.85,
        reflectance: 0.2,
        ..default()
    });
    // Forêts : silhouettes permanentes ici ; modèle complet et allégé à la
    // demande (`stream_forest`). Les trois niveaux d'une cellule partagent
    // la même origine, pour des fondus (`VisibilityRange`) exacts.
    let models = nature::TreeModels::embedded();
    let mut forest = Forest { cells: Vec::new(), models, material: tree_mat.clone() };
    let mut cells: Vec<Vec<(nature::TreeSpot, Option<sim_core::TileId>)>> = Vec::new();
    if !disabled("arbres") {
        cells.extend(
            nature::decor_tree_cells(world, 500.0)
                .into_iter()
                .map(|c| c.into_iter().map(|s| (s, None)).collect()),
        );
    }
    // Zone jouable : arbres naturels + alignements le long des rues, rangés
    // dans les mêmes cellules de 256 m.
    let mut play_cells = nature::play_area_tree_cells(world, 256.0);
    let columns = (nature::play_size_m(world).0 / 256.0).ceil() as usize;
    for (spot, tile) in urban::street_tree_spots(world, &scene::roads(world)) {
        let index = (spot.base[2] / 256.0) as usize * columns + (spot.base[0] / 256.0) as usize;
        if let Some(cell) = play_cells.get_mut(index) {
            cell.push((spot, tile));
        }
    }
    cells.extend(play_cells.into_iter().map(|c| c.into_iter().map(|(s, t)| (s, Some(t))).collect()));
    for spots in cells {
        let all: Vec<_> = spots.iter().map(|(s, _)| *s).collect();
        let origin = nature::spots_center(&all);
        let in_city = spots.first().is_some_and(|(_, t)| t.is_some());
        let current = if in_city { city_spots(world, &city.roads, &spots) } else { all };
        let chunk = nature::build_trees(&current, nature::TreeLod::Silhouette, &forest.models, origin);
        // Ville : mesh modifiable (la ville empiète) ; décor : GPU seulement.
        let silhouette = meshes.add(if in_city { tree_mesh(chunk) } else { static_tree_mesh(chunk) });
        spawn_tree_entity(&mut commands, silhouette.clone(), &tree_mat, nature::TreeLod::Silhouette, Vec3::from(origin));
        forest.cells.push(ForestCell { spots, current, origin, silhouette, streamed: [None, None] });
    }
    commands.insert_resource(forest);

    // --- ciel : dôme dégradé qui suit la caméra, hors brouillard
    let (sky_positions, sky_colors, sky_indices) = nature::sky_dome(24, 48);
    let sky_normals = sky_positions.iter().map(|p| [-p[0], -p[1], -p[2]]).collect();
    commands.spawn((
        Mesh3d(meshes.add(mesh(sky_positions, sky_normals, Some(sky_colors), None, sky_indices, static_usage))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::WHITE,
            unlit: true,
            fog_enabled: false,
            cull_mode: None,
            ..default()
        })),
        Transform::from_scale(Vec3::splat(nature::SKY_RADIUS_M)),
        bevy::light::NotShadowCaster,
        bevy::light::NotShadowReceiver,
        SkyDome,
    ));

    // --- soleil : assez bas pour sculpter le relief, ombres en cascade
    commands.spawn((
        DirectionalLight {
            illuminance: 11_000.0,
            color: Color::srgb(1.0, 0.96, 0.88),
            shadow_maps_enabled: !disabled("ombres"),
            ..default()
        },
        Transform::from_xyz(-0.55, 0.62, 0.45).looking_at(Vec3::ZERO, Vec3::Y),
        CascadeShadowConfigBuilder {
            num_cascades: 4,
            first_cascade_far_bound: 180.0,
            maximum_distance: 2_600.0,
            ..default()
        }
        .build(),
    ));
    commands.insert_resource(GlobalAmbientLight {
        color: srgb(nature::HORIZON_COLOR),
        brightness: 380.0,
        affects_lightmapped_meshes: false,
    });

    // --- caméra
    let bounds = nature::camera_bounds(world);
    let center = scene::map_center(world);
    let focus = Vec3::new(center[0], nature::ground_altitude_m(world, center[0], center[2]), center[2]);
    let orbit = OrbitCamera {
        focus,
        yaw: 0.8,
        pitch: 0.42,
        distance: 1_500.0,
        target_focus: focus,
        target_yaw: 0.8,
        target_pitch: 0.42,
        target_distance: 1_500.0,
        bounds,
    };
    let transform = orbit_transform(&orbit, world);
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: nature::CAMERA_FOV,
            near: 1.0,
            far: nature::SKY_RADIUS_M * 1.5,
            ..default()
        }),
        transform,
        DistanceFog {
            color: srgb(nature::HORIZON_COLOR),
            directional_light_color: Color::srgba(1.0, 0.92, 0.75, 0.45),
            directional_light_exponent: 24.0,
            falloff: FogFalloff::from_visibility_colors(
                nature::FOG_VISIBILITY_M,
                Color::srgb(0.55, 0.62, 0.72),
                srgb(nature::HORIZON_COLOR),
            ),
        },
        orbit,
        if disabled("msaa") { Msaa::Off } else { Msaa::default() },
    ));
}

fn step_simulation(time: Res<Time>, mut sim: ResMut<Sim>) {
    let batch = sim.clock.advance(time.delta());
    for _ in 0..batch.ticks {
        sim.world.tick();
    }
}

#[allow(clippy::too_many_arguments)]
fn camera_controls(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    time: Res<Time>,
    sim: Res<Sim>,
    tour: Res<Tour>,
    mut query: Query<(&mut Transform, &mut OrbitCamera)>,
) {
    let dt = time.delta_secs().min(0.1);
    for (mut transform, mut camera) in &mut query {
        if !tour.active {
            let rotate = 1.6 * dt;
            let pan = camera.target_distance.max(60.0) * 0.9 * dt;
            if keys.pressed(KeyCode::KeyQ) {
                camera.target_yaw += rotate;
            }
            if keys.pressed(KeyCode::KeyE) {
                camera.target_yaw -= rotate;
            }
            if keys.pressed(KeyCode::KeyR) {
                camera.target_pitch += rotate;
            }
            if keys.pressed(KeyCode::KeyF) {
                camera.target_pitch -= rotate;
            }
            let mut zoom = 1.0;
            if keys.pressed(KeyCode::KeyZ) || keys.pressed(KeyCode::Equal) {
                zoom *= 1.0 - 1.6 * dt;
            }
            if keys.pressed(KeyCode::KeyX) || keys.pressed(KeyCode::Minus) {
                zoom *= 1.0 + 1.6 * dt;
            }
            // Molette : un cran = 12 %.
            zoom *= 0.88f32.powf(scroll.delta.y);
            camera.target_distance *= zoom;

            let forward = Vec3::new(-camera.target_yaw.sin(), 0.0, -camera.target_yaw.cos());
            let right = Vec3::new(-forward.z, 0.0, forward.x);
            let mut shift = Vec3::ZERO;
            if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
                shift += forward;
            }
            if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
                shift -= forward;
            }
            if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
                shift += right;
            }
            if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
                shift -= right;
            }
            camera.target_focus += shift * pan;

            let delta = motion.delta;
            if buttons.pressed(MouseButton::Right) {
                camera.target_yaw -= delta.x * 0.005;
                camera.target_pitch += delta.y * 0.004;
            }
            if buttons.pressed(MouseButton::Middle) || buttons.pressed(MouseButton::Left) {
                // Glisser = attraper le sol : le déplacement suit la souris,
                // proportionnel à la distance.
                let scale = camera.target_distance * 0.0016;
                camera.target_focus += (-right * delta.x + forward * delta.y) * scale;
            }
        }

        // Bornes : on ne quitte pas sa ville.
        let b = camera.bounds;
        camera.target_pitch = camera.target_pitch.clamp(b.min_pitch, b.max_pitch);
        camera.target_distance = camera.target_distance.clamp(b.min_distance, b.max_distance);
        let (fx, fz) = b.clamp_focus(camera.target_focus.x, camera.target_focus.z);
        camera.target_focus = Vec3::new(fx, nature::ground_altitude_m(&sim.world, fx, fz).max(0.0), fz);

        // Amortissement exponentiel, indépendant du framerate.
        let k = 1.0 - (-10.0 * dt).exp();
        let target = (camera.target_focus, camera.target_yaw, camera.target_pitch, camera.target_distance);
        camera.focus = camera.focus.lerp(target.0, k);
        camera.yaw += (target.1 - camera.yaw) * k;
        camera.pitch += (target.2 - camera.pitch) * k;
        camera.distance += (target.3 - camera.distance) * k;
        *transform = orbit_transform(&camera, &sim.world);
    }
}

/// Position de l'œil, jamais sous le relief (collines du décor, berges).
fn orbit_transform(camera: &OrbitCamera, world: &World) -> Transform {
    let horizontal = camera.distance * camera.pitch.cos();
    let offset = Vec3::new(
        horizontal * camera.yaw.sin(),
        camera.distance * camera.pitch.sin(),
        horizontal * camera.yaw.cos(),
    );
    let mut eye = camera.focus + offset;
    let ground = nature::ground_altitude_m(world, eye.x, eye.z).max(0.0);
    eye.y = eye.y.max(ground + camera.bounds.ground_clearance);
    Transform::from_translation(eye).looking_at(camera.focus, Vec3::Y)
}

/// Le brouillard s'éclaircit quand la caméra plonge (voir `fog_visibility_m`).
fn adapt_fog(mut cameras: Query<(&OrbitCamera, &mut DistanceFog)>) {
    for (camera, mut fog) in &mut cameras {
        fog.falloff = FogFalloff::from_visibility_colors(
            nature::fog_visibility_m(camera.pitch, nature::CAMERA_FOV),
            Color::srgb(0.55, 0.62, 0.72),
            srgb(nature::HORIZON_COLOR),
        );
    }
}

fn follow_sky(
    camera: Query<&Transform, (With<Camera3d>, Without<SkyDome>)>,
    mut sky: Query<&mut Transform, With<SkyDome>>,
) {
    let Ok(camera) = camera.single() else {
        return;
    };
    for mut transform in &mut sky {
        transform.translation = camera.translation;
    }
}

/// Reconstruit la géométrie de la ville quand elle change, au plus deux fois
/// par seconde : pendant la croissance, la signature change presque à chaque
/// tick, et tout refaire à chaque image coûtait ~25 ms (mesuré).
#[allow(clippy::too_many_arguments)] // système Bevy : chaque ressource est un paramètre
fn refresh_city(
    sim: Res<Sim>,
    time: Res<Time>,
    mut meshes: ResMut<Assets<Mesh>>,
    handles: Res<Handles>,
    mut city: ResMut<City>,
    mut forest: ResMut<Forest>,
    mut commands: Commands,
    mut built_signature: Local<Option<(u32, u32, u32)>>,
    mut since_last: Local<f32>,
) {
    *since_last += time.delta_secs();
    let signature = sim.current_signature();
    if *built_signature == Some(signature) || (built_signature.is_some() && *since_last < 0.5) {
        return;
    }
    *built_signature = Some(signature);
    *since_last = 0.0;
    let world = &sim.world;
    let changed = city.refresh(world, &mut commands, &mut meshes, &handles);
    // Forêt : seules les cellules de la ville dont les arbres ont changé. Les
    // niveaux streamés sont libérés ; `stream_forest` les reconstruit.
    let forest = &mut *forest;
    let touched = |cell: &ForestCell| {
        cell.spots.iter().any(|(s, _)| {
            changed.iter().any(|r| s.base[0] >= r[0] && s.base[0] < r[2] && s.base[2] >= r[1] && s.base[2] < r[3])
        })
    };
    for cell in forest.cells.iter_mut().filter(|c| c.is_city() && touched(c)) {
        let spots = city_spots(world, &city.roads, &cell.spots);
        if spots.len() == cell.current.len() {
            continue;
        }
        cell.current = spots;
        if let Some(mut mesh) = meshes.get_mut(&cell.silhouette) {
            *mesh = tree_mesh(nature::build_trees(&cell.current, nature::TreeLod::Silhouette, &forest.models, cell.origin));
        }
        for slot in &mut cell.streamed {
            if let Some((entity, handle)) = slot.take() {
                commands.entity(entity).despawn();
                meshes.remove(&handle);
            }
        }
    }
}

/// Arbres d'une cellule de la ville qui ont encore leur place : terrains
/// libres, accotements, jardins de derrière (`urban::tree_allowed`).
fn city_spots(
    world: &World,
    roads: &urban::RoadIndex,
    spots: &[(nature::TreeSpot, Option<sim_core::TileId>)],
) -> Vec<nature::TreeSpot> {
    spots
        .iter()
        .filter_map(|(s, t)| t.map(|t| (*s, t)))
        .filter(|(s, t)| urban::tree_allowed(world, roads, s.base[0], s.base[2], *t))
        .map(|(s, _)| s)
        .collect()
}

/// Construit/libère le modèle complet et le modèle allégé des cellules selon
/// la distance à la caméra, avec un budget d'arbres par passage pour ne pas
/// figer l'image quand la caméra saute.
fn stream_forest(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut forest: ResMut<Forest>,
    cameras: Query<&Transform, With<Camera3d>>,
    time: Res<Time>,
    mut since_last: Local<f32>,
    mut warm: Local<bool>,
) {
    *since_last += time.delta_secs();
    if *warm && *since_last < 0.2 {
        return;
    }
    *since_last = 0.0;
    let Ok(camera) = cameras.single() else {
        return;
    };
    let eye = camera.translation;
    // Premier passage sans limite : l'image initiale est complète.
    let mut budget: usize = if *warm { 40_000 } else { usize::MAX };
    *warm = true;
    let forest = &mut *forest;
    let mut order: Vec<(f32, usize)> = forest
        .cells
        .iter()
        .enumerate()
        .map(|(i, c)| (eye.distance(Vec3::from(c.origin)), i))
        .collect();
    // Les plus proches d'abord : c'est là qu'un manque se verrait.
    order.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (distance, index) in order {
        let cell = &mut forest.cells[index];
        for (slot, lod) in STREAMED_LODS.iter().enumerate() {
            let built = cell.streamed[slot].is_some();
            let wanted = nature::streamed_lod_wanted(*lod, distance, built);
            if wanted && !built && budget >= cell.current.len() {
                budget -= cell.current.len();
                let chunk = nature::build_trees(&cell.current, *lod, &forest.models, cell.origin);
                let handle = meshes.add(static_tree_mesh(chunk));
                let entity = spawn_tree_entity(&mut commands, handle.clone(), &forest.material, *lod, Vec3::from(cell.origin));
                cell.streamed[slot] = Some((entity, handle));
            } else if !wanted && built {
                if let Some((entity, handle)) = cell.streamed[slot].take() {
                    commands.entity(entity).despawn();
                    meshes.remove(&handle);
                }
            }
        }
    }
}

/// Mesh d'arbres qui ne changera plus : seule la copie GPU est gardée (la
/// copie CPU du monde principal est libérée après téléversement).
fn static_tree_mesh(chunk: nature::TreeChunk) -> Mesh {
    mesh(chunk.positions, chunk.normals, Some(chunk.colors), Some(chunk.uvs), chunk.indices, RenderAssetUsages::RENDER_WORLD)
}

fn tree_mesh(chunk: nature::TreeChunk) -> Mesh {
    mesh(
        chunk.positions,
        chunk.normals,
        Some(chunk.colors),
        Some(chunk.uvs),
        chunk.indices,
        RenderAssetUsages::default(),
    )
}

// ------------------------------------------------------------- tour de captures

/// Tour de vérification : une suite de points de vue capturés puis on quitte.
#[derive(Resource)]
struct Tour {
    active: bool,
    step: usize,
    elapsed: f32,
    shot_taken: bool,
    /// Images rendues et temps cumulé, pour la mesure de performance.
    frames: u32,
    frame_time: f32,
}

impl Tour {
    fn new(active: bool) -> Self {
        Self {
            active,
            step: 0,
            elapsed: 0.0,
            shot_taken: false,
            frames: 0,
            frame_time: 0.0,
        }
    }
}

/// (nom, point visé relatif au centre (x, z), lacet, inclinaison, distance).
const TOUR_VIEWS: [(&str, f32, f32, f32, f32, f32); 10] = [
    ("ville", 0.0, 0.0, 0.8, 0.55, 900.0),
    ("rue", -0.2, -0.3, 0.6, 0.30, 70.0),
    ("quartier", -0.2, -0.3, 1.2, 0.55, 260.0),
    ("berge", 0.02, 0.18, 2.0, 0.32, 160.0),
    ("montagne", -0.6, -0.6, 2.4, 0.16, 900.0),
    ("fleuve", 0.15, -0.1, 2.3, 0.30, 420.0),
    ("horizon", 0.0, 0.0, 0.8, 0.10, 2_400.0),
    ("bord-ouest", -0.62, 0.0, -1.57, 0.12, 3_200.0),
    ("coin", 0.62, 0.62, 0.78, 0.10, 3_200.0),
    ("aplomb", 0.0, 0.0, 0.0, 1.45, 3_200.0),
];

fn tour_step(
    mut commands: Commands,
    time: Res<Time>,
    sim: Res<Sim>,
    mut tour: ResMut<Tour>,
    mut cameras: Query<&mut OrbitCamera>,
    mut exit: MessageWriter<AppExit>,
) {
    if !tour.active {
        return;
    }
    let Some(&(name, fx, fz, yaw, pitch, distance)) = TOUR_VIEWS.get(tour.step) else {
        if tour.frames > 0 {
            let average = tour.frame_time / tour.frames as f32;
            println!(
                "tour : {} images, {:.1} ms/image en moyenne ({:.0} i/s)",
                tour.frames,
                average * 1_000.0,
                1.0 / average
            );
        }
        exit.write(AppExit::Success);
        return;
    };
    tour.elapsed += time.delta_secs();
    if tour.elapsed > 1.0 {
        // Mesure hors des sauts de caméra (reconstructions éventuelles).
        tour.frames += 1;
        tour.frame_time += time.delta_secs();
    }
    if tour.elapsed < 0.1 {
        let (w, h) = nature::play_size_m(&sim.world);
        for mut camera in &mut cameras {
            camera.target_focus = Vec3::new(w * (0.5 + fx), 0.0, h * (0.5 + fz));
            camera.target_yaw = yaw;
            camera.target_pitch = pitch;
            camera.target_distance = distance;
            // Saut direct : pas d'interpolation entre deux vues du tour.
            camera.focus = camera.target_focus;
            camera.yaw = yaw;
            camera.pitch = pitch;
            camera.distance = distance;
        }
    }
    if tour.elapsed > 2.5 && !tour.shot_taken {
        tour.shot_taken = true;
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(format!("citymayor-tour-{}-{name}.png", tour.step)));
    }
    if tour.elapsed > 3.5 {
        tour.step += 1;
        tour.elapsed = 0.0;
        tour.shot_taken = false;
    }
}

fn auto_screenshot(
    tour: Res<Tour>,
    mut commands: Commands,
    mut elapsed: Local<f32>,
    mut done: Local<bool>,
    time: Res<Time>,
) {
    if *done || tour.active {
        return;
    }
    *elapsed += time.delta_secs();
    if *elapsed > 6.0 {
        *done = true;
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk("citymayor-auto.png"));
    }
}

fn screenshot_on_key(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut counter: Local<u32>,
    time: Res<Time>,
) {
    if keys.just_pressed(KeyCode::F12) {
        *counter += 1;
        let name = format!("citymayor-{}-{}.png", time.elapsed_secs() as u64, *counter);
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(name));
    }
}
