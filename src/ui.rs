use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use crate::camera::Follow;
use crate::creature::{Creature, Genes, GridPos, Stats};
use crate::ranged::SignedUnitInterval;
use crate::sim::SimParams;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(EguiPrimaryContextPass, (inspector, params_panel));
    }
}

fn inspector(
    mut contexts: EguiContexts,
    follow: Res<Follow>,
    params: Res<SimParams>,
    mut creatures: Query<(&mut Genes, &Stats, &GridPos), With<Creature>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    egui::Window::new("Creature").show(ctx, |ui| {
        let Some(e) = follow.0 else {
            ui.label("Middle-click a creature");
            return;
        };
        let Ok((mut genes, stats, grid)) = creatures.get_mut(e) else {
            ui.label("(creature gone)");
            return;
        };
        ui.label(format!("Tile: {}, {}", grid.0.x, grid.0.y));
        ui.separator();
        ui.heading("Genes");
        ui.add(egui::Slider::new(&mut genes.speed, 0.5..=8.0).text("speed (tiles/s)"));
        ui.add(egui::Slider::new(&mut genes.size, 0.0..=1.0).text("size (fraction of tile_size)"));
        let mut temp_diet = genes.diet.into_inner();
        ui.add(egui::Slider::new(&mut temp_diet, -1.0..=1.0).text("diet (continuous range from -1(carnivore) to 1(herbivore)"));
        genes.diet = SignedUnitInterval::try_new(temp_diet).unwrap();
        ui.add(egui::Slider::new(&mut genes.vision_radius, 4..=32).text("vision radius (tiles)"));
        ui.separator();
        ui.heading("State (energy)");
        let frac = (stats.energy/100.0).clamp(0.0, 1.0);
        ui.add(egui::ProgressBar::new(frac).text(format!("{:.1} / {:.0}", stats.energy, 100.0)));
    });
}

fn params_panel(mut contexts: EguiContexts, mut params: ResMut<SimParams>) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    egui::Window::new("Sim params").show(ctx, |ui| {
        ui.add(egui::Slider::new(&mut params.base_cost, 1.0..=20.0).text("base cost"));
    });
}
