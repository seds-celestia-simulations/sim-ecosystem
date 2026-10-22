mod camera;
mod creature;
mod food;
mod sim;
mod ui;

use bevy::prelude::*;

use bevy_egui::EguiPlugin;

pub const TILE_SZ: f32 = 32.0;

const GRID_SIZE: i32 = 128;

fn grid_to_world(p: IVec2) -> Vec2 {
    p.as_vec2() * TILE_SZ + Vec2::splat(TILE_SZ / 2.0)
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let mid = (GRID_SIZE - 1) as f32 / 2.0;
    let mid = Vec2::splat(mid);

    commands.spawn((Camera2d, Transform::from_translation(mid.extend(0.0))));

    let square = meshes.add(Rectangle::new(TILE_SZ, TILE_SZ));
    let light = materials.add(Color::srgb(0.72, 0.78, 0.60));
    let dark = materials.add(Color::srgb(0.62, 0.69, 0.50));

    for i in 0..GRID_SIZE {
        for j in 0..GRID_SIZE {
            let material = if (i + j) % 2 == 0 {
                light.clone()
            } else {
                dark.clone()
            };

            let p = grid_to_world(IVec2::new(i, j));

            commands.spawn((
                Mesh2d(square.clone()),
                MeshMaterial2d(material),
                Transform::from_translation(p.extend(0.0)),
            ));
        }
    }
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(ImagePlugin::default_nearest()))
        .add_plugins(EguiPlugin::default())
        .add_plugins(camera::CameraControlPlugin)
        .add_plugins(creature::CreaturePlugin)
        .add_plugins(food::FoodPlugin)
        .add_plugins(ui::UiPlugin)
        .add_systems(Startup, setup)
        .run();
}
