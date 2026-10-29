use bevy::prelude::*;

use crate::GRID_SIZE;
use crate::creature::{CreatureSimulation, WorldPos};
use crate::ranged::UnitInterval;
use crate::sim::SimParams;
use crate::worldgen::WorldGrid;
use crate::{grid_to_world, world_to_grid};
use rand::RngExt;

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
        app.add_systems(
            FixedUpdate,
            (decay, decompose).chain().before(CreatureSimulation),
        );
    }
}

#[derive(Component)]
pub struct FoodDetails {
    /* FIXME: better name perhaps? The energy gained from consuming a
    particular food item depends on the amount of food (quality) and the
    digestive efficiency of that creature at consuming the food's kind */
    pub quality: f32,

    /* TODO: visualize freshness, lerp towards gray, ie.: -1 freshness (stale) is gray */
    pub freshness: UnitInterval,
    pub kind: FoodKind,
}

#[derive(Component)]
pub struct Food;

fn decay(
    time: Res<Time>,
    params: Res<SimParams>,
    world: Res<WorldGrid>,
    mut q: Query<(&mut FoodDetails, &WorldPos), With<Food>>,
) {
    for (mut details, pos) in &mut q {
        let cell = world_to_grid(pos.0);
        let biome_multiplier = world.biome_at(cell).map_or(1.0, |b| b.decay_multiplier());

        let mut f = details.freshness.into_inner() as f32;
        f -= params.food_decay_rate * time.delta_secs() * biome_multiplier * 0.2;
        details.freshness = UnitInterval::new_clamped(f.into());
    }
}

fn decompose(mut commands: Commands, q: Query<(Entity, &FoodDetails, &WorldPos), With<Food>>) {
    let mut rng = rand::rng();
    for (entity, details, pos) in &q {
        if details.freshness.into_inner() <= 0.0 {
            commands.entity(entity).despawn();
            if matches!(details.kind, FoodKind::Plant) {
                regrow_plants(&mut commands, pos.0, &mut rng);
            }
        }
    }
}

fn regrow_plants(commands: &mut Commands, pos: Vec2, rng: &mut impl rand::Rng) {
    // Preserve the existing odds: 60% one plant, 20% none, 20% two.
    let roll = rng.random::<f32>();
    let offspring = if roll < 0.6 {
        1
    } else if roll < 0.8 {
        0
    } else {
        2
    };
    let cell = world_to_grid(pos);
    for _ in 0..offspring {
        let offset = IVec2::new(rng.random_range(-3..=3), rng.random_range(-3..=3));
        let new_cell = (cell + offset).clamp(IVec2::ZERO, IVec2::splat(GRID_SIZE - 1));
        let quality = rng.random_range(0.5..1.0);
        let freshness = UnitInterval::try_new(rng.random_range(0.5..1.0)).unwrap();
        commands.spawn(bundle(
            grid_to_world(new_cell),
            FoodKind::Plant,
            quality,
            freshness,
        ));
    }
}

pub fn bundle(pos: Vec2, kind: FoodKind, quality: f32, freshness: UnitInterval) -> impl Bundle {
    /* TODO: food size proportional to quality */
    let sz = 8.0;

    let details = FoodDetails {
        kind,
        quality,
        freshness,
    };

    (
        Food,
        details,
        WorldPos(pos),
        Transform::from_translation(pos.extend(0.25)),
        Sprite::from_color(kind.color(), Vec2::new(sz, sz / 2.0)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{SeedableRng, rngs::StdRng};

    #[test]
    fn expired_plant_is_replaced_only_by_fresh_regrowth() {
        let mut app = App::new();
        app.add_systems(Update, decompose);
        let plant = app
            .world_mut()
            .spawn(bundle(
                grid_to_world(IVec2::ZERO),
                FoodKind::Plant,
                1.0,
                UnitInterval::try_new(0.0).unwrap(),
            ))
            .id();
        app.update();

        assert!(app.world().get_entity(plant).is_err());
        let mut query = app
            .world_mut()
            .query_filtered::<(&WorldPos, &FoodDetails), With<Food>>();
        assert!(query.iter(app.world()).count() <= 2);
        for (pos, details) in query.iter(app.world()) {
            assert!(matches!(details.kind, FoodKind::Plant));
            assert!(details.freshness.into_inner() > 0.0);
            assert_eq!(pos.0, grid_to_world(world_to_grid(pos.0)));
        }
    }

    #[test]
    fn decomposition_removes_carcasses_without_growing_plants_and_leaves_fresh_food_alone() {
        let mut app = App::new();
        app.add_systems(Update, decompose);
        let carcass = app
            .world_mut()
            .spawn(bundle(
                Vec2::ZERO,
                FoodKind::Carcass,
                1.0,
                UnitInterval::try_new(0.0).unwrap(),
            ))
            .id();
        let plant = app
            .world_mut()
            .spawn(bundle(
                Vec2::ZERO,
                FoodKind::Plant,
                1.0,
                UnitInterval::try_new(1.0).unwrap(),
            ))
            .id();
        app.update();

        assert!(app.world().get_entity(carcass).is_err());
        assert!(app.world().get_entity(plant).is_ok());
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<Food>>()
                .iter(app.world())
                .count(),
            1
        );
    }

    #[derive(Resource)]
    struct Regrowth {
        rng: StdRng,
        pos: Vec2,
    }

    fn seeded_regrowth(mut commands: Commands, mut regrowth: ResMut<Regrowth>) {
        let pos = regrowth.pos;
        regrow_plants(&mut commands, pos, &mut regrowth.rng);
    }

    #[test]
    fn regrowth_can_produce_zero_one_or_two_reachable_plants_inside_the_map() {
        for cell in [
            IVec2::ZERO,
            IVec2::new(0, GRID_SIZE - 1),
            IVec2::new(GRID_SIZE - 1, 0),
            IVec2::splat(GRID_SIZE - 1),
        ] {
            let mut app = App::new();
            app.insert_resource(Regrowth {
                rng: StdRng::seed_from_u64(42),
                pos: grid_to_world(cell),
            })
            .add_systems(Update, seeded_regrowth);
            let mut seen = [false; 3];
            let mut previous_count = 0;
            for _ in 0..64 {
                app.update();
                let mut query = app
                    .world_mut()
                    .query_filtered::<(&WorldPos, &FoodDetails), With<Food>>();
                let count = query.iter(app.world()).count();
                seen[count - previous_count] = true;
                previous_count = count;
                for (pos, details) in query.iter(app.world()) {
                    let new_cell = world_to_grid(pos.0);
                    assert!(new_cell.cmpge(IVec2::ZERO).all());
                    assert!(new_cell.cmplt(IVec2::splat(GRID_SIZE)).all());
                    assert!((new_cell - cell).abs().cmple(IVec2::splat(3)).all());
                    assert_eq!(pos.0, grid_to_world(new_cell));
                    assert!(matches!(details.kind, FoodKind::Plant));
                }
            }
            assert_eq!(seen, [true; 3]);
        }
    }
}
