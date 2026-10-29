use std::ops::RangeInclusive;

use bevy::prelude::*;

use crate::ranged::UnitInterval;

#[derive(Resource)]
pub struct SimParams {
    pub initial_creature_count: u32,
    pub base_cost: f32,
    pub food_decay_rate: f32,
    pub gene_pick_probability: UnitInterval,
    pub gene_mutation_factor: UnitInterval,
    pub reproduction_tick: RangeInclusive<u32>,
    pub reproduction_cost: f32,
    pub attack_radius: f32,
    pub carcass_quality_multiplier: f32,

    pub speed_min: f32,
    pub speed_max: f32,
    pub size_min: f32,
    pub size_max: f32,
    pub vision_radius_min: u32,
    pub vision_radius_max: u32,
    pub attack_min: f32,
    pub attack_max: f32,
    pub diet_min: f32,
    pub diet_max: f32,

    pub min_aggression: f32,

    pub w_size: f32,
    pub w_move: f32,
    pub w_vision: f32,
    pub w_attack: f32,

    pub max_energy: f32,
}

impl Default for SimParams {
    fn default() -> Self {
        Self {
            initial_creature_count: 100,
            base_cost: 0.8,
            food_decay_rate: 0.05,
            gene_pick_probability: UnitInterval::try_new(0.3).unwrap(),
            gene_mutation_factor: UnitInterval::try_new(0.1).unwrap(),
            reproduction_tick: 800..=1200,
            reproduction_cost: 50.0,
            attack_radius: 3.0,
            carcass_quality_multiplier: 40.0,

            speed_min: 0.1,
            speed_max: 10.0,
            size_min: 0.05,
            size_max: 0.95,
            vision_radius_min: 4,
            vision_radius_max: 32,
            attack_min: 0.1,
            attack_max: 30.0,
            diet_min: -1.0,
            diet_max: 1.0,

            min_aggression: 0.0,

            w_size: 0.5,
            w_move: 0.35,
            w_vision: 0.15,
            w_attack: 0.3,

            max_energy: 100.0,
        }
    }
}
