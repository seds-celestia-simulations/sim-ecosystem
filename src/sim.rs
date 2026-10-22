use std::num::NonZeroU32;

use bevy::prelude::*;

#[derive(Resource)]
pub struct SimParams {
    pub base_cost: f32,
    pub food_decay_rate: f32,
    pub reproduction_tick: NonZeroU32,
}

impl Default for SimParams {
    fn default() -> Self {
        Self {
            base_cost: 0.1,
            food_decay_rate: 0.6,
            reproduction_tick: NonZeroU32::new(100).expect("100 is not zero"),
        }
    }
}
