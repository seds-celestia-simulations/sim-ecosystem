use bevy::prelude::*;
use rand::{Rng, RngExt};

use crate::GRID_SIZE;
use crate::TILE_SZ;
use crate::food;
use crate::food::{Food, FoodKind};
use crate::grid_to_world;
use crate::sim::SimParams;

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
    pub speed: f32, /* tiles / dt */
    pub size: f32,  /* fraction of TILE_SZ */
}

impl Genes {
    fn mutate(&self, rng: &mut impl Rng, params: &Res<SimParams>) -> Self {
        let p = params.gene_pick_probability;
        let f = params.gene_mutation_factor;
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

#[derive(Resource, Default)]
pub struct Counter(u32);

impl Plugin for CreaturePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SimParams>()
            .init_resource::<Counter>()
            .add_systems(Startup, spawn)
            .add_systems(Update, (update, facing))
            .add_systems(
                FixedUpdate,
                (start_step, advance, metabolize, die, reproduce).chain(),
            );
    }
}

fn facing(keys: Res<ButtonInput<KeyCode>>, mut q: Query<&mut Facing, With<Creature>>) {
    let dir = if keys.pressed(KeyCode::ArrowUp) {
        Some(IVec2::new(0, 1))
    } else if keys.pressed(KeyCode::ArrowRight) {
        Some(IVec2::new(1, 0))
    } else if keys.pressed(KeyCode::ArrowDown) {
        Some(IVec2::new(0, -1))
    } else if keys.pressed(KeyCode::ArrowLeft) {
        Some(IVec2::new(-1, 0))
    } else {
        None
    };
    for mut f in &mut q {
        f.0 = dir;
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
 *       maybe abstract this to it's own render module?
 * FIXME: introduce an explicit Z-Ordering for shadows, food, creatures, etc. */
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
        /* FIXME: better way to sort out moving?
         * the current system just checks whether the sprite is very close to
         * the tile center and accepts that as stationary. This could be a bug */

        let moving = pos.0.distance(gpos.0.as_vec2()) > 1e-4;

        let moving = if moving { 1.0 } else { 0.0 };

        /* TODO: possibly a better abstraction for calculating cost of particular
         * characterestics / stats? */

        let cost = params.base_cost * genes.size.powf(0.75) * genes.speed.powf(2.0) * moving;

        stats.energy -= (time.delta_secs() * cost).max(0.0);
    }
}

fn die(mut commands: Commands, q: Query<(Entity, &Genes, &WorldPos, &Stats), With<Creature>>) {
    for (entity, _, pos, Stats { energy }) in &q {
        if *energy <= 0.0 {
            commands.spawn(food::bundle(
                pos.0,
                FoodKind::Carcass,
                20.0, /* TODO: food quality proportional to size gene and energy */
            ));

            commands.entity(entity).despawn();
        }
    }
}

fn in_bounds(p: IVec2) -> bool {
    p.cmpge(IVec2::ZERO).all() && p.cmplt(IVec2::splat(GRID_SIZE)).all()
}

fn pick_direction(gpos: &GridPos) -> GridPos {
    for i in -1..=1 {
        for j in -1..=1 {
            if i == 0 && j == 0 {
                continue;
            }
            let p = gpos.0 + IVec2::new(i, j);
            if in_bounds(p) {
                return GridPos(p);
            }
        }
    }

    panic!("atleast one direction should be in bounds")
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
        let gpos_ = pick_direction(gpos);
        commands.spawn(spawn_creature(gpos_, genes_));
    }
}

fn spawn_creature(gpos: GridPos, genes: Genes) -> impl Bundle {
    let pos = gpos.0.as_vec2() * TILE_SZ;
    let rect = Vec2::splat(TILE_SZ * genes.size);
    let sprite = Sprite::from_color(Color::srgb(0.2, 0.7, 0.5), rect);

    (
        Creature,
        WorldPos(pos),
        gpos,
        genes,
        Facing::default(),
        Stats::default(),
        Transform::default(),
        Visibility::default(),
        children![(sprite, Transform::from_translation(pos.extend(0.5)),)],
    )
}

fn spawn(mut commands: Commands) {
    /* TODO: procedural color
     * TODO: default position */

    let gpos = GridPos(IVec2::new(0, 0));
    let genes = Genes {
        speed: 4.0,
        size: 0.5,
    };

    commands.spawn(spawn_creature(gpos, genes));
}
