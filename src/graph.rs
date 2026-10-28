//! Grid-snapped box selection and regional mean gene history, measured in simulation time.
use crate::{
    GRID_SIZE, TILE_SZ,
    creature::{Creature, Genes, WorldPos},
};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use std::collections::VecDeque;

const INTERVAL: f64 = 0.5;
const MAX_SAMPLES: usize = 1800;
const COLORS: [egui::Color32; 2] = [
    egui::Color32::from_rgb(70, 180, 255),
    egui::Color32::from_rgb(255, 175, 65),
];
const LABELS: [&str; 2] = ["Speed (tiles/s)", "Size (tile fraction)"];

pub struct GraphPlugin;
impl Plugin for GraphPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GraphState>()
            .add_systems(EguiPrimaryContextPass, graph);
    }
}
#[derive(Default, Resource)]
struct GraphState {
    drag_start: Option<Vec2>,
    draft: Option<Rect>,
    region: Option<Rect>,
    baseline: Option<[f32; 2]>,
    history: VecDeque<Sample>,
    started: f64,
    last_sample: Option<f64>,
}
struct Sample {
    time: f64,
    count: usize,
    mean: Option<[f32; 2]>,
}
// Include the starting and ending tiles, irrespective of drag direction.
fn snapped_box(start: Vec2, end: Vec2) -> Rect {
    let tile = |p: Vec2| {
        (p / TILE_SZ)
            .floor()
            .clamp(Vec2::ZERO, Vec2::splat((GRID_SIZE - 1) as f32))
    };
    let (a, b) = (tile(start), tile(end));
    Rect::from_corners(a.min(b) * TILE_SZ, (a.max(b) + Vec2::ONE) * TILE_SZ)
}
fn percent(value: f32, baseline: f32) -> Option<f32> {
    if baseline.abs() < 1e-6 {
        return None;
    }
    let result = 100.0 * (value / baseline - 1.0);
    result.is_finite().then_some(result)
}

fn graph(
    mut contexts: EguiContexts,
    camera: Single<(&Camera, &GlobalTransform), With<Camera2d>>,
    creatures: Query<(&WorldPos, &Genes), With<Creature>>,
    time: Res<Time<Virtual>>,
    mut state: ResMut<GraphState>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let (cam, transform) = camera.into_inner();
    let now = time.elapsed_secs_f64();
    let (pressed, released, pointer, escape) = ctx.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_released(),
            i.pointer.interact_pos(),
            i.key_pressed(egui::Key::Escape),
        )
    });
    if escape {
        state.drag_start = None;
        state.draft = None;
    }
    let pointer_world = pointer.and_then(|p| {
        cam.viewport_to_world_2d(transform, Vec2::new(p.x, p.y))
            .ok()
    });
    if pressed && !escape && !ctx.is_pointer_over_egui() {
        if let Some(world) = pointer_world {
            let limit = Vec2::splat(GRID_SIZE as f32 * TILE_SZ);
            if world.cmpge(Vec2::ZERO).all() && world.cmplt(limit).all() {
                state.drag_start = Some(world);
                state.draft = Some(snapped_box(world, world));
            }
        }
    }
    if let Some(start) = state.drag_start {
        if let Some(end) = pointer_world {
            state.draft = Some(snapped_box(start, end));
        }
        if released {
            if let Some(region) = state.draft {
                *state = GraphState {
                    region: Some(region),
                    started: now,
                    ..Default::default()
                };
            }
            state.drag_start = None;
        }
    }
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("box_selection"),
    ));
    let screen = |p: Vec2| {
        cam.world_to_viewport(transform, p.extend(0.5))
            .ok()
            .map(|p| egui::pos2(p.x, p.y))
    };
    for (region, color) in [
        (state.region, egui::Color32::from_rgb(110, 220, 210)),
        (state.draft, egui::Color32::WHITE),
    ] {
        if let Some(region) = region {
            let corners = [
                region.min,
                Vec2::new(region.max.x, region.min.y),
                region.max,
                Vec2::new(region.min.x, region.max.y),
            ];
            let points: Vec<_> = corners.into_iter().filter_map(screen).collect();
            if points.len() == 4 {
                painter.add(egui::Shape::closed_line(
                    points,
                    egui::Stroke::new(2.0, color),
                ));
            }
        }
    }
    // Share one membership check between live highlights, count, and sampled averages.
    let (mut count, mut total) = (0, [0.0; 2]);
    for (pos, genes) in &creatures {
        let center = pos.0 + Vec2::new(0.0, TILE_SZ * 0.25);
        if !state
            .region
            .is_some_and(|r| center.cmpge(r.min).all() && center.cmplt(r.max).all())
        {
            continue;
        }
        count += 1;
        total[0] += genes.speed;
        total[1] += genes.size;
        if let Some(p) = screen(center) {
            painter.circle_stroke(p, 9.0, egui::Stroke::new(1.5, COLORS[0]));
        }
    }
    let mean = (count > 0).then(|| total.map(|v| v / count as f32));
    if state.region.is_some() && state.last_sample.is_none_or(|last| now - last >= INTERVAL) {
        if state.baseline.is_none() {
            state.baseline = mean;
        }
        let sample = Sample {
            time: now - state.started,
            count,
            mean,
        };
        state.history.push_back(sample);
        if state.history.len() > MAX_SAMPLES {
            state.history.pop_front();
        }
        state.last_sample = Some(now);
    }
    egui::Window::new("Regional gene graph")
        .default_width(560.0)
        .show(ctx, |ui| {
            ui.label(
                "Left-drag a box; edges snap to whole tiles. Release to select; Escape cancels.",
            );
            ui.small("Right-drag: pan | Wheel: zoom | Middle-click: follow creature");
            if state.region.is_none() {
                ui.label("Drag a box to begin. Click selects one tile.");
                return;
            }
            ui.label(
                "Tracks current occupants of this fixed area, including newcomers and offspring.",
            );
            ui.small("Blue rings mark occupants. Gene curves use their arithmetic means.");
            if ui.button("Clear selection and history").clicked() {
                *state = GraphState::default();
                return;
            }
            ui.label(format!("Creatures inside now: {count}"));
            if let Some(latest) = state.history.back() {
                ui.small(format!("Last recorded count: {}", latest.count));
            }
            let Some(baseline) = state.baseline else {
                ui.label("Waiting for a nonempty sample to set the baseline.");
                return;
            };
            egui::Grid::new("gene_values").striped(true).show(ui, |ui| {
                for heading in ["Gene", "Initial mean", "Latest sampled mean"] {
                    ui.label(heading);
                }
                ui.end_row();
                for i in 0..2 {
                    ui.colored_label(COLORS[i], LABELS[i]);
                    ui.label(format!("{:.3}", baseline[i]));
                    ui.label(
                        state
                            .history
                            .back()
                            .and_then(|s| s.mean)
                            .map_or_else(|| "-".into(), |m| format!("{:.3}", m[i])),
                    );
                    ui.end_row();
                }
            });
            ui.label("Y: change from initial mean (%) | X: simulation seconds since selection");
            ui.small(
                "0% = initial mean; +20% = 1.2x. Sampled every 0.5s; last 15 minutes retained.",
            );
            if baseline.iter().any(|v| v.abs() < 1e-6) {
                ui.label("Zero initial mean: percentage curve unavailable.");
            }
            if count == 0 {
                ui.label("Area empty: recording gaps; new arrivals are included automatically.");
            }
            plot(ui, &state, baseline);
        });
}

