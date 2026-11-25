use crate::show::Show;
use egui::{Color32, RichText, Ui};

use super::ncod_view::NcodViewState;
use super::theme;
use super::timecode;
use super::undo::{UndoOp, UndoStack};

pub fn bar(ui: &mut Ui, show: &mut Show, state: &mut NcodViewState, undo: &mut UndoStack) -> bool {
    let mut dirty = false;
    let end_ms = f64::from(show.end_ms().max(1));
    let play_ms = show
        .frames
        .get(state.playhead)
        .map(|f| f.at_ms)
        .unwrap_or(0);

    ui.horizontal(|ui| {
        ui.visuals_mut().override_text_color = Some(Color32::from_gray(210));
        if transport_button(ui, "|◀", "Beginning").clicked() {
            state.place_playhead(0);
            state.selected = vec![0];
        }
        if transport_button(ui, "◀", "Previous frame").clicked() {
            state.place_playhead(state.playhead.saturating_sub(1));
            state.selected = vec![state.playhead];
        }
        let play_btn = if state.playing {
            transport_button(ui, "■", "Stop")
        } else {
            transport_button_colored(ui, "▶", "Play", theme::TRANSPORT_PLAY)
        };
        if play_btn.clicked() {
            if state.playing {
                state.stop_playback();
            } else {
                toggle_play(show, state);
            }
        }
        if transport_button(ui, "▶|", "Next frame").clicked() {
            let last = show.frames.len().saturating_sub(1);
            state.place_playhead((state.playhead + 1).min(last));
            state.selected = vec![state.playhead];
        }
        if transport_button(ui, "▶▶|", "End").clicked() {
            let last = show.frames.len().saturating_sub(1);
            state.place_playhead(last);
            state.selected = vec![state.playhead];
        }

        ui.separator();
        ui.checkbox(&mut state.playback.r#loop, "Loop");
        ui.checkbox(&mut state.follow_playhead, "Follow");

        ui.separator();
        ui.label(theme::label_timecode(timecode::format_ms(play_ms)));
        ui.label(
            RichText::new(format!(
                "frame {} / {}",
                state.playhead,
                show.frames.len().saturating_sub(1)
            ))
            .monospace()
            .color(Color32::from_gray(160)),
        );

        let mut tc = timecode::format_ms(play_ms);
        ui.add(
            egui::TextEdit::singleline(&mut tc)
                .desired_width(120.0)
                .font(egui::TextStyle::Monospace),
        );
        if ui.button("Seek").clicked() {
            if let Some(ms) = timecode::parse_timecode(&tc) {
                state.place_playhead(nearest_frame(show, ms));
                state.selected = vec![state.playhead];
            }
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Fit").clicked() {
                state.view_start_ms = 0.0;
                state.view_end_ms = end_ms;
            }
            if ui.button("−").clicked() {
                zoom(state, end_ms, 1.25);
            }
            if ui.button("+").clicked() {
                zoom(state, end_ms, 0.8);
            }
            ui.label(
                RichText::new("Zoom")
                    .small()
                    .color(Color32::from_gray(130)),
            );
        });
    });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Art-Net").small());
        ui.add(
            egui::TextEdit::singleline(&mut state.playback.target)
                .desired_width(140.0)
                .hint_text("255.255.255.255"),
        );
        ui.label("Universe");
        let mut uni = state.playback.universe;
        if ui.add(egui::DragValue::new(&mut uni).range(0..=32767)).changed() {
            state.playback.universe = uni;
        }
        ui.label(format!(
            "  {} frames · {:.1} min",
            show.frames.len(),
            end_ms / 60_000.0
        ));
        if !show.timestamps_monotonic() {
            ui.label(RichText::new("Non-monotonic times").color(Color32::RED));
            if ui.button("Sort").clicked() {
                let before = show.frames.clone();
                show.sort_by_time();
                let after = show.frames.clone();
                undo.push(vec![UndoOp::ReplaceFrames { before, after }]);
                dirty = true;
            }
        }
    });

    dirty
}

fn transport_button(ui: &mut Ui, label: &'static str, _hint: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).monospace().size(15.0))
            .min_size(egui::vec2(36.0, 28.0)),
    )
}

fn transport_button_colored(
    ui: &mut Ui,
    label: &'static str,
    _hint: &str,
    color: Color32,
) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).monospace().size(15.0).color(color))
            .min_size(egui::vec2(36.0, 28.0)),
    )
}

fn zoom(state: &mut NcodViewState, end_ms: f64, factor: f64) {
    let span = (state.view_end_ms - state.view_start_ms).clamp(20.0, end_ms);
    let new_span = (span * factor).clamp(20.0, end_ms);
    let center = (state.view_start_ms + state.view_end_ms) * 0.5;
    let mut start = center - new_span * 0.5;
    if start < 0.0 {
        start = 0.0;
    }
    if start + new_span > end_ms {
        start = (end_ms - new_span).max(0.0);
    }
    state.view_start_ms = start;
    state.view_end_ms = start + new_span;
}

pub fn toggle_play(show: &Show, state: &mut NcodViewState) {
    state.stop_playback();
    let frames = std::sync::Arc::new(show.frames.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    match super::playback::start(frames, state.playhead, state.playback.clone(), tx) {
        Ok(handle) => {
            state.playing = true;
            state.play_gen = 0;
            state.playback_handle = Some(handle);
            state.playback_rx = Some(rx);
        }
        Err(err) => eprintln!("playback: {err}"),
    }
}

fn nearest_frame(show: &Show, at_ms: u32) -> usize {
    show.frames
        .iter()
        .enumerate()
        .min_by_key(|(_, frame)| frame.at_ms.abs_diff(at_ms))
        .map(|(index, _)| index)
        .unwrap_or(0)
}
