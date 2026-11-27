use crate::curve::{self, CurvePoint, TransferLaw, BULB_GAMMA};
use crate::show::Show;
use egui::{pos2, vec2, Color32, Context, Pos2, Rect, Sense, Ui};

use super::ncod_view::NcodViewState;
use super::playback;
use super::theme;

#[derive(Clone, Copy)]
struct CurveDrag(usize);

pub fn ui(ctx: &Context, show: &Show, state: &mut NcodViewState) {
    let Some(channel) = state.curve_editor else {
        return;
    };
    if !(1..=512).contains(&channel) {
        state.curve_editor = None;
        return;
    }
    let mut open = true;
    egui::Window::new(format!("Transmission · DMX {channel}"))
        .id(egui::Id::new("transmission-curve"))
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .default_width(280.0)
        .show(ctx, |ui| editor(ui, show, state, channel));
    if !open {
        state.curve_editor = None;
    }
}

fn editor(ui: &mut Ui, show: &Show, state: &mut NcodViewState, channel: u16) {
    let law = state.curves.law(channel);
    ui.label("Programmed level → DMX sent to the lamp.");
    let selected = state.selected_channels();
    if selected.len() > 1 && selected.contains(&channel) {
        ui.label(theme::label_muted(format!(
            "Applies to the {} selected tracks",
            selected.len()
        )));
    }
    ui.horizontal_wrapped(|ui| {
        if ui.selectable_label(law.is_linear(), "Linear").clicked() {
            state.set_transfer(channel, TransferLaw::Linear);
        }
        if preset(ui, &law, 2.0, "Square", "Power 2. Stays dark, then rises.") {
            state.set_transfer(channel, TransferLaw::Gamma { gamma: 2.0 });
        }
        if preset(
            ui,
            &law,
            0.5,
            "Inverse",
            "Power 0.5. Rises fast, then flattens.",
        ) {
            state.set_transfer(channel, TransferLaw::Gamma { gamma: 0.5 });
        }
        if preset(
            ui,
            &law,
            BULB_GAMMA,
            "Bulb",
            "Power 0.30. A tungsten lamp stays dark at low voltage. This sends more DMX early so a linear fade looks linear on the lamp.",
        ) {
            state.set_transfer(channel, TransferLaw::Gamma { gamma: BULB_GAMMA });
        }
        if ui
            .selectable_label(matches!(law, TransferLaw::SCurve), "S")
            .on_hover_text("Slow at the bottom and the top.")
            .clicked()
        {
            state.set_transfer(channel, TransferLaw::SCurve);
        }
    });
    let law = state.curves.law(channel);
    if let Some(mut gamma) = law.gamma_value() {
        if ui
            .add(
                egui::Slider::new(&mut gamma, 0.2..=5.0)
                    .logarithmic(true)
                    .fixed_decimals(2)
                    .text("power"),
            )
            .changed()
        {
            state.set_transfer(channel, TransferLaw::Gamma { gamma });
        }
        ui.label(theme::label_muted(
            "1 linear · below 1 rises early · above 1 stays dark then rises",
        ));
    }
    let law = state.curves.law(channel);
    if !matches!(law, TransferLaw::Points { .. }) {
        if ui.button("Edit points").clicked() {
            state.set_transfer(
                channel,
                TransferLaw::Points {
                    points: curve::seed_points(&law),
                },
            );
        }
    } else {
        ui.label(theme::label_muted(
            "Drag a point. Double-click a middle point to remove it.",
        ));
    }
    let mut min_on = state.curves.min_on(channel);
    if ui
        .add(egui::Slider::new(&mut min_on, 0..=255).text("minimum on"))
        .on_hover_text("DMX level where this lamp strikes. Programmed 0 stays off. The first level above 0 is sent here, then the curve runs up to 255.")
        .changed()
    {
        state.set_min_on(channel, min_on);
    }
    let law = state.curves.law(channel);
    let min_on = state.curves.min_on(channel);
    let programmed = show.value_at(usize::from(channel - 1), state.play_ms);
    let gained = playback::scale_level(programmed, state.levels.get(channel));
    let sent = state.curves.lookup(channel, gained);
    ui.label(theme::label_muted("At the playhead, after the track fader"));
    ui.monospace(format!("{programmed}  →  {sent}"));
    let lane = state
        .pinned
        .iter()
        .position(|pinned| *pinned == channel)
        .unwrap_or(0);
    if let Some(next) = plot(ui, &law, min_on, theme::track_color(lane), gained, sent) {
        state.set_transfer(channel, next);
    }
}