fn plot(ui: &mut egui::Ui, state: &GraphState, baseline: [f32; 2]) {
    let (response, painter) = ui.allocate_painter(
        egui::vec2(ui.available_width().max(200.0), 260.0),
        egui::Sense::hover(),
    );
    let chart = egui::Rect::from_min_max(
        response.rect.min + egui::vec2(55.0, 10.0),
        response.rect.max - egui::vec2(12.0, 30.0),
    );
    let first = state.history.front().map_or(0.0, |s| s.time);
    let end = state
        .history
        .back()
        .map_or(first + 1.0, |s| s.time)
        .max(first + 1.0);
    let (mut low, mut high) = (-10.0_f32, 10.0_f32);
    for sample in &state.history {
        if let Some(mean) = sample.mean {
            for i in 0..2 {
                if let Some(y) = percent(mean[i], baseline[i]) {
                    low = low.min(y);
                    high = high.max(y);
                }
            }
        }
    }
    let padding = (high - low) * 0.1;
    low -= padding;
    high += padding;
    let project = |t: f64, y: f32| {
        egui::pos2(
            chart.left() + ((t - first) / (end - first)) as f32 * chart.width(),
            chart.bottom() - (y - low) / (high - low) * chart.height(),
        )
    };
    let font = egui::FontId::monospace(11.0);
    for tick in 0..=4 {
        let y = low + (high - low) * tick as f32 / 4.0;
        let p = project(first, y);
        painter.line_segment(
            [p, egui::pos2(chart.right(), p.y)],
            egui::Stroke::new(1.0, egui::Color32::from_gray(60)),
        );
        painter.text(
            p - egui::vec2(5.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            format!("{y:.1}%"),
            font.clone(),
            egui::Color32::LIGHT_GRAY,
        );
        let t = first + (end - first) * tick as f64 / 4.0;
        painter.text(
            egui::pos2(project(t, low).x, chart.bottom() + 6.0),
            egui::Align2::CENTER_TOP,
            format!("{t:.0}s"),
            font.clone(),
            egui::Color32::LIGHT_GRAY,
        );
    }
    painter.line_segment(
        [project(first, 0.0), project(end, 0.0)],
        egui::Stroke::new(1.0, egui::Color32::GRAY),
    );
    for gene in 0..2 {
        let mut previous = None;
        for sample in &state.history {
            let point = sample
                .mean
                .and_then(|m| percent(m[gene], baseline[gene]))
                .map(|y| project(sample.time, y));
            if let Some(p) = point {
                if let Some(last) = previous {
                    painter.line_segment([last, p], egui::Stroke::new(2.0, COLORS[gene]));
                }
                painter.circle_filled(p, 2.0, COLORS[gene]);
            }
            previous = point;
        }
    }
}
