use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;
use rand::{Rng, RngExt, seq::IteratorRandom};

use crate::GRID_SIZE;
use crate::TILE_SZ;
use crate::food;
use crate::food::{Food, FoodDetails, FoodKind};
use crate::grid_to_world;
use crate::sim::SimParams;

use crate::ranged::{SignedUnitInterval, UnitInterval};

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
    pub speed: f32,               /* tiles / second */
    pub size: f32,                /* fraction of TILE_SZ */
    pub diet: SignedUnitInterval, /* -1 is carnivorous, 1 is herbivorous */
    pub vision_radius: u32,       /* grids */
    pub attack: f32,
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
            speed: one(self.speed, f, 0.1, params.speed_max),
            size: one(self.size, f, 0.05, params.size_max),
            vision_radius: one(
                self.vision_radius as f32,
                f,
                4 as f32,
                params.vision_radius_max as f32,
            ) as u32,
            diet: SignedUnitInterval::try_new(
                one(self.diet.into_inner() as f32, f, -1.0, 1.0).into(),
            )
            .unwrap(),
            attack: one(self.attack, f, 0.1, params.attack_max),
        }
    }
}

#[derive(Component)]
pub struct Stats {
    pub energy: f32,
    pub reproduction_ticks_remaining: u32,
}

pub struct CreaturePlugin;

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct CreatureSimulation;

impl Plugin for CreaturePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SimParams>()
            .add_systems(Startup, spawn)
            .add_systems(Update, update)
            .add_systems(
                FixedUpdate,
                (
                    facing, start_step, advance, attack, eat, metabolize, reproduce, die,
                )
                    .chain()
                    .in_set(CreatureSimulation),
            );
    }
}

/// Lower is better. None = not worth considering.
fn hunting_cost(
    prey_size: f32,
    e_gain: f32,     // energy the corpse will give (your corpse formula)
    dist: f32,       // tiles
    attack: f32,     // attack gene: energy drained per second, before / size
    aggression: f32, // -diet, in [-1, 1]
    c_move: f32,     // energy per tile
    idle_burn: f32,  // burn rate while standing still
    p: &SimParams,
) -> Option<f64> {
    if aggression <= p.min_aggression || attack <= 0.0 || prey_size <= 0.0 {
        return None;
    }

    let fight_secs = p.max_energy * prey_size / attack;

    let chase = dist * c_move;
    let fight = fight_secs * idle_burn;

    // Don't hunt at a loss
    if chase + fight >= e_gain {
        return None;
    }

    Some(((chase + fight) / (e_gain * aggression)).into())
}

fn attack(
    time: Res<Time>,
    params: Res<SimParams>,
    mut q: Query<(Entity, &WorldPos, &Genes, &mut Stats), With<Creature>>,
) {
    let dt = time.delta_secs();
    let r2 = (params.attack_radius * TILE_SZ).powf(2.0);

    let snap: Vec<(Entity, Vec2, f32)> = q.iter().map(|(e, p, g, _)| (e, p.0, g.size)).collect();

    let mut damage: Vec<(Entity, f32)> = Vec::new();

    for (me, pos, genes, mut stats) in &mut q {
        if stats.energy <= 0.0 {
            continue;
        }
        let aggression = -genes.diet.into_inner();
        if aggression <= params.min_aggression.into() {
            continue;
        }

        let victim = snap
            .iter()
            .filter(|(e, ..)| *e != me)
            .map(|(e, p, size)| (*e, *size, (*p - pos.0).length_squared()))
            .filter(|(_, _, d2)| *d2 < r2)
            .min_by(|a, b| a.2.total_cmp(&b.2));

        let Some((victim, victim_size, _)) = victim else {
            continue;
        };

        damage.push((victim, genes.attack / victim_size * dt));
        stats.energy -= attack_burn(genes, &params) * dt;
    }

    for (victim, dmg) in damage {
        if let Ok((_, _, _, mut s)) = q.get_mut(victim) {
            s.energy -= dmg;
        }
    }
}

