//! Viewer Bevy : la ville s'affiche, se pilote et se laisse regarder.
//!
//! # Contrat de cette couche
//! - **Aucune règle de jeu ici.** Le viewer lit `scene::*` (dérivé de `World`)
//!   et pousse des `timings` de tick. Si une règle de ville doit changer, elle
//!   change dans `sim-core`, jamais ici.
//! - **Géométrie statique reconstruite à la demande**, pas par frame : les
//!   bâtiments ne bougent pas, donc on ne re-téléverse un mesh que quand la
//!   ville change (signature `(bâtiments, segments, mois)`).
//! - **Rendu à fréquence libre, simulation à 20 Hz** : `FixedStepClock` absorbe
//!   le `dt` de la frame, comme le fera le jeu final. C'est ce qui autorise
//!   144 fps de caméra avec une simulation fixe.
//!
//! # Limites de ce premier jet (itération 2a)
//! - terrain = grille de sommets sans LOD ni normales lissées ;
//! - bâtiments = boîtes fusionnées en un seul mesh (pas encore d'instancing GPU
//!   ni de modèles) ;
//! - overlays non branchés, caméra pilotée au clavier (souris : molette + orbite
//!   à la prochaine étape, avec l'UI) ;
//! - pas d'ombres (choix de performance pour ce squelette).

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::render::mesh::{Indices, Mesh, PrimitiveTopology};

use sim_core::{FixedStepClock, World};

use crate::scene;

/// Lancée par le binaire `city-viewer`.
pub fn run() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "CityMayor — vertical slice".into(),
                resolution: (1400u32, 860u32).into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(Sim::new(scene::demo_city()))
        .insert_resource(Handles::default())
        // `ScreenshotPlugin` est déjà fourni par `DefaultPlugins` en Bevy 0.20.
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                step_simulation,
                camera_controls,
                refresh_meshes,
                (auto_screenshot, screenshot_on_key).chain(),
            )
                .chain(),
        )
        .run();
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

    fn current_signature(&self) -> (u32, u32, u64) {
        (
            self.world.buildings().live_count(),
            self.world.roads().live_segments(),
            self.world.snapshot().month,
        )
    }
}

#[derive(Resource, Default)]
struct Handles {
    terrain: Handle<Mesh>,
    buildings: Handle<Mesh>,
    roads: Handle<Mesh>,
}

/// Caméra orbitale amortie.
#[derive(Component, Debug)]
struct OrbitCamera {
    focus: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    sim: Res<Sim>,
    mut handles: ResMut<Handles>,
) {
    let center = scene::map_center(&sim.world);
    let focus = Vec3::new(center[0], center[1], center[2]);

    handles.terrain = meshes.add(terrain_mesh(&scene::terrain(&sim.world)));
    handles.buildings = meshes.add(buildings_mesh(&scene::buildings(&sim.world)));
    handles.roads = meshes.add(roads_mesh(&scene::roads(&sim.world)));

    let terrain_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.34, 0.44, 0.30),
        perceptual_roughness: 0.95,
        cull_mode: None,
        ..default()
    });
    let building_mat = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.75,
        cull_mode: None,
        ..default()
    });
    let road_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.16, 0.16, 0.18),
        perceptual_roughness: 0.9,
        cull_mode: None,
        ..default()
    });

    commands.spawn((
        Mesh3d(handles.terrain.clone()),
        MeshMaterial3d(terrain_mat),
        Transform::default(),
    ));
    commands.spawn((
        Mesh3d(handles.buildings.clone()),
        MeshMaterial3d(building_mat),
        Transform::default(),
    ));
    commands.spawn((
        Mesh3d(handles.roads.clone()),
        MeshMaterial3d(road_mat),
        Transform::default(),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(-1.0, 2.0, 1.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    // En Bevy 0.20, `AmbientLight` est une composante de caméra ; la ressource
    // globale s'appelle `GlobalAmbientLight`.
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.7, 0.8, 1.0),
        brightness: 420.0,
        affects_lightmapped_meshes: false,
    });

    let orbit = OrbitCamera {
        focus,
        yaw: 0.7,
        pitch: 0.55,
        distance: 900.0,
    };
    let transform = orbit_transform(&orbit);
    commands.spawn((Camera3d::default(), transform, orbit));
}

fn step_simulation(time: Res<Time>, mut sim: ResMut<Sim>) {
    let batch = sim.clock.advance(time.delta());
    for _ in 0..batch.ticks {
        sim.world.tick();
    }
}

fn camera_controls(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut query: Query<(&mut Transform, &mut OrbitCamera)>,
) {
    let dt = time.delta_secs();
    let mut changed = false;
    let mut zoom_in = false;
    let mut zoom_out = false;

    for (mut transform, mut camera) in &mut query {
        let rotate = 1.6 * dt;
        let pan = camera.distance.max(60.0) * 0.9 * dt;

        if keys.pressed(KeyCode::KeyQ) {
            camera.yaw += rotate;
            changed = true;
        }
        if keys.pressed(KeyCode::KeyE) {
            camera.yaw -= rotate;
            changed = true;
        }
        if keys.pressed(KeyCode::KeyR) {
            camera.pitch = (camera.pitch + rotate).min(1.45);
            changed = true;
        }
        if keys.pressed(KeyCode::KeyF) {
            camera.pitch = (camera.pitch - rotate).max(0.15);
            changed = true;
        }
        if keys.pressed(KeyCode::KeyZ) || keys.pressed(KeyCode::Equal) {
            zoom_in = true;
        }
        if keys.pressed(KeyCode::KeyX) || keys.pressed(KeyCode::Minus) {
            zoom_out = true;
        }

        let forward = Vec3::new(camera.yaw.sin(), 0.0, camera.yaw.cos());
        let right = Vec3::new(forward.z, 0.0, -forward.x);
        if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
            camera.focus += forward * pan;
            changed = true;
        }
        if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
            camera.focus -= forward * pan;
            changed = true;
        }
        if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
            camera.focus -= right * pan;
            changed = true;
        }
        if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
            camera.focus += right * pan;
            changed = true;
        }

        if changed || zoom_in || zoom_out {
            if zoom_in {
                camera.distance = (camera.distance * 0.92).max(25.0);
            }
            if zoom_out {
                camera.distance = (camera.distance * 1.08).min(2600.0);
            }
            *transform = orbit_transform(&camera);
        }
    }
}

