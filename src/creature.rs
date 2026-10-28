use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;
use rand::{Rng, RngExt, seq::IteratorRandom};

use crate::GRID_SIZE;
use crate::TILE_SZ;
use crate::food;
use crate::food::{Food, FoodKind, FoodDetails};
use crate::grid_to_world;
use crate::sim::SimParams;

use crate::ranged::{NoNaNF32, SignedUnitInterval, UnitInterval};

#[derive(Component)]
pub struct Creature;

#[derive(Component, Clone, Copy)]
pub struct GridPos(pub IVec2);

#[derive(Component, Clone, Copy)]
pub struct WorldPos(pub Vec2);

#[derive(Component, Default)]
pub struct Facing(pub Option<IVec2>);

#[derive(Component, Clone, Copy)]
pub struct Genes {
    pub speed: f32,               /* tiles / dt */
    pub size: f32,                /* fraction of TILE_SZ */
    pub diet: SignedUnitInterval, /* -1 is carnivorous, 1 is herbivorous */
    pub vision_radius: u32,       /* grids */
}

impl Genes {
    fn mutate(&self, rng: &mut impl Rng, params: &Res<SimParams>) -> Self {
        let p = params.gene_pick_probability.into_inner() as f32;
        let f = params.gene_mutation_factor.into_inner() as f32;
        let mut one = |gene: f32, factor: f32, min: f32, max: f32| -> f32 {
            if rng.random_bool(p.into()) {
                let range = (1.0 - factor)..(1.0 + factor);
                gene * rng.random_range(range).clamp(min, max)
            } else {
                gene
            }
        };

        Self {
            /* TODO: move min / max fine-tuning to sim::SimParams */
            speed: one(self.speed, f, 0.1, 10.0),
            size: one(self.size, f, 0.05, 0.95),
            vision_radius: one(self.vision_radius as f32, f, 4 as f32, GRID_SIZE as f32) as u32,
            diet: SignedUnitInterval::try_new(
                one(self.diet.into_inner() as f32, f, -1.0, 1.0).into(),
            )
            .unwrap(),
        }
    }
}

#[derive(Component)]
pub struct Stats {
    pub energy: f32,
}

impl Default for Stats {
    fn default() -> Self {
        Self { energy: 100.0 }
    }
}

pub struct CreaturePlugin;

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct CreatureSimulation;

#[derive(Resource, Default)]
pub struct Counter(u32);

impl Plugin for CreaturePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SimParams>()
            .init_resource::<Counter>()
            .add_systems(Startup, spawn)
            .add_systems(Update, update)
            .add_systems(
                FixedUpdate,
                (facing, start_step, advance, eat, metabolize, die, reproduce)
                    .chain()
                    .in_set(CreatureSimulation),
            );
    }
}

fn facing(
    mut qcreature: Query<(&mut Facing, &GridPos, &WorldPos, &Genes), With<Creature>>,
    qfood: Query<&WorldPos, With<Food>>,
) {
    for (mut facing, gpos, pos, genes) in &mut qcreature {
        let r = genes.vision_radius as f32 * TILE_SZ;

        let mut foods = qfood
            .iter()
            .map(|x| (x, (x.0 - pos.0).length()))
            .filter(|(_, d)| *d <= r)
            .collect::<Vec<_>>();

        foods.sort_by_key(|(_, d)| NoNaNF32::try_new(*d).unwrap());

        /* TODO: pick a random direction */
        let Some(nearest) = foods.first() else {
            let mut rng = rand::rng();
            facing.0 = pick_direction(&gpos, &mut rng);
            continue;
        };

        let direction = nearest.0.0 - pos.0;
        if direction == Vec2::ZERO {
            facing.0 = None;
            continue;
        }
        let direction = if direction.x.abs() > direction.y.abs() {
            IVec2::new(direction.x.signum() as i32, 0)
        } else {
            IVec2::new(0, direction.y.signum() as i32)
        };

        facing.0 = Some(direction);
    }
}

