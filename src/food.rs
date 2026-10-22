use bevy::prelude::*;

use crate::creature::WorldPos;
use crate::sim::SimParams;

#[derive(Clone, Copy)]
pub enum FoodKind {
    Plant,
    Carcass,
}

impl FoodKind {
    pub fn color(&self) -> Color {
        match self {
            Self::Plant => Color::srgb(0.1, 0.5, 0.1),
            Self::Carcass => Color::srgb(0.8, 0.1, 0.1),
        }
    }
}

pub struct FoodPlugin;

impl Plugin for FoodPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, (decay, decompose).chain());
    }
}

#[derive(Component)]
pub struct FoodDetails {
    /* FIXME: better name perhaps? The energy gained from consuming a
    particular food item depends on the amount of food (quality) and the
    digestive efficiency of that creature at consuming the food's kind */
    pub quality: f32,

    /* TODO: an explicit range from [-1, 1] for freshness as a newtype,
     * the nutype crate seems interesting for this */
    /* TODO: visualize freshness, lerp towards gray, ie.: -1 freshness (stale) is gray */
    pub freshness: f32,
    pub kind: FoodKind,
}

#[derive(Component)]
pub struct Food;

fn decay(time: Res<Time>, params: Res<SimParams>, mut q: Query<&mut FoodDetails, With<Food>>) {
    for mut details in &mut q {
        details.freshness -= params.food_decay_rate * time.delta_secs();
    }
}

fn decompose(mut commands: Commands, q: Query<(Entity, &FoodDetails), With<Food>>) {
    for (entity, details) in &q {
        if details.freshness <= -1.0 {
            commands.entity(entity).despawn();
        }
    }
}

pub fn bundle(pos: Vec2, kind: FoodKind, quality: f32) -> impl Bundle {
    /* TODO: food size proportional to quality */
    let sz = 8.0;

    let details = FoodDetails {
        kind,
        quality,
        freshness: 1.0,
    };

    (
        Food,
        details,
        WorldPos(pos),
        Transform::default(),
        Sprite::from_color(kind.color(), Vec2::new(sz, sz / 2.0)),
    )
}