fn orbit_transform(camera: &OrbitCamera) -> Transform {
    let horizontal = camera.distance * camera.pitch.cos();
    let offset = Vec3::new(
        horizontal * camera.yaw.sin(),
        camera.distance * camera.pitch.sin(),
        horizontal * camera.yaw.cos(),
    );
    Transform::from_translation(camera.focus + offset).looking_at(camera.focus, Vec3::Y)
}

fn refresh_meshes(
    sim: Res<Sim>,
    mut meshes: ResMut<Assets<Mesh>>,
    handles: Res<Handles>,
    mut built_signature: Local<Option<(u32, u32, u64)>>,
) {
    let signature = sim.current_signature();
    // `None` = premier passage : on reconstruit même si la signature n'a pas
    // bougé (les meshes initiaux sont vides, la ville n'a pas encore poussé).
    if *built_signature == Some(signature) {
        return;
    }
    *built_signature = Some(signature);

    if let Some(mut mesh) = meshes.get_mut(&handles.buildings) {
        *mesh = buildings_mesh(&scene::buildings(&sim.world));
    }
    if let Some(mut mesh) = meshes.get_mut(&handles.roads) {
        *mesh = roads_mesh(&scene::roads(&sim.world));
    }
}

/// Capture unique ~4 s après le lancement (pratique pour vérifier le rendu
/// sans interaction) puis à chaque F12. Les captures vont dans le dossier de
/// travail : `citymayor-*.png`.
fn auto_screenshot(
    mut commands: Commands,
    mut elapsed: Local<f32>,
    mut done: Local<bool>,
    time: Res<Time>,
) {
    if *done {
        return;
    }
    *elapsed += time.delta_secs();
    if *elapsed > 4.0 {
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

// ------------------------------------------------------------------- meshes

fn terrain_mesh(data: &scene::TerrainMeshData) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, data.positions.clone());
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, data.normals.clone());
    mesh.insert_indices(Indices::U32(data.indices.clone()));
    mesh
}

fn buildings_mesh(blocks: &[scene::BuildingBlock]) -> Mesh {
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(blocks.len() * 24);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(blocks.len() * 24);
    let mut colors: Vec<[f32; 4]> = Vec::with_capacity(blocks.len() * 24);
    let mut indices: Vec<u32> = Vec::with_capacity(blocks.len() * 36);

    for block in blocks {
        let color = scene::kind_color(block.kind);
        push_box(
            &mut positions,
            &mut normals,
            &mut colors,
            &mut indices,
            block.min,
            block.max,
            [color[0], color[1], color[2], 1.0],
        );
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

fn roads_mesh(strips: &[scene::RoadStrip]) -> Mesh {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for strip in strips {
        for pair in strip.points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let dir = [b[0] - a[0], b[2] - a[2]];
            let len = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
            if len < 1e-3 {
                continue;
            }
            let perp = [
                -dir[1] / len * strip.half_width_m,
                dir[0] / len * strip.half_width_m,
            ];
            let base = positions.len() as u32;
            let corners = [
                [a[0] - perp[0], a[1], a[2] - perp[1]],
                [a[0] + perp[0], a[1], a[2] + perp[1]],
                [b[0] + perp[0], b[1], b[2] + perp[1]],
                [b[0] - perp[0], b[1], b[2] - perp[1]],
            ];
            for corner in corners {
                positions.push(corner);
                normals.push([0.0, 1.0, 0.0]);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

fn push_box(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    colors: &mut Vec<[f32; 4]>,
    indices: &mut Vec<u32>,
    min: [f32; 3],
    max: [f32; 3],
    color: [f32; 4],
) {
    let corners = [
        [min[0], min[1], min[2]],
        [max[0], min[1], min[2]],
        [max[0], min[1], max[2]],
        [min[0], min[1], max[2]],
        [min[0], max[1], min[2]],
        [max[0], max[1], min[2]],
        [max[0], max[1], max[2]],
        [min[0], max[1], max[2]],
    ];
    // (quatre coins, normale) — le sens de winding n'a pas d'importance ici :
    // le matériau désactive le back-face culling.
    let faces: [([usize; 4], [f32; 3]); 6] = [
        ([4, 5, 6, 7], [0.0, 1.0, 0.0]),
        ([3, 2, 1, 0], [0.0, -1.0, 0.0]),
        ([0, 1, 5, 4], [0.0, 0.0, -1.0]),
        ([2, 3, 7, 6], [0.0, 0.0, 1.0]),
        ([3, 0, 4, 7], [-1.0, 0.0, 0.0]),
        ([1, 2, 6, 5], [1.0, 0.0, 0.0]),
    ];

    for (quad, normal) in faces {
        let base = positions.len() as u32;
        for corner in quad {
            positions.push(corners[corner]);
            normals.push(normal);
            colors.push(color);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}
