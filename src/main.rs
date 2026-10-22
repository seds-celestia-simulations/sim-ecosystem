mod camera;
mod creature;
mod food;
mod sim;
mod ui;
mod worldgen;

use bevy::prelude::*;

use bevy_egui::EguiPlugin;

pub const TILE_SZ: f32 = 32.0;

const GRID_SIZE: i32 = 128;

fn grid_to_world(p: IVec2) -> Vec2 {
    p.as_vec2() * TILE_SZ + Vec2::new(TILE_SZ / 2.0, TILE_SZ / 2.0)
}

fn setup(mut commands: Commands) {
    let mid = (GRID_SIZE - 1) as f32 / 2.0;
    commands.spawn((Camera2d, Transform::from_xyz(mid, mid, 0.0)));
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(ImagePlugin::default_nearest()))
        .add_plugins(EguiPlugin::default())
        .add_plugins(camera::CameraControlPlugin)
        .add_plugins(creature::CreaturePlugin)
        .add_plugins(food::FoodPlugin)
        .add_plugins(ui::UiPlugin)
        .add_plugins(worldgen::WorldGenPlugin)
        .add_systems(Startup, setup)
        .run();
}
