use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::creature::{Creature, WorldPos};

use crate::TILE_SZ;

const ZOOM_MIN: f32 = 0.25;
const ZOOM_MAX: f32 = 4.0;
const ZOOM_PER_LINE: f32 = 0.1;
const ZOOM_PER_PIXEL: f32 = 0.002;

const BODY_CENTER_Y: f32 = TILE_SZ * 0.25;
const PICK_RADIUS: f32 = TILE_SZ;

/* Lock the camera to a creature */
#[derive(Resource, Default)]
pub struct Follow(pub Option<Entity>);

pub struct CameraControlPlugin;

impl Plugin for CameraControlPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Follow>()
            .add_systems(Update, (zoom, pan, pick_creature, follow_camera).chain());
    }
}

fn pick_creature(
    mouse: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera2d>>,
    creatures: Query<(Entity, &WorldPos), With<Creature>>,
    mut follow: ResMut<Follow>,
) {
    if !mouse.just_pressed(MouseButton::Middle) {
        return;
    }
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let (cam, cam_tf) = camera.into_inner();
    let Ok(point) = cam.viewport_to_world_2d(cam_tf, cursor) else {
        return;
    };

    follow.0 = creatures
        .iter()
        .map(|(e, p)| {
            let s = p.0 + Vec2::new(0.0, BODY_CENTER_Y);
            (e, s.distance(point))
        })
        .filter(|(_, d)| *d < PICK_RADIUS)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(e, _)| e);
}

fn follow_camera(
    mut follow: ResMut<Follow>,
    creatures: Query<&WorldPos, With<Creature>>,
    mut camera: Single<&mut Transform, With<Camera2d>>,
) {
    let Some(e) = follow.0 else { return };
    let Ok(pos) = creatures.get(e) else {
        follow.0 = None;
        return;
    };
    let s = pos.0 + Vec2::new(0.0, BODY_CENTER_Y);
    camera.translation.x = s.x;
    camera.translation.y = s.y;
}

fn zoom(
    scroll: Res<AccumulatedMouseScroll>,
    mut projection: Single<&mut Projection, With<Camera2d>>,
) {
    if scroll.delta.y == 0.0 {
        return;
    }

    // Mouse wheels report lines, trackpads report pixels.
    let k = match scroll.unit {
        MouseScrollUnit::Line => ZOOM_PER_LINE,
        MouseScrollUnit::Pixel => ZOOM_PER_PIXEL,
    };
    if let Projection::Orthographic(ortho) = &mut **projection {
        ortho.scale = (ortho.scale * (-k * scroll.delta.y).exp()).clamp(ZOOM_MIN, ZOOM_MAX);
    }
}

fn pan(
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    mut follow: ResMut<Follow>,
    mut camera: Single<&mut Transform, With<Camera2d>>,
) {
    let pan_factor = 1.0;
    if mouse.pressed(MouseButton::Right) {
        camera.translation.x -= motion.delta.x * pan_factor;
        camera.translation.y += motion.delta.y * pan_factor;
        follow.0 = None;
    }
    debug!("panfactpr={}, motion delta={}", pan_factor, motion.delta);
}
