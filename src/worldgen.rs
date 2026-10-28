use bevy::prelude::*;
use noise::{Fbm, MultiFractal, NoiseFn, Perlin};
use rand::RngExt;

use crate::food::{FoodKind, bundle};
use crate::{GRID_SIZE, TILE_SZ, grid_to_world};
use crate::ranged::UnitInterval;


// ---------------------------------------------------------------------------
// Tunables
// ---------------------------------------------------------------------------

// Lower frequency = bigger biome regions.
const TEMP_FREQUENCY: f64 = 0.02;
const HUMIDITY_FREQUENCY: f64 = 0.025;

// Fbm output clusters near the middle, so stretch it to use the full 0..1 range.
const NOISE_CONTRAST: f32 = 1.4;

// Fewer octaves = less fine detail = fewer tiny pockets to begin with.
const OCTAVES: usize = 2;

// Any connected patch of one biome smaller than this many tiles gets absorbed
// into its most common neighbouring biome. (128 * 128 = 16384 tiles total.)
const MIN_REGION_SIZE: usize = 200;

// Biome thresholds (all values are in 0..1).
const COLD_BELOW: f32 = 0.40;
const HOT_ABOVE: f32 = 0.55;
const DRY_BELOW: f32 = 0.40;
const WET_ABOVE: f32 = 0.60;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Biome {
    Snowy,
    Desert,
    Rainforest,
    Grassland,
}

impl Biome {
    const ALL: [Biome; 4] = [
        Biome::Snowy,
        Biome::Desert,
        Biome::Rainforest,
        Biome::Grassland,
    ];

    fn index(self) -> usize {
        match self {
            Biome::Snowy => 0,
            Biome::Desert => 1,
            Biome::Rainforest => 2,
            Biome::Grassland => 3,
        }
    }

    fn color(self) -> Color {
        match self {
            Biome::Snowy => Color::srgb(0.93, 0.95, 0.98),
            Biome::Desert => Color::srgb(0.92, 0.80, 0.50),
            Biome::Rainforest => Color::srgb(0.16, 0.50, 0.22),
            Biome::Grassland => Color::srgb(0.72, 0.78, 0.60),
        }
    }

    fn plant_density(self) -> f32 {
        match self {
            Biome::Rainforest => 0.30,
            Biome::Grassland => 0.12,
            Biome::Desert => 0.01,
            Biome::Snowy => 0.005,
        }
    }

