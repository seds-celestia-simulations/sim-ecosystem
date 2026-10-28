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
const COLORS: [egui::Color32; 4] = [
    egui::Color32::from_rgb(70, 180, 255),
    egui::Color32::from_rgb(255, 175, 65),
    egui::Color32::from_rgb(140, 220, 150),
    egui::Color32::from_rgb(200, 150, 255),
];
const LABELS: [&str; 4] = [
    "Speed (tiles/s)",
    "Size (tile fraction)",
    "Diet (-1 meat, +1 plants)",
    "Vision radius (tiles)",
];

pub struct GraphPlugin;
impl Plugin for GraphPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GraphState>()
            .add_systems(EguiPrimaryContextPass, graph);
    }
}
#[derive(Resource)]
struct GraphState {
    drag_start: Option<Vec2>,
    draft: Option<Rect>,
    region: Option<Rect>,
    history: VecDeque<Sample>,
    started: f64,
    last_sample: Option<f64>,
}
impl Default for GraphState {
    fn default() -> Self {
        Self {
            drag_start: None,
            draft: None,
            region: None,
            history: VecDeque::new(),
            started: 0.0,
            last_sample: None,
        }
    }
}
struct Sample {
    time: f64,
    count: usize,
    mean: Option<[f32; 4]>,
    bounds: Option<([f32; 4], [f32; 4])>,
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
    let (mut count, mut total) = (0, [0.0; 4]);
    let (mut minimum, mut maximum) = ([f32::INFINITY; 4], [f32::NEG_INFINITY; 4]);
    for (pos, genes) in &creatures {
        let center = pos.0 + Vec2::new(0.0, TILE_SZ * 0.25);
        if !state
            .region
            .is_some_and(|r| center.cmpge(r.min).all() && center.cmplt(r.max).all())
        {
            continue;
        }
        count += 1;
        let values = [
            genes.speed,
            genes.size,
            genes.diet.into_inner() as f32,
            genes.vision_radius as f32,
        ];
        for i in 0..4 {
            total[i] += values[i];
            minimum[i] = minimum[i].min(values[i]);
            maximum[i] = maximum[i].max(values[i]);
        }
        if let Some(p) = screen(center) {
            painter.circle_stroke(p, 9.0, egui::Stroke::new(1.5, COLORS[0]));
        }
    }
    let mean = (count > 0).then(|| total.map(|v| v / count as f32));
    let bounds = (count > 0).then_some((minimum, maximum));
    if state.region.is_some() && state.last_sample.is_none_or(|last| now - last >= INTERVAL) {
        let sample = Sample {
            time: now - state.started,
            count,
            mean,
            bounds,
        };
        state.history.push_back(sample);
        if state.history.len() > MAX_SAMPLES {
            state.history.pop_front();
        }
        state.last_sample = Some(now);
    }
    egui::Window::new("Regional gene graph")
        .default_width(560.0)
        .vscroll(true)
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
            egui::Grid::new("gene_values").striped(true).show(ui, |ui| {
                for heading in ["Gene", "Highest inside now", "Latest sampled mean"] {
                    ui.label(heading);
                }
                ui.end_row();
                for i in 0..4 {
                    ui.colored_label(COLORS[i], LABELS[i]);
                    ui.label(bounds.map_or_else(|| "-".into(), |(_, max)| format!("{:.3}", max[i])));
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
            ui.label("Y: raw regional mean | X: simulation seconds since selection");
            ui.small("Sampled every 0.5s; last 15 minutes retained. Diet is fixed at -1 to +1; other axis tops use current individual maxima.");
            ui.small("Past values above the current maximum are clipped. Empty regions keep the last nonempty scale.");
            if count == 0 {
                ui.label("Area empty: recording gaps; new arrivals are included automatically.");
            }
            let scale = bounds.or_else(|| state.history.iter().rev().find_map(|s| s.bounds));
            for gene in 0..4 {
                ui.colored_label(COLORS[gene], LABELS[gene]);
                if gene == 2 {
                    plot(ui, &state, gene, -1.0, 1.0);
                } else if let Some((min, max)) = scale {
                    plot(ui, &state, gene, min[gene], max[gene]);
                } else {
                    ui.label("Waiting for creatures in this region.");
                }
            }
        });
}

fn plot(ui: &mut egui::Ui, state: &GraphState, gene: usize, minimum: f32, high: f32) {
    let (response, painter) = ui.allocate_painter(
        egui::vec2(ui.available_width().max(200.0), 180.0),
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
    let low = minimum.min(0.0);
    // Keep the top exactly at the regional maximum, including all-zero regions.
    let low = if high > low { low } else { high - 1.0 };
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
            format!("{y:.3}"),
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
    {
        let painter = painter.with_clip_rect(chart);
        let mut previous = None;
        for sample in &state.history {
            let point = sample
                .mean
                .map(|m| m[gene])
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