/// Energy per second spent while attacking. Grows with the attack gene.
fn attack_burn(genes: &Genes, p: &SimParams) -> f32 {
    let a = genes.attack / p.attack_max;
    p.base_cost * p.w_attack * a * a
}

fn facing(
    mut qcreature: Query<(Entity, &mut Facing, &GridPos, &WorldPos, &Genes), With<Creature>>,
    qprey: Query<(Entity, &WorldPos, &Genes), With<Creature>>,
    qfood: Query<(&WorldPos, &FoodDetails), With<Food>>,
    params: Res<SimParams>,
) {
    for (me, mut facing, gpos, pos, genes) in &mut qcreature {
        let r = genes.vision_radius as f32 * TILE_SZ;

        /* TODO: move cost determination to a new function
         * decoupled from path-finding */
        let food_cost = |food: (&WorldPos, &FoodDetails)| {
            let (food_pos, details) = food;
            let dist = (food_pos.0 - pos.0).length();
            let kind = match details.kind {
                FoodKind::Plant => 1.0,
                FoodKind::Carcass => -1.0,
            };

            let k: f64 = 1.0;
            let cost = (kind * k * genes.diet.into_inner()).exp();

            (*food_pos, dist, cost)
        };

        let c_move = move_cost_per_tile(genes, &params);
        let idle_burn = burn_rate(genes, &params, 0.0);
        let attack = genes.attack;
        let aggression = -genes.diet.into_inner();

        let hunt = |(prey_pos, prey_genes): (&WorldPos, &Genes)| -> Option<(WorldPos, f32, f64)> {
            let d = (prey_pos.0 - pos.0).abs();
            let dist = (d.x + d.y) / TILE_SZ;
            if dist > r {
                return None;
            }
            let cost = hunting_cost(
                prey_genes.size,
                corpse_energy(prey_genes),
                dist,
                attack,
                aggression as f32,
                c_move,
                idle_burn,
                &params,
            )?;
            Some((*prey_pos, dist, cost))
        };

        let food = qfood
            .iter()
            .map(food_cost)
            .filter(|(_, d, _)| *d <= r)
            .min_by(|a, b| a.2.total_cmp(&b.2));

        let prey = qprey
            .iter()
            .filter(|(e, ..)| *e != me)
            .map(|(_, pos, genes)| (pos, genes))
            .filter_map(hunt)
            .min_by(|a, b| a.2.total_cmp(&b.2));

        let target = match (food, prey) {
            (Some(f), Some(h)) if h.2 < f.2 => Some(h.0),
            (Some(f), _) => Some(f.0),
            (None, h) => h.map(|h| h.0),
        };

        let Some(target_pos) = target else {
            let mut rng = rand::rng();
            facing.0 = pick_direction(&gpos, &mut rng);
            continue;
        };

        let direction = target_pos.0 - pos.0;
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

fn burn_rate(genes: &Genes, p: &SimParams, moving: f32) -> f32 {
    let s = genes.size / p.size_max;
    let v = genes.speed / p.speed_max;
    let r = genes.vision_radius as f32 / p.vision_radius_max as f32;

    p.base_cost * (p.w_size * s.powi(3) + p.w_move * moving * s * v * v + p.w_vision * r * r)
}

fn move_cost_per_tile(genes: &Genes, p: &SimParams) -> f32 {
    burn_rate(genes, p, 1.0) / genes.speed.max(f32::EPSILON)
}

fn metabolize(
    time: Res<Time>,
    params: Res<SimParams>,
    mut q: Query<(&Genes, &mut Stats, &WorldPos, &GridPos), With<Creature>>,
) {
    for (genes, mut stats, pos, gpos) in &mut q {
        let moving = pos.0.distance(grid_to_world(gpos.0)) > 1e-4;

        let moving = if moving { 1.0 } else { 0.0 };

        let cost = burn_rate(genes, &params, moving);
        stats.energy -= time.delta_secs() * cost;
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
                stats.energy +=
                    food_details.quality * food_details.freshness.into_inner() as f32 * 2.0;
                commands.entity(entity).despawn();
            }
        }
    }
}

