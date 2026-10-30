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

// Five genes with twelve ordered thresholds each fit in a u64. Ten differing
// thresholds tolerate ordinary offspring mutations without protecting all prey.
const GENE_HASH_BITS: u32 = 12;
const KIN_HASH_DISTANCE: u32 = 10;

#[derive(Clone, Copy)]
struct GeneHash(u64);

impl GeneHash {
    fn distance(self, other: Self) -> u32 {
        (self.0 ^ other.0).count_ones()
    }

    fn is_kin(self, other: Self) -> bool {
        self.distance(other) <= KIN_HASH_DISTANCE
    }
}

impl Genes {
    /// A locality-preserving hash: each normalized gene sets a prefix of bits.
    /// Unlike hashing float bytes, small numeric mutations change few bits;
    /// unlike comparing genes to their mean, absolute trait values are retained.
    /// PS: thanks claude :>
    fn perceptual_hash(&self, params: &SimParams) -> GeneHash {
        let traits = [
            (self.speed, params.speed_min, params.speed_max),
            (self.size, params.size_min, params.size_max),
            (
                self.diet.into_inner() as f32,
                params.diet_min,
                params.diet_max,
            ),
            (
                self.vision_radius as f32,
                params.vision_radius_min as f32,
                params.vision_radius_max as f32,
            ),
            (self.attack, params.attack_min, params.attack_max),
        ];
        let mut hash = 0;
        for (value, min, max) in traits {
            let normalized = if max > min {
                ((value - min) / (max - min)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let bits = (normalized * GENE_HASH_BITS as f32).round() as u32;
            hash = (hash << GENE_HASH_BITS) | ((1_u64 << bits) - 1);
        }
        GeneHash(hash)
    }

    fn random(rng: &mut impl Rng, params: &SimParams) -> Self {
        Self {
            speed: rng.random_range(params.speed_min..=params.speed_max),
            size: rng.random_range(params.size_min..=params.size_max),
            vision_radius: rng.random_range(params.vision_radius_min..=params.vision_radius_max),
            attack: rng.random_range(params.attack_min..=params.attack_max),
            diet: SignedUnitInterval::try_new(
                rng.random_range(params.diet_min..=params.diet_max).into(),
            )
            .unwrap(),
        }
    }

    fn mutate(&self, rng: &mut impl Rng, params: &SimParams) -> Self {
        let p = params.gene_pick_probability.into_inner() as f32;
        let f = params.gene_mutation_factor.into_inner() as f32;
        let mut one = |gene: f32, factor: f32, min: f32, max: f32| -> f32 {
            if rng.random_bool(p.into()) {
                let range = (1.0 - factor)..=(1.0 + factor);
                (gene * rng.random_range(range)).clamp(min, max)
            } else {
                gene
            }
        };

        Self {
            speed: one(self.speed, f, params.speed_min, params.speed_max),
            size: one(self.size, f, params.size_min, params.size_max),
            vision_radius: one(
                self.vision_radius as f32,
                f,
                params.vision_radius_min as f32,
                params.vision_radius_max as f32,
            ) as u32,
            diet: SignedUnitInterval::try_new(
                one(
                    self.diet.into_inner() as f32,
                    f,
                    params.diet_min,
                    params.diet_max,
                )
                .into(),
            )
            .unwrap(),
            attack: one(self.attack, f, params.attack_min, params.attack_max),
        }
    }
}

#[derive(Component)]
pub struct Stats {
    pub energy: f32,
    pub reproduction_ticks_remaining: u32,
}

/// A successful reproduction; founders are not counted as births.
#[derive(Event)]
pub struct CreatureBorn {
    pub parent_diet: f32,
    pub offspring_diet: f32,
}

#[derive(Event)]
pub struct CreatureDied {
    pub diet: f32,
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
    dist: f32, // tiles
    hunter: &Genes,
    p: &SimParams,
) -> Option<f64> {
    if !can_hunt(hunter, p) || hunter.attack <= 0.0 || prey_size <= 0.0 {
        return None;
    }

    let e_gain = food_energy(hunter, &corpse_food(&p, prey_size));
    let fight_secs = p.max_energy * prey_size / hunter.attack;

    let chase = dist * move_cost_per_tile(hunter, p);
    let fight = fight_secs * (burn_rate(hunter, p, 0.0) + attack_burn(hunter, p));

    // Don't hunt at a loss
    if chase + fight >= e_gain {
        return None;
    }

    Some(((chase + fight) / e_gain).into())
}

fn can_hunt(genes: &Genes, params: &SimParams) -> bool {
    let aggression = -genes.diet.into_inner();
    aggression > 0.0 && aggression > params.min_aggression.into()
}

fn attack(
    time: Res<Time>,
    params: Res<SimParams>,
    mut q: Query<(Entity, &WorldPos, &Genes, &mut Stats), With<Creature>>,
) {
    let dt = time.delta_secs();
    let r2 = (params.attack_radius * TILE_SZ).powf(2.0);

    let snap: Vec<_> = q
        .iter()
        .map(|(e, p, g, _)| (e, p.0, g.size, g.perceptual_hash(&params)))
        .collect();

    let mut damage: Vec<(Entity, f32)> = Vec::new();

    for (me, pos, genes, mut stats) in &mut q {
        if stats.energy <= 0.0 {
            continue;
        }
        if !can_hunt(genes, &params) {
            continue;
        }

        let kin_hash = genes.perceptual_hash(&params);
        let victim = snap
            .iter()
            .filter(|(e, _, _, hash)| *e != me && !kin_hash.is_kin(*hash))
            .map(|(e, p, size, _)| (*e, *size, (*p - pos.0).length_squared()))
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
    let prey_snapshot: Vec<_> = qprey
        .iter()
        .map(|(e, pos, genes)| {
            (
                e,
                *pos,
                genes.size,
                genes.perceptual_hash(&params),
                -genes.diet.into_inner() as f32,
            )
        })
        .collect();

    for (me, mut facing, gpos, pos, genes) in &mut qcreature {
        let r = genes.vision_radius as f32;
        let c_move = move_cost_per_tile(genes, &params);
        let kin_hash = genes.perceptual_hash(&params);

        /* TODO: move cost determination to a new function
         * decoupled from path-finding */
        let food_cost = |food: (&WorldPos, &FoodDetails)| {
            let (food_pos, details) = food;
            let d = (food_pos.0 - pos.0).abs();
            let dist = (d.x + d.y) / TILE_SZ;
            let gain = food_energy(genes, details);
            if gain <= 0.0 {
                return None;
            }
            let cost = (dist * c_move / gain) as f64;
            Some((*food_pos, dist, cost))
        };

        let hunt = |prey_pos: &WorldPos, prey_size: f32| -> Option<(WorldPos, f32, f64)> {
            let d = (prey_pos.0 - pos.0).abs();
            let dist = (d.x + d.y) / TILE_SZ;
            if dist > r {
                return None;
            }
            let cost = hunting_cost(prey_size, dist, genes, &params)?;
            Some((*prey_pos, dist, cost))
        };

        let food = qfood
            .iter()
            .filter_map(food_cost)
            .filter(|(_, d, _)| *d <= r)
            .min_by(|a, b| a.2.total_cmp(&b.2));

        let prey = prey_snapshot
            .iter()
            .filter(|(e, _, _, hash, _)| *e != me && !kin_hash.is_kin(*hash))
            .filter_map(|(_, pos, size, _, _)| hunt(pos, *size))
            .min_by(|a, b| a.2.total_cmp(&b.2));

        let predator = prey_snapshot
            .iter()
            .filter(|(e, _, _, _, _)| *e != me)
            .filter_map(|(_, predator_pos, _, _, aggression)| {
                let distance = (predator_pos.0 - pos.0).abs();
                let dist = (distance.x + distance.y) / TILE_SZ;
                let my_aggression = -genes.diet.into_inner() as f32;
                (*aggression > my_aggression && dist <= r).then_some((*predator_pos, dist))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));

        if let Some((predator_pos, _)) = predator {
            let away = pos.0 - predator_pos.0;
            let direction = if away.x.abs() > away.y.abs() {
                IVec2::new(away.x.signum() as i32, 0)
            } else {
                IVec2::new(0, away.y.signum() as i32)
            };
            let next = gpos.0 + direction;
            if in_bounds(next) {
                facing.0 = Some(direction);
            } else {
                let along_wall = if direction.x != 0 {
                    if away.y >= 0.0 { IVec2::Y } else { IVec2::NEG_Y }
                } else if away.x >= 0.0 {
                    IVec2::X
                } else {
                    IVec2::NEG_X
                };
                facing.0 = if in_bounds(gpos.0 + along_wall) {
                    Some(along_wall)
                } else {
                    Some(-along_wall)
                };
            }
            continue;
        }

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
    mut qcreature: Query<(&WorldPos, &Genes, &mut Stats), With<Creature>>,
    params: Res<SimParams>,
    qfood: Query<(Entity, &WorldPos, &FoodDetails), With<Food>>,
) {
    let eat_radius_squared = TILE_SZ * TILE_SZ / 2.0;
    let mut eaten = EntityHashSet::default();

    for (pos, genes, mut stats) in &mut qcreature {
        for (entity, food_pos, food_details) in &qfood {
            let gain = food_energy(genes, food_details);
            if gain > 0.0
                && pos.0.distance_squared(food_pos.0) <= eat_radius_squared
                && eaten.insert(entity)
            {
                stats.energy = (stats.energy + gain).max(params.max_energy);
                commands.entity(entity).despawn();
            }
        }
    }
}

/// Linear absorption efficiency: plants use 1 + diet, carcasses use 1 - diet.
fn food_energy(genes: &Genes, food: &FoodDetails) -> f32 {
    let diet = genes.diet.into_inner() as f32;
    let efficiency = match food.kind {
        FoodKind::Plant => 1.0 + diet,
        FoodKind::Carcass => 1.0 - diet,
    };
    2.0 * food.quality * food.freshness.into_inner() as f32 * efficiency
}

// Shared by death and hunting so predicted fresh-carcass yield matches actual food.
fn corpse_food(p: &SimParams, prey_size: f32) -> FoodDetails {
    FoodDetails {
        kind: FoodKind::Carcass,
        quality: p.carcass_quality_multiplier * prey_size,
        freshness: UnitInterval::try_new(1.0).unwrap(),
    }
}

fn die(
    mut commands: Commands,
    q: Query<(Entity, &Genes, &WorldPos, &Stats), With<Creature>>,
    p: Res<SimParams>,
) {
    for (entity, genes, pos, Stats { energy, .. }) in &q {
        if *energy <= 0.0 {
            commands.trigger(CreatureDied {
                diet: genes.diet.into_inner() as f32,
            });
            let corpse = corpse_food(&p, genes.size);
            commands.spawn(food::bundle(
                pos.0,
                corpse.kind,
                corpse.quality,
                corpse.freshness,
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
        commands.trigger(CreatureBorn {
            parent_diet: genes.diet.into_inner() as f32,
            offspring_diet: genes_.diet.into_inner() as f32,
        });
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
            energy: rng.random_range(params.initial_energy_range.clone()),
            reproduction_ticks_remaining: rng.random_range(params.reproduction_tick.clone()),
        },
        Transform::from_translation(pos.extend(0.0)),
        Visibility::default(),
        children![(sprite, Transform::from_xyz(0.0, 0.0, 0.5),)],
    )
}

fn spawn(mut commands: Commands, params: Res<SimParams>) {
    let mut rng = rand::rng();
    for _ in 0..params.initial_creature_count {
        let gpos = GridPos(IVec2::new(
            rng.random_range(0..GRID_SIZE),
            rng.random_range(0..GRID_SIZE),
        ));
        let genes = Genes::random(&mut rng, &params);
        commands.spawn(spawn_creature(gpos, genes, &params, &mut rng));
    }
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
        let params = SimParams {
            initial_energy_range: 100.0..=100.0,
            ..default()
        };
        spawn_creature(gpos, genes(), &params, &mut rand::rng())
    }

    fn app_with_time(seconds: f32) -> App {
        let mut app = App::new();
        let mut time = Time::<()>::default();
        time.advance_by(Duration::from_secs_f32(seconds));
        app.insert_resource(time);
        app
    }

    fn assert_genes_in_bounds(genes: &Genes, params: &SimParams) {
        assert!((params.speed_min..=params.speed_max).contains(&genes.speed));
        assert!((params.size_min..=params.size_max).contains(&genes.size));
        assert!(
            (params.vision_radius_min..=params.vision_radius_max).contains(&genes.vision_radius)
        );
        assert!((params.attack_min..=params.attack_max).contains(&genes.attack));
        assert!(
            (params.diet_min as f64..=params.diet_max as f64).contains(&genes.diet.into_inner())
        );
    }

    #[test]
    fn kin_hash_preserves_distance_for_every_gene_including_signed_diet() {
        let params = SimParams::default();
        let low = Genes {
            speed: params.speed_min,
            size: params.size_min,
            diet: SignedUnitInterval::try_new(-1.0).unwrap(),
            vision_radius: params.vision_radius_min,
            attack: params.attack_min,
        };
        let hash = low.perceptual_hash(&params);
        assert_eq!(hash.distance(hash), 0);
        for high in [
            Genes {
                speed: params.speed_max,
                ..low
            },
            Genes {
                size: params.size_max,
                ..low
            },
            Genes {
                diet: SignedUnitInterval::try_new(1.0).unwrap(),
                ..low
            },
            Genes {
                vision_radius: params.vision_radius_max,
                ..low
            },
            Genes {
                attack: params.attack_max,
                ..low
            },
        ] {
            let other = high.perceptual_hash(&params);
            assert_eq!(hash.distance(other), GENE_HASH_BITS);
            assert_eq!(other.distance(hash), GENE_HASH_BITS);
            assert!(!hash.is_kin(other));
        }
    }

    #[test]
    fn kin_hash_tolerates_default_mutations_even_when_every_gene_is_picked() {
        use rand::SeedableRng;

        let mut params = SimParams {
            gene_pick_probability: UnitInterval::try_new(1.0).unwrap(),
            ..default()
        };
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let mut saw_difference = false;
        for _ in 0..1024 {
            let parent = Genes::random(&mut rng, &params);
            let child = parent.mutate(&mut rng, &params);
            let a = parent.perceptual_hash(&params);
            let b = child.perceptual_hash(&params);
            saw_difference |= a.distance(b) > 0;
            assert!(a.is_kin(b));
            assert!(b.is_kin(a));
        }
        assert!(saw_difference);

        // Constant traits carry no distinguishing information and must not divide by zero.
        params.speed_max = params.speed_min;
        params.size_max = params.size_min;
        params.diet_max = params.diet_min;
        params.vision_radius_max = params.vision_radius_min;
        params.attack_max = params.attack_min;
        let parent = Genes::random(&mut rng, &params);
        assert_eq!(parent.perceptual_hash(&params).0, 0);
        assert!(
            parent
                .perceptual_hash(&params)
                .is_kin(parent.mutate(&mut rng, &params).perceptual_hash(&params))
        );
    }

    #[test]
    fn kin_parent_and_newborn_do_not_attack_each_other_or_spend_attack_energy() {
        let params = SimParams {
            initial_energy_range: 100.0..=100.0,
            gene_pick_probability: UnitInterval::try_new(1.0).unwrap(),
            ..default()
        };
        let mut app = app_with_time(1.0);
        app.add_systems(Update, reproduce)
            .add_systems(FixedUpdate, (attack, die).chain());
        let parent_genes = Genes {
            diet: SignedUnitInterval::try_new(-1.0).unwrap(),
            ..genes()
        };
        let parent = app
            .world_mut()
            .spawn(spawn_creature(
                GridPos(IVec2::splat(10)),
                parent_genes,
                &params,
                &mut rand::rng(),
            ))
            .id();
        app.world_mut()
            .get_mut::<Stats>(parent)
            .unwrap()
            .reproduction_ticks_remaining = 1;
        app.insert_resource(params);
        app.update();
        let before: Vec<_> = app
            .world_mut()
            .query_filtered::<(Entity, &Stats), With<Creature>>()
            .iter(app.world())
            .map(|(e, s)| (e, s.energy))
            .collect();
        assert_eq!(before.len(), 2);
        for _ in 0..16 {
            app.world_mut().run_schedule(FixedUpdate);
        }
        for (entity, energy) in before {
            assert_eq!(app.world().get::<Stats>(entity).unwrap().energy, energy);
        }
    }

    #[test]
    fn kin_attack_skips_nearer_relatives_but_still_damages_unrelated_prey() {
        let params = SimParams {
            initial_energy_range: 100.0..=100.0,
            ..default()
        };
        let hunter = Genes {
            diet: SignedUnitInterval::try_new(-1.0).unwrap(),
            ..genes()
        };
        let relative = Genes {
            speed: hunter.speed * 1.05,
            ..hunter
        };
        let prey = Genes {
            diet: SignedUnitInterval::try_new(1.0).unwrap(),
            ..hunter
        };
        let mut app = app_with_time(1.0);
        app.add_systems(Update, attack);
        let mut entities = Vec::new();
        for (offset, genes) in [
            (IVec2::ZERO, hunter),
            (IVec2::X, relative),
            (IVec2::Y * 2, prey),
        ] {
            entities.push(
                app.world_mut()
                    .spawn(spawn_creature(
                        GridPos(IVec2::splat(10) + offset),
                        genes,
                        &params,
                        &mut rand::rng(),
                    ))
                    .id(),
            );
        }
        let expected_hunter_energy = 100.0 - attack_burn(&hunter, &params);
        let expected_relative_energy = 100.0 - attack_burn(&relative, &params);
        app.insert_resource(params);
        app.update();
        assert_eq!(
            app.world().get::<Stats>(entities[0]).unwrap().energy,
            expected_hunter_energy
        );
        assert_eq!(
            app.world().get::<Stats>(entities[1]).unwrap().energy,
            expected_relative_energy
        );
        assert_eq!(
            app.world().get::<Stats>(entities[2]).unwrap().energy,
            100.0 - (hunter.attack + relative.attack) / prey.size
        );
    }

    #[test]
    fn kin_facing_skips_nearer_relatives_for_food_or_unrelated_prey() {
        for include_prey in [false, true] {
            let params = SimParams::default();
            let hunter = Genes {
                diet: SignedUnitInterval::try_new(-1.0).unwrap(),
                ..genes()
            };
            let cell = IVec2::splat(10);
            let mut app = App::new();
            app.add_systems(Update, facing);
            let entity = app
                .world_mut()
                .spawn(spawn_creature(
                    GridPos(cell),
                    hunter,
                    &params,
                    &mut rand::rng(),
                ))
                .id();
            for (offset, genes) in [
                (
                    IVec2::Y,
                    Genes {
                        speed: hunter.speed * 1.05,
                        ..hunter
                    },
                ),
                (
                    IVec2::X * 2,
                    Genes {
                        diet: SignedUnitInterval::try_new(1.0).unwrap(),
                        ..hunter
                    },
                ),
            ]
            .into_iter()
            .take(if include_prey { 2 } else { 1 })
            {
                app.world_mut().spawn(spawn_creature(
                    GridPos(cell + offset),
                    genes,
                    &params,
                    &mut rand::rng(),
                ));
            }
            app.world_mut().spawn(food::bundle(
                grid_to_world(cell + IVec2::X * 3),
                FoodKind::Carcass,
                0.05,
                UnitInterval::try_new(1.0).unwrap(),
            ));
            app.insert_resource(params);
            app.update();
            assert_eq!(app.world().get::<Facing>(entity).unwrap().0, Some(IVec2::X));
        }
    }

    #[test]
    fn kin_filter_preserves_the_diet_and_hunting_cost_tradeoff() {
        for (diet, expected_direction) in [
            (-1.0, IVec2::Y),
            (-0.5, IVec2::Y),
            (-0.1, IVec2::X),
            (1.0, IVec2::X),
        ] {
            let params = SimParams {
                base_cost: 1.0,
                size_max: 1.0,
                speed_max: 1.0,
                attack_max: 10.0,
                w_size: 0.0,
                w_vision: 0.0,
                w_move: 1.0,
                w_attack: 1.0,
                ..default()
            };
            let hunter = Genes {
                diet: SignedUnitInterval::try_new(diet).unwrap(),
                ..genes()
            };
            let prey = Genes {
                diet: SignedUnitInterval::try_new(1.0).unwrap(),
                vision_radius: params.vision_radius_max,
                ..hunter
            };
            let cell = IVec2::splat(10);
            let mut app = App::new();
            app.add_systems(Update, facing);
            let entity = app
                .world_mut()
                .spawn(spawn_creature(
                    GridPos(cell),
                    hunter,
                    &params,
                    &mut rand::rng(),
                ))
                .id();
            app.world_mut().spawn(spawn_creature(
                GridPos(cell + IVec2::Y * 2),
                prey,
                &params,
                &mut rand::rng(),
            ));
            app.world_mut().spawn(food::bundle(
                grid_to_world(cell + IVec2::X * 2),
                FoodKind::Plant,
                8.0,
                UnitInterval::try_new(1.0).unwrap(),
            ));
            app.insert_resource(params);
            app.update();
            assert_eq!(
                app.world().get::<Facing>(entity).unwrap().0,
                Some(expected_direction)
            );
        }
    }

    #[test]
    fn eating_scales_energy_by_diet_and_leaves_indigestible_food() {
        for (diet, plant_gain, carcass_gain) in [
            (-1.0, 0.0, 6.0),
            (-0.5, 1.5, 4.5),
            (0.0, 3.0, 3.0),
            (0.5, 4.5, 1.5),
            (1.0, 6.0, 0.0),
        ] {
            for (kind, expected_gain) in [
                (FoodKind::Plant, plant_gain),
                (FoodKind::Carcass, carcass_gain),
            ] {
                let mut app = App::new();
                app.add_systems(Update, eat);
                let mut genes = genes();
                genes.diet = SignedUnitInterval::try_new(diet).unwrap();
                let params = SimParams {
                    initial_energy_range: 100.0..=100.0,
                    ..default()
                };
                let creature = app
                    .world_mut()
                    .spawn(spawn_creature(
                        GridPos(IVec2::ZERO),
                        genes,
                        &params,
                        &mut rand::rng(),
                    ))
                    .id();
                let food = app
                    .world_mut()
                    .spawn(food::bundle(
                        grid_to_world(IVec2::ZERO),
                        kind,
                        3.0,
                        UnitInterval::try_new(0.5).unwrap(),
                    ))
                    .id();

                app.insert_resource(params);
                app.update();
                assert_eq!(
                    app.world().get::<Stats>(creature).unwrap().energy,
                    100.0 + expected_gain
                );
                assert_eq!(
                    app.world().get::<Food>(food).is_some(),
                    expected_gain == 0.0
                );
            }
        }
    }

    #[test]
    fn food_selection_ignores_food_with_zero_diet_efficiency() {
        for (diet, preferred, ignored) in [
            (-1.0, FoodKind::Carcass, FoodKind::Plant),
            (1.0, FoodKind::Plant, FoodKind::Carcass),
        ] {
            let mut app = App::new();
            app.init_resource::<SimParams>().add_systems(Update, facing);
            let mut genes = genes();
            genes.diet = SignedUnitInterval::try_new(diet).unwrap();
            let cell = IVec2::splat(10);
            let creature = app
                .world_mut()
                .spawn(spawn_creature(
                    GridPos(cell),
                    genes,
                    &SimParams::default(),
                    &mut rand::rng(),
                ))
                .id();
            for (offset, kind) in [(IVec2::X * 2, preferred), (IVec2::Y, ignored)] {
                app.world_mut().spawn(food::bundle(
                    grid_to_world(cell + offset),
                    kind,
                    1.0,
                    UnitInterval::try_new(1.0).unwrap(),
                ));
            }

            app.update();
            assert_eq!(
                app.world().get::<Facing>(creature).unwrap().0,
                Some(IVec2::X)
            );
        }
    }

    #[test]
    fn hunting_uses_absorbed_carcass_energy_and_all_attack_costs() {
        let params = SimParams {
            base_cost: 1.0,
            size_max: 1.0,
            speed_max: 1.0,
            attack_max: 10.0,
            w_size: 0.0,
            w_vision: 0.0,
            w_move: 1.0,
            w_attack: 1.0,
            ..default()
        };
        let mut hunter = genes();
        for (diet, expected_cost) in [(-1.0, 0.075), (-0.5, 0.1)] {
            hunter.diet = SignedUnitInterval::try_new(diet).unwrap();
            // Two tiles cost 1 energy; five seconds of attacking cost 5.
            let cost = hunting_cost(0.5, 2.0, &hunter, &params).unwrap();
            assert!((cost - expected_cost).abs() < 1e-6);
        }
        // At 70 energy spent, the 60-energy yield loses; the 80-energy yield pays.
        assert!(hunting_cost(0.5, 130.0, &hunter, &params).is_none());
        hunter.diet = SignedUnitInterval::try_new(-1.0).unwrap();
        assert!(hunting_cost(0.5, 130.0, &hunter, &params).is_some());
        assert!(hunting_cost(0.5, 150.0, &hunter, &params).is_none());
    }

    #[test]
    fn only_carnivorous_creatures_hunt_or_attack_even_with_a_negative_aggression_threshold() {
        for diet in [-1.0, 0.0, 0.5, 1.0] {
            let params = SimParams {
                min_aggression: -1.0,
                initial_energy_range: 100.0..=100.0,
                ..default()
            };
            let mut attacker_genes = genes();
            attacker_genes.diet = SignedUnitInterval::try_new(diet).unwrap();
            assert_eq!(
                hunting_cost(0.5, 0.0, &attacker_genes, &params).is_some(),
                diet < 0.0
            );
            let mut app = app_with_time(1.0);
            app.add_systems(Update, attack);
            let attacker = app
                .world_mut()
                .spawn(spawn_creature(
                    GridPos(IVec2::ZERO),
                    attacker_genes,
                    &params,
                    &mut rand::rng(),
                ))
                .id();
            let prey = Genes {
                speed: params.speed_max,
                vision_radius: params.vision_radius_max,
                ..genes()
            };
            let target = app
                .world_mut()
                .spawn(spawn_creature(
                    GridPos(IVec2::X),
                    prey,
                    &params,
                    &mut rand::rng(),
                ))
                .id();
            app.insert_resource(params);

            app.update();
            assert_eq!(
                app.world().get::<Stats>(attacker).unwrap().energy < 100.0,
                diet < 0.0
            );
            assert_eq!(
                app.world().get::<Stats>(target).unwrap().energy,
                if diet < 0.0 { 80.0 } else { 100.0 }
            );
        }
    }

    #[test]
    fn startup_scatters_the_configured_population_with_bounded_genes() {
        for count in [0, 32] {
            let params = SimParams {
                initial_creature_count: count,
                speed_min: 2.0,
                speed_max: 3.0,
                size_min: 0.2,
                size_max: 0.3,
                vision_radius_min: 6,
                vision_radius_max: 8,
                attack_min: 3.0,
                attack_max: 4.0,
                diet_min: -0.6,
                diet_max: -0.4,
                ..default()
            };
            let mut app = App::new();
            app.insert_resource(params).add_systems(Startup, spawn);
            app.update();

            let mut query = app
                .world_mut()
                .query_filtered::<(&GridPos, &WorldPos, &Genes, &Stats), With<Creature>>();
            let params = app.world().resource::<SimParams>();
            assert_eq!(query.iter(app.world()).count(), count as usize);
            for (grid, pos, genes, stats) in query.iter(app.world()) {
                assert!(in_bounds(grid.0));
                assert_eq!(pos.0, grid_to_world(grid.0));
                assert_genes_in_bounds(genes, params);
                assert!(
                    params
                        .reproduction_tick
                        .contains(&stats.reproduction_ticks_remaining)
                );
            }
        }
    }

    #[test]
    fn mutation_keeps_random_genes_within_configured_bounds() {
        use rand::SeedableRng;

        let mut params = SimParams {
            speed_min: 2.0,
            speed_max: 3.0,
            size_min: 0.2,
            size_max: 0.3,
            vision_radius_min: 6,
            vision_radius_max: 8,
            attack_min: 3.0,
            attack_max: 4.0,
            diet_min: 0.4,
            diet_max: 0.6,
            gene_pick_probability: UnitInterval::try_new(1.0).unwrap(),
            ..default()
        };
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        for factor in [0.0, 1.0] {
            params.gene_mutation_factor = UnitInterval::try_new(factor).unwrap();
            let mut genes = Genes::random(&mut rng, &params);
            for _ in 0..128 {
                genes = genes.mutate(&mut rng, &params);
                assert_genes_in_bounds(&genes, &params);
            }
        }
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
            initial_energy_range: 100.0..=100.0,
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
        #[derive(Resource, Default)]
        struct Events {
            births: usize,
            deaths: usize,
        }

        for energy in [0.0, 49.0, 50.0, 51.0] {
            let mut app = App::new();
            app.init_resource::<SimParams>()
                .init_resource::<Events>()
                .add_observer(|event: On<CreatureBorn>, mut events: ResMut<Events>| {
                    assert_eq!(event.parent_diet, 0.0);
                    assert_eq!(event.offspring_diet, 0.0);
                    events.births += 1;
                })
                .add_observer(|event: On<CreatureDied>, mut events: ResMut<Events>| {
                    assert_eq!(event.diet, 0.0);
                    events.deaths += 1;
                })
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
            let events = app.world().resource::<Events>();
            assert_eq!(events.births, usize::from(energy > 50.0));
            assert_eq!(events.deaths, usize::from(energy <= 50.0));
            if energy > 50.0 {
                assert_eq!(creatures, 2);
                assert_eq!(carcasses, 0);
                let stats = app.world().get::<Stats>(parent).unwrap();
                assert_eq!(stats.energy, 1.0);
                assert!(
                    app.world()
                        .resource::<SimParams>()
                        .reproduction_tick
                        .contains(&stats.reproduction_ticks_remaining)
                );
            } else {
                assert_eq!(creatures, 0);
                assert_eq!(carcasses, 1);
                assert!(app.world().get::<Creature>(parent).is_none());
            }
        }
    }
}
