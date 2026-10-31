mod camera;
mod creature;
mod food;
mod graph;
mod ranged;
mod sim;
mod ui;
mod worldgen;

use bevy::prelude::*;

use bevy_egui::{EguiGlobalSettings, EguiPlugin, PrimaryEguiContext};

pub const TILE_SZ: f32 = 32.0;

const GRID_SIZE: i32 = 256;

fn grid_to_world(p: IVec2) -> Vec2 {
    p.as_vec2() * TILE_SZ + Vec2::splat(TILE_SZ / 2.0)
}

fn world_to_grid(p: Vec2) -> IVec2 {
    (p / TILE_SZ).floor().as_ivec2()
}

fn setup(mut commands: Commands, mut egui_settings: ResMut<EguiGlobalSettings>) {
    egui_settings.auto_create_primary_context = false;
    let mid = GRID_SIZE as f32 * TILE_SZ / 2.0;
    commands.spawn((
        Camera2d,
        PrimaryEguiContext,
        Transform::from_xyz(mid, mid, 0.0).with_scale(Vec3::splat(2.0)),
    ));
}

fn main() {
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(ImagePlugin::default_nearest())
                .set(WindowPlugin {
                    exit_condition: bevy::window::ExitCondition::OnPrimaryClosed,
                    ..default()
                }),
        )
        .add_plugins(EguiPlugin::default())
        .add_plugins(camera::CameraControlPlugin)
        .add_plugins(creature::CreaturePlugin)
        .add_plugins(food::FoodPlugin)
        .add_plugins(ui::UiPlugin)
        .add_plugins(graph::GraphPlugin)
        .add_plugins(worldgen::WorldGenPlugin)
        .add_systems(Startup, setup)
        .run();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_positions_map_to_the_tile_containing_them() {
        for cell in [IVec2::ZERO, IVec2::new(12, 45), IVec2::splat(GRID_SIZE - 1)] {
            for offset in [Vec2::ZERO, Vec2::splat(0.1), Vec2::splat(TILE_SZ - 0.1)] {
                assert_eq!(world_to_grid(cell.as_vec2() * TILE_SZ + offset), cell);
            }
            assert_eq!(world_to_grid(grid_to_world(cell)), cell);
        }
        assert_eq!(world_to_grid(Vec2::splat(-0.1)), IVec2::NEG_ONE);
        assert_eq!(
            world_to_grid(Vec2::splat(GRID_SIZE as f32 * TILE_SZ)),
            IVec2::splat(GRID_SIZE)
        );
    }
}