fn preset(ui: &mut Ui, law: &TransferLaw, gamma: f32, name: &str, tip: &str) -> bool {
    let active = law
        .gamma_value()
        .is_some_and(|value| (value - gamma).abs() < 0.011)
        && !law.is_linear();
    ui.selectable_label(active, name)
        .on_hover_text(tip)
        .clicked()
}

fn plot(
    ui: &mut Ui,
    law: &TransferLaw,
    min_on: u8,
    color: Color32,
    marker_in: u8,
    marker_out: u8,
) -> Option<TransferLaw> {
    let (rect, response) = ui.allocate_exact_size(vec2(260.0, 160.0), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_rgb(16, 16, 18));
    let inner = rect.shrink2(vec2(10.0, 10.0));
    painter.line_segment(
        [plot_pos(inner, 0, 0), plot_pos(inner, 255, 255)],
        egui::Stroke::new(1.0_f32, Color32::from_white_alpha(28)),
    );
    let lut = curve::bake_min_on(&law.lut(), min_on);
    let mut cursor = plot_pos(inner, 0, lut[0]);
    for input in 1..=255 {
        let next = plot_pos(inner, input as u8, lut[input]);
        painter.line_segment([cursor, next], egui::Stroke::new(1.5_f32, color));
        cursor = next;
    }
    let shown = match law {
        TransferLaw::Points { points } => points.clone(),
        _ => Vec::new(),
    };
    let display: Vec<CurvePoint> = shown
        .iter()
        .map(|point| CurvePoint {
            input: point.input,
            output: curve::apply_min_on(point.input, point.output, min_on),
        })
        .collect();
    for (index, point) in display.iter().enumerate() {
        let end = index == 0 || index + 1 == shown.len();
        painter.circle_filled(
            plot_pos(inner, point.input, point.output),
            4.0,
            if end { Color32::from_gray(230) } else { color },
        );
    }
    painter.circle_stroke(
        plot_pos(inner, marker_in, marker_out),
        4.5,
        egui::Stroke::new(1.5_f32, Color32::WHITE),
    );

    let id = response.id;
    if response.double_clicked() {
        if let TransferLaw::Points { mut points } = law.clone() {
            if let Some(pos) = pointer(&response) {
                if let Some(index) = nearest(inner, &display, pos, 10.0) {
                    if curve::remove_point(&mut points, index) {
                        ui.data_mut(|data| data.remove::<CurveDrag>(id));
                        return Some(TransferLaw::Points { points });
                    }
                }
            }
        }
        return None;
    }
    if !(response.dragged() || response.drag_started()) {
        if response.drag_stopped() {
            ui.data_mut(|data| data.remove::<CurveDrag>(id));
        }
        return None;
    }
    let Some(pos) = pointer(&response) else {
        return None;
    };
    let (x, y) = plot_value(inner, pos);
    let y = curve::unbake_min_on(y, min_on);
    let mut points = match law {
        TransferLaw::Points { points } => points.clone(),
        other => curve::seed_points(other),
    };
    let index = if let Some(CurveDrag(index)) = ui.data(|data| data.get_temp::<CurveDrag>(id)) {
        index
    } else {
        let index = nearest(inner, &display, pos, 12.0).unwrap_or_else(|| {
            let mut owned = points.clone();
            let index = curve::insert_point(&mut owned, x, y);
            points = owned;
            index
        });
        ui.data_mut(|data| data.insert_temp(id, CurveDrag(index)));
        index
    };
    if index >= points.len() {
        return None;
    }
    curve::move_point(&mut points, index, x, y);
    Some(TransferLaw::Points { points })
}

fn pointer(response: &egui::Response) -> Option<Pos2> {
    response.interact_pointer_pos().or(response.hover_pos())
}

fn plot_pos(rect: Rect, input: u8, output: u8) -> Pos2 {
    pos2(
        rect.left() + rect.width() * (f32::from(input) / 255.0),
        rect.bottom() - rect.height() * (f32::from(output) / 255.0),
    )
}

fn plot_value(rect: Rect, pos: Pos2) -> (u8, u8) {
    let x = ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
    let y = ((rect.bottom() - pos.y) / rect.height().max(1.0)).clamp(0.0, 1.0);
    ((x * 255.0).round() as u8, (y * 255.0).round() as u8)
}

fn nearest(rect: Rect, points: &[CurvePoint], pos: Pos2, radius: f32) -> Option<usize> {
    points
        .iter()
        .enumerate()
        .filter_map(|(index, point)| {
            let delta = plot_pos(rect, point.input, point.output) - pos;
            let distance = delta.length();
            (distance <= radius).then_some((index, distance))
        })
        .min_by(|(_, left), (_, right)| left.total_cmp(right))
        .map(|(index, _)| index)
}