fn advance(time: Res<Time>, mut q: Query<(&mut WorldPos, &GridPos, &Genes), With<Creature>>) {
    for (mut pos, grid, gene) in &mut q {
        let to = grid_to_world(grid.0) - pos.0;
        let dist = to.length();
        let step = gene.speed * TILE_SZ * time.delta_secs();

        pos.0 = if dist <= step {
            grid_to_world(grid.0)
        } else {
            pos.0 + to / dist * step
        };
    }
}

fn start_step(mut q: Query<(&mut GridPos, &WorldPos, &Facing), With<Creature>>) {
    for (mut gpos, pos, intent) in &mut q {
        if pos.0.distance(grid_to_world(gpos.0)) > 1e-4 {
            continue;
        }

        let Some(dir) = intent.0 else { continue };
        let next = gpos.0 + dir;
        if next.cmpge(IVec2::ZERO).all() && next.cmplt(IVec2::splat(GRID_SIZE)).all() {
            gpos.0 = next;
        }
    }
}

/* TODO: update is just a general drawing function at this point,
 *       maybe abstract this to it's own render module? */
fn update(mut q: Query<(&WorldPos, &mut Transform), Or<(With<Creature>, With<Food>)>>) {
    for (p, mut t) in &mut q {
        t.translation.x = p.0.x;
        t.translation.y = p.0.y;
    }
}

fn metabolize(
    time: Res<Time>,
    params: Res<SimParams>,
    mut q: Query<(&Genes, &mut Stats, &WorldPos, &GridPos), With<Creature>>,
) {
    for (genes, mut stats, pos, gpos) in &mut q {
        let moving = pos.0.distance(grid_to_world(gpos.0)) > 1e-4;

        let moving = if moving { 1.0 } else { 0.0 };

        /* TODO: possibly a better abstraction for calculating cost of particular
         * characterestics / stats? */

        let size_cost = genes.size.powf(3.0) * params.base_cost;
        let speed_cost = genes.speed.powf(2.0) * moving;
        let vision_cost = genes.vision_radius as f32 * 0.05;

        let cost = size_cost + speed_cost + vision_cost;

        stats.energy -= (time.delta_secs() * cost).max(0.0);
    }
}

fn eat(
    mut commands: Commands,
    mut qcreature: Query<(&WorldPos, &mut Stats), With<Creature>>,
    qfood: Query<(Entity, &WorldPos, &FoodDetails), With<Food>>,
) {
    // Half a tile's diagonal reaches every point in a tile from its center.
    let eat_radius_squared = TILE_SZ * TILE_SZ / 2.0;
    let mut eaten = EntityHashSet::default();

    for (pos, mut stats) in &mut qcreature {
        for (entity, food_pos, food_details) in &qfood {
            if pos.0.distance_squared(food_pos.0) <= eat_radius_squared && eaten.insert(entity) {
                /* TODO: diet */
                /* TODO: how do creatures know food isn't fresh? */
                stats.energy += food_details.quality * food_details.freshness.into_inner() as f32 * 2.0;
                commands.entity(entity).despawn();
            }
        }
    }
}

fn die(mut commands: Commands, q: Query<(Entity, &Genes, &WorldPos, &Stats), With<Creature>>) {
    for (entity, _, pos, Stats { energy }) in &q {
        if *energy <= 0.0 {
            commands.spawn(food::bundle(
                pos.0,
                FoodKind::Carcass,
                20.0,
                UnitInterval::try_new(1.0).unwrap(),
            ));

            commands.entity(entity).despawn();
        }
    }
}

fn in_bounds(p: IVec2) -> bool {
    p.cmpge(IVec2::ZERO).all() && p.cmplt(IVec2::splat(GRID_SIZE)).all()
}

fn pick_direction(gpos: &GridPos, rng: &mut impl rand::Rng) -> Option<IVec2> {
    const DIRS: [IVec2; 4] = [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y];
    DIRS.iter()
        .filter(|&d| in_bounds(gpos.0 + d))
        .choose(rng)
        .copied()
}