fn corpse_energy(genes: &Genes) -> f32 {
    genes.size * 10.0
}

fn die(mut commands: Commands, q: Query<(Entity, &Genes, &WorldPos, &Stats), With<Creature>>) {
    for (entity, _, pos, Stats { energy, .. }) in &q {
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
    mut q: Query<(&Genes, &GridPos, &mut Stats), With<Creature>>,
    params: Res<SimParams>,
) {
    let mut rng = rand::rng();
    for (genes, gpos, mut stats) in &mut q {
        if stats.energy <= 0.0 {
            continue;
        }

        stats.reproduction_ticks_remaining = stats.reproduction_ticks_remaining.saturating_sub(1);
        if stats.reproduction_ticks_remaining > 0 {
            continue;
        }

        stats.energy -= params.reproduction_cost;
        if stats.energy <= 0.0 {
            continue;
        }

        stats.reproduction_ticks_remaining = rng.random_range(params.reproduction_tick.clone());
        let genes_ = genes.mutate(&mut rng, &params);
        let gpos_ = GridPos(gpos.0 + pick_direction(&gpos, &mut rng).unwrap_or(IVec2::ZERO));
        commands.spawn(spawn_creature(gpos_, genes_, &params, &mut rng));
    }
}

fn spawn_creature<R: Rng>(
    gpos: GridPos,
    genes: Genes,
    params: &SimParams,
    rng: &mut R,
) -> impl Bundle + use<R> {
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
        Stats {
            energy: 100.0,
            reproduction_ticks_remaining: rng.random_range(params.reproduction_tick.clone()),
        },
        Transform::from_translation(pos.extend(0.0)),
        Visibility::default(),
        children![(sprite, Transform::from_xyz(0.0, 0.0, 0.5),)],
    )
}

