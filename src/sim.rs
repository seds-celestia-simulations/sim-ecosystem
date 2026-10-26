use std::num::NonZeroU32;

use bevy::prelude::*;

#[derive(Resource)]
pub struct SimParams {
    pub base_cost: f32,
    pub food_decay_rate: f32,
    pub gene_pick_probability: f32,
    pub gene_mutation_factor: f32,
    pub reproduction_tick: NonZeroU32,
}

impl Default for SimParams {
    fn default() -> Self {
        Self {
            base_cost: 0.1,
            food_decay_rate: 0.6,
            gene_pick_probability: 0.3,
            gene_mutation_factor: 0.1,
            reproduction_tick: NonZeroU32::new(5000).expect("100 is not zero"),
        }
    }
}