fn reproduce(
    mut commands: Commands,
    q: Query<(&Genes, &GridPos), With<Creature>>,
    mut counter: ResMut<Counter>,
    params: Res<SimParams>,
) {
    if counter.0 < params.reproduction_tick.get() {
        counter.0 += 1;
        return;
    }

    counter.0 = 0;

    for (genes, gpos) in &q {
        let mut rng = rand::rng();
        let genes_ = genes.mutate(&mut rng, &params);
        let gpos_ = GridPos(gpos.0 + pick_direction(&gpos, &mut rng).unwrap_or(IVec2::ZERO));
        commands.spawn(spawn_creature(gpos_, genes_));
    }
}

fn spawn_creature(gpos: GridPos, genes: Genes) -> impl Bundle {
    let pos = grid_to_world(gpos.0);
    let rect = Vec2::splat(TILE_SZ * genes.size);

    let red = Color::srgb(0.8588, 0.0274, 0.0274);
    let green = Color::srgb(0.4784, 1.0, 0.3490);
    let t = (genes.diet.into_inner() + 1.0) / 2.0;

    let color = red.mix(&green, t as f32);

    let sprite = Sprite::from_color(color, rect);

    (
        Creature,
        WorldPos(pos),
        gpos,
        genes,
        Facing::default(),
        Stats::default(),
        Transform::from_translation(pos.extend(0.0)),
        Visibility::default(),
        children![(sprite, Transform::from_xyz(0.0, 0.0, 0.5),)],
    )
}