    pub fn decay_multiplier(self) -> f32 {
        match self {
            Biome::Rainforest => 1.5,
            Biome::Grassland => 1.0,
            Biome::Desert => 2.0,
            Biome::Snowy => 0.5,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Tile {
    pub temperature: f32, // 0.0 (freezing) ..= 1.0 (scorching)
    pub humidity: f32,    // 0.0 (arid) ..= 1.0 (soaked)
    pub biome: Biome,
}

/// The generated map. Other modules (creatures, food) can query it.
#[derive(Resource)]
pub struct WorldGrid {
    tiles: Vec<Tile>,
}

impl WorldGrid {
    pub fn get(&self, p: IVec2) -> Option<&Tile> {
        if p.cmplt(IVec2::ZERO).any() || p.cmpge(IVec2::splat(GRID_SIZE)).any() {
            return None;
        }
        self.tiles.get((p.y * GRID_SIZE + p.x) as usize)
    }

    pub fn biome_at(&self, p: IVec2) -> Option<Biome> {
        self.get(p).map(|t| t.biome)
    }
}

// ---------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------

/// Noise gives roughly -1..1; remap to 0..1 with a bit of contrast.
fn normalize(v: f64) -> f32 {
    ((v as f32 * NOISE_CONTRAST) * 0.5 + 0.5).clamp(0.0, 1.0)
}

fn classify(temperature: f32, humidity: f32) -> Biome {
    match (temperature, humidity) {
        (t, _) if t < COLD_BELOW => Biome::Snowy,
        (t, h) if t > HOT_ABOVE && h < DRY_BELOW => Biome::Desert,
        (t, h) if t > HOT_ABOVE && h > WET_ABOVE => Biome::Rainforest,
        _ => Biome::Grassland,
    }
}

fn generate(seed: u32) -> WorldGrid {
    let temperature_noise = Fbm::<Perlin>::new(seed)
        .set_octaves(OCTAVES)
        .set_frequency(TEMP_FREQUENCY)
        .set_persistence(0.5)
        .set_lacunarity(2.0);

    // Different seed, so humidity is independent of temperature.
    let humidity_noise = Fbm::<Perlin>::new(seed.wrapping_add(1))
        .set_octaves(OCTAVES)
        .set_frequency(HUMIDITY_FREQUENCY)
        .set_persistence(0.5)
        .set_lacunarity(2.0);

    let mut tiles = Vec::with_capacity((GRID_SIZE * GRID_SIZE) as usize);

    // Row-major order (y outer, x inner) to match WorldGrid::get.
    for y in 0..GRID_SIZE {
        for x in 0..GRID_SIZE {
            let p = [x as f64, y as f64];
            let temperature = normalize(temperature_noise.get(p));
            let humidity = normalize(humidity_noise.get(p));

            tiles.push(Tile {
                temperature,
                humidity,
                biome: classify(temperature, humidity),
            });
        }
    }

    remove_small_regions(&mut tiles);

    WorldGrid { tiles }
}

/// A connected patch of one biome (4-connected), found by flood fill.
struct Region {
    tiles: Vec<usize>,
    /// How many edges this region shares with each neighbouring biome (indexed by Biome::index).
    border: [usize; 4],
}

fn find_regions(tiles: &[Tile]) -> Vec<Region> {
    let n = GRID_SIZE as usize;
    let mut visited = vec![false; n * n];
    let mut regions = Vec::new();

    for start in 0..n * n {
        if visited[start] {
            continue;
        }
        let biome = tiles[start].biome;
        let mut region = Region {
            tiles: vec![start],
            border: [0; 4],
        };
        visited[start] = true;

        let mut i = 0;
        while i < region.tiles.len() {
            let idx = region.tiles[i];
            i += 1;
            let (x, y) = (idx % n, idx / n);

            let neighbours = [
                (x > 0).then(|| idx - 1),
                (x + 1 < n).then(|| idx + 1),
                (y > 0).then(|| idx - n),
                (y + 1 < n).then(|| idx + n),
            ];

            for nb in neighbours.into_iter().flatten() {
                if tiles[nb].biome == biome {
                    if !visited[nb] {
                        visited[nb] = true;
                        region.tiles.push(nb);
                    }
                } else {
                    region.border[tiles[nb].biome.index()] += 1;
                }
            }
        }
        regions.push(region);
    }

    regions
}

/// Repeatedly merges the smallest undersized region into the biome it touches most,
/// until every region has at least MIN_REGION_SIZE tiles.
fn remove_small_regions(tiles: &mut [Tile]) {
    loop {
        let regions = find_regions(tiles);

        let Some(smallest) = regions
            .iter()
            .filter(|r| r.tiles.len() < MIN_REGION_SIZE)
            .min_by_key(|r| r.tiles.len())
        else {
            break; // nothing left to merge
        };

        // Pick the neighbouring biome that shares the longest border.
        let (best, count) = smallest
            .border
            .iter()
            .copied()
            .enumerate()
            .max_by_key(|&(_, c)| c)
            .unwrap();
        if count == 0 {
            break; // region has no neighbours (whole map is one biome)
        }

        let new_biome = Biome::ALL[best];
        for &idx in &smallest.tiles {
            tiles[idx].biome = new_biome;
        }
    }
}

// ---------------------------------------------------------------------------
// Spawning
// ---------------------------------------------------------------------------

fn spawn_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let seed: u32 = rand::random();
    let grid = generate(seed);

    // One shared mesh for every tile.
    let square = meshes.add(Rectangle::new(TILE_SZ, TILE_SZ));

    // One material per biome, in the same order as Biome::ALL.
    let handles: Vec<Handle<ColorMaterial>> = Biome::ALL
        .iter()
        .map(|biome| materials.add(biome.color()))
        .collect();

    for y in 0..GRID_SIZE {
        for x in 0..GRID_SIZE {
            let cell = IVec2::new(x, y);
            let biome = grid.biome_at(cell).unwrap();

            let biome_index = Biome::ALL.iter().position(|b| *b == biome).unwrap();
            let material = handles[biome_index].clone();

            let p = grid_to_world(cell);
            commands.spawn((
                Mesh2d(square.clone()),
                MeshMaterial2d(material),
                Transform::from_xyz(p.x, p.y, 0.0),
            ));
        }
    }

    commands.insert_resource(grid);
}

fn spawn_plants(mut commands: Commands, world: Res<WorldGrid>) {
    let mut rng = rand::rng();

    for y in 0..GRID_SIZE {
        for x in 0..GRID_SIZE {
            let cell = IVec2::new(x, y);

            let Some(biome) = world.biome_at(cell) else {
                continue;
            };

            // Roll the dice for this tile.
            if rng.random::<f32>() >= biome.plant_density() {
                continue;
            }

            let quality = rng.random_range(0.5..1.0);
            let freshness = rng.random_range(-0.5..1.0);
            let freshness = UnitInterval::new_clamped(freshness);
            commands.spawn(bundle(
                grid_to_world(cell),
                FoodKind::Plant,
                quality,
                freshness,
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct WorldGenPlugin;

impl Plugin for WorldGenPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_world)
            .add_systems(PostStartup, spawn_plants);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grassland() -> WorldGrid {
        WorldGrid {
            tiles: vec![
                Tile {
                    temperature: 0.5,
                    humidity: 0.5,
                    biome: Biome::Grassland
                };
                (GRID_SIZE * GRID_SIZE) as usize
            ],
        }
    }

    #[test]
    fn grid_lookup_rejects_out_of_bounds_coordinates_instead_of_aliasing_other_rows() {
        let grid = grassland();
        assert!(grid.get(IVec2::ZERO).is_some());
        assert!(grid.get(IVec2::splat(GRID_SIZE - 1)).is_some());
        for cell in [
            IVec2::new(GRID_SIZE, 0),
            IVec2::new(-1, 1),
            IVec2::new(0, -1),
            IVec2::new(0, GRID_SIZE),
        ] {
            assert!(grid.get(cell).is_none());
        }
    }

    #[test]
    fn fixed_simulation_decomposes_food_before_creatures_choose_and_eat_it() {
        use crate::creature::{Creature, CreaturePlugin, Stats};
        use crate::food::{Food, FoodPlugin};
        use std::time::Duration;

        let mut app = App::new();
        let mut time = Time::<()>::default();
        time.advance_by(Duration::from_secs_f32(1.0 / 64.0));
        app.insert_resource(time)
            .insert_resource(grassland())
            .add_plugins((CreaturePlugin, FoodPlugin));
        app.update();

        let pos = grid_to_world(IVec2::ZERO);
        app.world_mut()
            .spawn(bundle(pos, FoodKind::Carcass, 1.0, -1.0));
        app.world_mut()
            .spawn(bundle(pos, FoodKind::Plant, 1.0, 1.0));
        app.world_mut().run_schedule(FixedUpdate);

        assert_eq!(
            app.world_mut()
                .query_filtered::<&Stats, With<Creature>>()
                .single(app.world())
                .unwrap()
                .energy,
            110.0
        );
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<Food>>()
                .iter(app.world())
                .count(),
            0
        );
    }
}
