use bevy::input::mouse::{AccumulatedMouseMotion, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::EguiContexts;

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
    if !window.focused || !mouse.just_pressed(MouseButton::Middle) {
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
    mut scroll: MessageReader<MouseWheel>,
    window: Single<Entity, With<PrimaryWindow>>,
    mut projection: Single<&mut Projection, With<Camera2d>>,
    mut contexts: EguiContexts,
) {
    // Wheel messages carry their window; the accumulated resource mixes all windows.
    let delta: f32 = scroll
        .read()
        .filter(|event| event.window == *window)
        .map(|event| {
            let k = match event.unit {
                MouseScrollUnit::Line => ZOOM_PER_LINE,
                MouseScrollUnit::Pixel => ZOOM_PER_PIXEL,
            };
            k * event.y
        })
        .sum();
    if delta == 0.0 {
        return;
    }
    // UI scrolling must not also zoom the simulation underneath it.
    if contexts
        .ctx_mut()
        .is_ok_and(|ctx| ctx.is_pointer_over_egui() || ctx.egui_wants_pointer_input())
    {
        return;
    }

    if let Projection::Orthographic(ortho) = &mut **projection {
        ortho.scale = (ortho.scale * (-delta).exp()).clamp(ZOOM_MIN, ZOOM_MAX);
    }
}

fn pan(
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    mut follow: ResMut<Follow>,
    mut camera: Single<&mut Transform, With<Camera2d>>,
    window: Single<&Window, With<PrimaryWindow>>,
) {
    if !window.focused || window.cursor_position().is_none() {
        return;
    }
    let pan_factor = 1.0;
    if mouse.pressed(MouseButton::Right) {
        camera.translation.x -= motion.delta.x * pan_factor;
        camera.translation.y += motion.delta.y * pan_factor;
        follow.0 = None;
        
    }
    debug!("panfactpr={}, motion delta={}", pan_factor, motion.delta);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::touch::TouchPhase;

    #[test]
    fn wheel_in_the_evolution_window_does_not_zoom_the_simulation() {
        let mut app = App::new();
        app.add_message::<MouseWheel>()
            .init_resource::<bevy_egui::EguiUserTextures>()
            .add_systems(Update, zoom);
        let primary = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        let graph = app.world_mut().spawn(Window::default()).id();
        let camera = app.world_mut().spawn(Camera2d).id();
        for (window, expected) in [(graph, 1.0), (primary, (-ZOOM_PER_LINE).exp())] {
            app.world_mut().write_message(MouseWheel {
                unit: MouseScrollUnit::Line,
                x: 0.0,
                y: 1.0,
                window,
                phase: TouchPhase::Moved,
            });
            app.update();
            let Projection::Orthographic(projection) =
                app.world().get::<Projection>(camera).unwrap()
            else {
                panic!("expected orthographic camera");
            };
            assert!((projection.scale - expected).abs() < 1e-6);
        }
    }

    #[test]
    fn dragging_in_another_window_preserves_the_simulation_camera_and_follow_target() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<MouseButton>>()
            .insert_resource(AccumulatedMouseMotion {
                delta: Vec2::new(20.0, 10.0),
            })
            .insert_resource(Follow(Some(Entity::PLACEHOLDER)))
            .add_systems(Update, pan);
        let primary = app
            .world_mut()
            .spawn((
                Window {
                    focused: false,
                    ..default()
                },
                PrimaryWindow,
            ))
            .id();
        app.world_mut().spawn(Window::default());
        let camera = app.world_mut().spawn(Camera2d).id();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Right);
        app.update();
        assert_eq!(
            app.world().get::<Transform>(camera).unwrap().translation,
            Vec3::ZERO
        );
        assert!(app.world().resource::<Follow>().0.is_some());
        {
            let mut window = app.world_mut().get_mut::<Window>(primary).unwrap();
            window.focused = true;
            window.set_cursor_position(Some(Vec2::new(100.0, 100.0)));
        }
        app.update();
        assert_eq!(
            app.world().get::<Transform>(camera).unwrap().translation,
            Vec3::new(-20.0, 10.0, 0.0)
        );
        assert!(app.world().resource::<Follow>().0.is_none());
    }
}