fn spawn(mut commands: Commands) {
    /* TODO: procedural color
     * TODO: default position */

    let gpos = GridPos(IVec2::new(0, 0));
    let genes = Genes {
        speed: 4.0,
        size: 0.5,
        vision_radius: 10,
        diet: SignedUnitInterval::try_new(0.0).unwrap(),
    };

    commands.spawn(spawn_creature(gpos, genes));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn genes() -> Genes {
        Genes {
            speed: 1.0,
            size: 0.5,
            vision_radius: 10,
            diet: SignedUnitInterval::try_new(0.0).unwrap(),
        }
    }

    fn app_with_time(seconds: f32) -> App {
        let mut app = App::new();
        let mut time = Time::<()>::default();
        time.advance_by(Duration::from_secs_f32(seconds));
        app.insert_resource(time);
        app
    }

    #[test]
    fn creature_sprite_tracks_its_world_position_without_a_spawn_offset() {
        let mut app = App::new();
        app.add_plugins(TransformPlugin).add_systems(Update, update);
        let cell = IVec2::splat(GRID_SIZE - 1);
        let creature = app
            .world_mut()
            .spawn(spawn_creature(GridPos(cell), genes()))
            .id();
        assert_eq!(
            app.world().get::<WorldPos>(creature).unwrap().0,
            grid_to_world(cell)
        );

        for position in [
            grid_to_world(cell),
            grid_to_world(cell) - Vec2::new(8.0, 0.0),
        ] {
            app.world_mut().get_mut::<WorldPos>(creature).unwrap().0 = position;
            app.update();
            let sprite = app.world().get::<Children>(creature).unwrap()[0];
            let visible = app
                .world()
                .get::<GlobalTransform>(sprite)
                .unwrap()
                .translation();
            assert_eq!(visible.truncate(), position);
            assert_eq!(visible.z, 0.5);
        }
    }

    #[test]
    fn newborn_can_start_moving_and_cannot_step_outside_the_map() {
        let mut app = app_with_time(1.0);
        app.add_systems(Update, (start_step, advance).chain());
        let cell = IVec2::new(12, 34);
        let newborn = app
            .world_mut()
            .spawn(spawn_creature(GridPos(cell), genes()))
            .id();
        app.world_mut().get_mut::<Facing>(newborn).unwrap().0 = Some(IVec2::X);

        for corner in [
            IVec2::ZERO,
            IVec2::new(0, GRID_SIZE - 1),
            IVec2::new(GRID_SIZE - 1, 0),
            IVec2::splat(GRID_SIZE - 1),
        ] {
            for dir in [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y] {
                let creature = app
                    .world_mut()
                    .spawn(spawn_creature(GridPos(corner), genes()))
                    .id();
                app.world_mut().get_mut::<Facing>(creature).unwrap().0 = Some(dir);
            }
        }

        app.update();
        assert_eq!(
            app.world().get::<GridPos>(newborn).unwrap().0,
            cell + IVec2::X
        );
        assert_eq!(
            app.world().get::<WorldPos>(newborn).unwrap().0,
            grid_to_world(cell + IVec2::X)
        );
        for _ in 0..3 {
            app.update();
            for (grid, pos) in app
                .world_mut()
                .query::<(&GridPos, &WorldPos)>()
                .iter(app.world())
            {
                assert!(in_bounds(grid.0));
                assert_eq!(pos.0, grid_to_world(grid.0));
            }
        }
    }

    #[test]
    fn offspring_spawn_on_a_neighboring_tile_near_the_parent() {
        for cell in [IVec2::ZERO, IVec2::new(42, 60), IVec2::splat(GRID_SIZE - 1)] {
            let mut app = App::new();
            app.init_resource::<SimParams>()
                .insert_resource(Counter(SimParams::default().reproduction_tick.get()))
                .add_systems(Update, reproduce);
            let parent = app
                .world_mut()
                .spawn(spawn_creature(GridPos(cell), genes()))
                .id();
            app.update();

            let mut query = app
                .world_mut()
                .query_filtered::<(Entity, &GridPos, &WorldPos), With<Creature>>();
            assert_eq!(query.iter(app.world()).count(), 2);
            for (entity, grid, pos) in query.iter(app.world()) {
                if entity != parent {
                    assert!(in_bounds(grid.0));
                    assert_eq!((grid.0 - cell).abs().element_sum(), 1);
                    assert_eq!(pos.0, grid_to_world(grid.0));
                    assert_eq!(pos.0.distance(grid_to_world(cell)), TILE_SZ);
                }
            }
        }
    }

    #[test]
    fn creature_reaches_and_eats_food_between_tile_centers() {
        let mut app = app_with_time(0.125);
        app.add_systems(Update, (facing, start_step, advance, eat).chain());
        let cell = IVec2::new(30, 40);
        let creature = app
            .world_mut()
            .spawn(spawn_creature(GridPos(cell), genes()))
            .id();
        let food_pos = (cell + IVec2::new(3, 2)).as_vec2() * TILE_SZ + Vec2::splat(0.1);
        let food = app
            .world_mut()
            .spawn(food::bundle(food_pos, FoodKind::Plant, 1.0, 1.0))
            .id();

        for _ in 0..80 {
            app.update();
            if app.world().get_entity(food).is_err() {
                break;
            }
        }
        assert!(
            app.world().get_entity(food).is_err(),
            "creature never reached the food"
        );
        assert_eq!(app.world().get::<Stats>(creature).unwrap().energy, 110.0);
    }

    #[test]
    fn food_is_consumed_only_once_when_creatures_share_a_tile() {
        let mut app = App::new();
        app.add_systems(Update, eat);
        let cell = IVec2::new(10, 20);
        for _ in 0..2 {
            app.world_mut()
                .spawn(spawn_creature(GridPos(cell), genes()));
        }
        let food = app
            .world_mut()
            .spawn(food::bundle(grid_to_world(cell), FoodKind::Plant, 1.0, 1.0))
            .id();
        app.update();

        let energy: f32 = app
            .world_mut()
            .query::<&Stats>()
            .iter(app.world())
            .map(|s| s.energy)
            .sum();
        assert_eq!(energy, 210.0);
        assert!(app.world().get_entity(food).is_err());
    }
}