fn spawn(mut commands: Commands, params: Res<SimParams>) {
    /* TODO: procedural color
     * TODO: default position */

    let gpos = GridPos(IVec2::new(0, 0));
    let genes = Genes {
        speed: 4.0,
        size: 0.5,
        vision_radius: 10,
        attack: 10.0,
        diet: SignedUnitInterval::try_new(0.0).unwrap(),
    };

    commands.spawn(spawn_creature(gpos, genes, &params, &mut rand::rng()));
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
            attack: 10.0,
            diet: SignedUnitInterval::try_new(0.0).unwrap(),
        }
    }

    fn test_creature(gpos: GridPos) -> impl Bundle {
        spawn_creature(gpos, genes(), &SimParams::default(), &mut rand::rng())
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
        let creature = app.world_mut().spawn(test_creature(GridPos(cell))).id();
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
        let newborn = app.world_mut().spawn(test_creature(GridPos(cell))).id();
        app.world_mut().get_mut::<Facing>(newborn).unwrap().0 = Some(IVec2::X);

        for corner in [
            IVec2::ZERO,
            IVec2::new(0, GRID_SIZE - 1),
            IVec2::new(GRID_SIZE - 1, 0),
            IVec2::splat(GRID_SIZE - 1),
        ] {
            for dir in [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y] {
                let creature = app.world_mut().spawn(test_creature(GridPos(corner))).id();
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
                .add_systems(Update, reproduce);
            let parent = app.world_mut().spawn(test_creature(GridPos(cell))).id();
            app.world_mut()
                .get_mut::<Stats>(parent)
                .unwrap()
                .reproduction_ticks_remaining = 1;
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
    fn spawn_samples_each_countdown_from_the_configured_range() {
        use rand::SeedableRng;

        let params = SimParams {
            reproduction_tick: 2..=4,
            ..default()
        };
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let mut world = World::new();
        let mut countdowns = Vec::new();
        for _ in 0..32 {
            let creature = world.spawn(spawn_creature(
                GridPos(IVec2::ZERO),
                genes(),
                &params,
                &mut rng,
            ));
            countdowns.push(
                creature
                    .get::<Stats>()
                    .unwrap()
                    .reproduction_ticks_remaining,
            );
        }
        assert!(
            countdowns
                .iter()
                .all(|ticks| params.reproduction_tick.contains(ticks))
        );
        assert!(countdowns.contains(&2));
        assert!(countdowns.contains(&4));
    }

    #[test]
    fn creatures_count_down_independently_and_reroll_after_reproducing() {
        let mut app = App::new();
        app.insert_resource(SimParams {
            reproduction_tick: 7..=7,
            reproduction_cost: 25.0,
            ..default()
        })
        .add_systems(Update, reproduce);
        let parent = app
            .world_mut()
            .spawn(test_creature(GridPos(IVec2::ZERO)))
            .id();
        let other = app
            .world_mut()
            .spawn(test_creature(GridPos(IVec2::ONE)))
            .id();
        app.world_mut()
            .get_mut::<Stats>(parent)
            .unwrap()
            .reproduction_ticks_remaining = 2;
        app.world_mut()
            .get_mut::<Stats>(other)
            .unwrap()
            .reproduction_ticks_remaining = 4;

        app.update();
        assert_eq!(
            app.world()
                .get::<Stats>(parent)
                .unwrap()
                .reproduction_ticks_remaining,
            1
        );
        assert_eq!(
            app.world()
                .get::<Stats>(other)
                .unwrap()
                .reproduction_ticks_remaining,
            3
        );
        assert_eq!(app.world().get::<Stats>(parent).unwrap().energy, 100.0);
        assert_eq!(
            app.world_mut()
                .query::<&Creature>()
                .iter(app.world())
                .count(),
            2
        );

        app.update();
        let stats = app.world().get::<Stats>(parent).unwrap();
        assert_eq!(stats.reproduction_ticks_remaining, 7);
        assert_eq!(stats.energy, 75.0);
        let stats = app.world().get::<Stats>(other).unwrap();
        assert_eq!(stats.reproduction_ticks_remaining, 2);
        assert_eq!(stats.energy, 100.0);
        let mut query = app
            .world_mut()
            .query_filtered::<(Entity, &Stats), With<Creature>>();
        assert_eq!(query.iter(app.world()).count(), 3);
        let (_, newborn) = query
            .iter(app.world())
            .find(|(entity, _)| *entity != parent && *entity != other)
            .unwrap();
        assert_eq!(newborn.reproduction_ticks_remaining, 7);
        assert_eq!(newborn.energy, 100.0);

        app.update();
        assert_eq!(
            app.world()
                .get::<Stats>(parent)
                .unwrap()
                .reproduction_ticks_remaining,
            6
        );
    }

    #[test]
    fn reproduction_requires_strictly_positive_energy_after_paying_the_cost() {
        for energy in [0.0, 49.0, 50.0, 51.0] {
            let mut app = App::new();
            app.init_resource::<SimParams>()
                .add_systems(Update, (reproduce, die).chain());
            let parent = app
                .world_mut()
                .spawn(test_creature(GridPos(IVec2::ZERO)))
                .id();
            {
                let mut stats = app.world_mut().get_mut::<Stats>(parent).unwrap();
                stats.energy = energy;
                stats.reproduction_ticks_remaining = 1;
            }

            app.update();
            let creatures = app
                .world_mut()
                .query::<&Creature>()
                .iter(app.world())
                .count();
            let carcasses = app.world_mut().query::<&Food>().iter(app.world()).count();
            if energy > 50.0 {
                assert_eq!(creatures, 2);
                assert_eq!(carcasses, 0);
                let stats = app.world().get::<Stats>(parent).unwrap();
                assert_eq!(stats.energy, 1.0);
                assert!((800..=1200).contains(&stats.reproduction_ticks_remaining));
            } else {
                assert_eq!(creatures, 0);
                assert_eq!(carcasses, 1);
                assert!(app.world().get::<Creature>(parent).is_none());
            }
        }
    }
}
