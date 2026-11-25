use crate::ncod::Show;
use egui::{pos2, vec2, Color32, Painter, PointerButton, Rect, Response, Sense, Ui};

use super::ncod_view::{Gesture, NcodViewState, TimelineTool};
use super::theme;
use super::timecode;
use super::undo::UndoStack;

pub const HEADER_W: f32 = 88.0;
pub const RULER_H: f32 = 26.0;
pub const LANE_H: f32 = 84.0;
pub const MINIMAP_H: f32 = 36.0;

pub fn snap_ms(time_ms: u32, grid_ms: u32) -> u32 {
    if grid_ms == 0 {
        return time_ms;
    }
    let rounded = ((time_ms as f64 / grid_ms as f64).round() * grid_ms as f64) as u32;
    rounded
}

pub fn ui(ui: &mut Ui, show: &mut Show, state: &mut NcodViewState, undo: &mut UndoStack) -> bool {
    let mut dirty = false;
    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.tool, TimelineTool::Select, "V  Select");
        ui.selectable_value(&mut state.tool, TimelineTool::Edit, "K  Keyframe");
        ui.selectable_value(&mut state.tool, TimelineTool::Brush, "B  Brush");
        ui.separator();
        if ui.button("Fade range").clicked() {
            dirty |= fade_range(show, state, undo);
        }
        if ui.button("Clear range").clicked() {
            state.range_ms = None;
        }
        ui.separator();
        ui.label("Snap");
        ui.selectable_value(&mut state.snap_ms, 0, "Off");
        ui.selectable_value(&mut state.snap_ms, 50, "50ms");
        ui.selectable_value(&mut state.snap_ms, 100, "100ms");
        ui.selectable_value(&mut state.snap_ms, 500, "500ms");
        ui.selectable_value(&mut state.snap_ms, 1_000, "1s");
        ui.checkbox(&mut state.show_keyframes, "Keyframes");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(theme::label_muted("Scroll zooms at cursor"));
        });
    });
    dirty |= minimap(ui, show, state);
    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            dirty |= timeline_body(ui, show, state, undo);
        });
    dirty
}

fn minimap(ui: &mut Ui, show: &Show, state: &mut NcodViewState) -> bool {
    let mut changed = false;
    let end_ms = f64::from(show.end_ms().max(1));
    let w = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(w, MINIMAP_H), Sense::click_and_drag());
    let painter = ui.painter_at(rect).with_clip_rect(rect);
    painter.rect_filled(rect, 4.0, Color32::from_rgb(18, 18, 20));

    let track = state.selected_track.min(state.pinned.len().saturating_sub(1));
    let channel = state.pinned.get(track).copied().unwrap_or(1);
    if channel > 0 && channel <= 512 {
        let slot = usize::from(channel - 1);
        let cols = (rect.width().max(1.0) as usize).min(800);
        let mut points = Vec::with_capacity(cols);
        for col in 0..cols {
            let t = ((col as f64 + 0.5) / cols as f64) * end_ms;
            let value = held_value(show, slot, t as u32);
            let x = rect.left() + (col as f32 / cols as f32) * rect.width();
            let y = rect.bottom() - 3.0 - (value as f32 / 255.0) * (rect.height() - 8.0);
            points.push(pos2(x, y));
        }
        if points.len() >= 2 {
            painter.add(egui::Shape::line(
                points,
                egui::Stroke::new(1.2_f32, theme::track_color(track)),
            ));
        }
        painter.text(
            pos2(rect.left() + 6.0, rect.top() + 8.0),
            egui::Align2::LEFT_CENTER,
            format!("DMX {channel}  ·  full show"),
            egui::FontId::monospace(10.0),
            Color32::from_white_alpha(140),
        );
    }

    let span = (state.view_end_ms - state.view_start_ms).max(1.0);
    if span < end_ms * 0.98 {
        let v0 = time_to_x(rect, 0.0, end_ms, state.view_start_ms);
        let v1 = time_to_x(rect, 0.0, end_ms, state.view_end_ms);
        let viewport = Rect::from_min_max(
            pos2(v0, rect.top() + 2.0),
            pos2(v1.max(v0 + 4.0), rect.bottom() - 2.0),
        );
        painter.rect_stroke(viewport, 2.0, egui::Stroke::new(1.5_f32, theme::ACCENT));
    }

    let play_ms = show.frames.get(state.playhead).map(|f| f.at_ms).unwrap_or(0);
    let px = time_to_x(rect, 0.0, end_ms, f64::from(play_ms));
    painter.line_segment(
        [pos2(px, rect.top()), pos2(px, rect.bottom())],
        egui::Stroke::new(1.5_f32, theme::PLAYHEAD),
    );

    if let Some(pos) = response.interact_pointer_pos() {
        if response.clicked() || response.dragged() {
            let ms = x_to_time(rect, 0.0, end_ms, pos.x);
            let span = state.view_end_ms - state.view_start_ms;
            state.view_start_ms = (ms - span * 0.5).clamp(0.0, end_ms);
            state.view_end_ms = (state.view_start_ms + span).clamp(span, end_ms);
            changed = true;
        }
    }
    changed
}

fn timeline_body(
    ui: &mut Ui,
    show: &mut Show,
    state: &mut NcodViewState,
    undo: &mut UndoStack,
) -> bool {
    let mut dirty = false;
    let end_ms = f64::from(show.end_ms().max(1));
    let lanes = state.pinned.len().max(1);
    let height = RULER_H + LANE_H * lanes as f32;
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click_and_drag());
    let lane_h = LANE_H;
    let painter = ui.painter_at(rect);

    let header = Rect::from_min_max(rect.min, pos2(rect.left() + HEADER_W, rect.bottom()));
    let lanes_rect = Rect::from_min_max(pos2(rect.left() + HEADER_W, rect.top()), rect.max);
    let ruler = Rect::from_min_max(lanes_rect.min, pos2(lanes_rect.right(), lanes_rect.top() + RULER_H));
    let body_top = ruler.bottom();
    let body = Rect::from_min_max(pos2(lanes_rect.left(), body_top), lanes_rect.max);

    painter.rect_filled(header, 0.0, theme::BG_PANEL);
    painter.rect_filled(ruler, 0.0, theme::RULER_BG);
    painter.rect_filled(body, 0.0, theme::BG_DEEP);

    draw_ruler(&painter, ruler, state.view_start_ms, state.view_end_ms, state.snap_ms);
    draw_grid(&painter, body, state.view_start_ms, state.view_end_ms, state.snap_ms);
    if let Some((start, end)) = state.range_ms {
        let x0 = time_to_x(body, state.view_start_ms, state.view_end_ms, f64::from(start));
        let x1 = time_to_x(body, state.view_start_ms, state.view_end_ms, f64::from(end));
        let band = Rect::from_min_max(pos2(x0.min(x1), body.top()), pos2(x0.max(x1), body.bottom()));
        painter.rect_filled(band, 0.0, Color32::from_rgba_unmultiplied(100, 160, 220, 28));
    }

    if handle_zoom_pan(&response, ui, state, end_ms, lanes_rect) {
        dirty = true;
    }

    for (lane, &channel) in state.pinned.iter().enumerate() {
        let y0 = body_top + lane as f32 * lane_h;
        let lane_rect = Rect::from_min_size(pos2(body.left(), y0), vec2(body.width(), lane_h));
        let header_lane = Rect::from_min_size(pos2(header.left(), y0), vec2(HEADER_W, lane_h));
        let bg = if lane % 2 == 0 {
            theme::BG_LANE
        } else {
            theme::BG_LANE_ALT
        };
        painter.rect_filled(lane_rect, 0.0, bg);
        painter.rect_filled(header_lane, 0.0, bg);
        draw_level_scale(&painter, lane_rect);
        if state.selected_track == lane {
            painter.rect_stroke(
                header_lane,
                0.0,
                egui::Stroke::new(1.5_f32, theme::ACCENT),
            );
        }
        let color = theme::track_color(lane);
        let now = show
            .frames
            .get(state.playhead)
            .map(|frame| frame.slots[usize::from(channel.saturating_sub(1))])
            .unwrap_or(0);
        painter.text(
            pos2(header_lane.left() + 10.0, header_lane.center().y - 7.0),
            egui::Align2::LEFT_CENTER,
            format!("DMX {channel}"),
            egui::FontId::monospace(12.0),
            if state.selected_track == lane {
                Color32::WHITE
            } else {
                Color32::from_gray(185)
            },
        );
        painter.text(
            pos2(header_lane.left() + 10.0, header_lane.center().y + 8.0),
            egui::Align2::LEFT_CENTER,
            format!("{now}"),
            egui::FontId::monospace(11.0),
            color,
        );
        if channel > 0 && channel <= 512 {
            let lane_painter = painter.with_clip_rect(lane_rect);
            draw_lane_envelope(
                &lane_painter,
                lane_rect,
                show,
                usize::from(channel - 1),
                state.view_start_ms,
                state.view_end_ms,
                color,
            );
            if state.show_keyframes
                && (lane == state.selected_track
                    || state.tool == TimelineTool::Edit
                    || state.tool == TimelineTool::Brush)
            {
                draw_keyframes(
                    &painter,
                    lane_rect,
                    show,
                    lane,
                    usize::from(channel - 1),
                    state,
                    state.view_start_ms,
                    state.view_end_ms,
                    color,
                );
            }
        }
    }

    dirty |= handle_pointer(
        show, state, undo, &response, header, body, body_top, lane_h, end_ms,
    );

    let play_ms = show.frames.get(state.playhead).map(|f| f.at_ms).unwrap_or(0);
    let play_x = time_to_x(body, state.view_start_ms, state.view_end_ms, f64::from(play_ms));
    painter.line_segment(
        [pos2(play_x, ruler.top()), pos2(play_x, body.bottom())],
        egui::Stroke::new(2.0_f32, theme::PLAYHEAD),
    );
    painter.circle_filled(pos2(play_x, ruler.top() + 6.0), 5.0, theme::PLAYHEAD);

    let holding = response.dragged() || response.is_pointer_button_down_on();
    if state.follow_playhead && state.playing && !holding {
        center_window_on(state, f64::from(play_ms), end_ms);
    }

    dirty
}

fn handle_pointer(
    show: &mut Show,
    state: &mut NcodViewState,
    undo: &mut UndoStack,
    response: &Response,
    header: Rect,
    body: Rect,
    body_top: f32,
    lane_h: f32,
    end_ms: f64,
) -> bool {
    let mut dirty = false;
    let Some(pos) = response.interact_pointer_pos() else {
        if response.drag_stopped() {
            state.finish_gesture(undo, show);
        }
        return dirty;
    };

    if response.clicked() && header.contains(pos) && pos.y >= body_top {
        let lane = ((pos.y - body_top) / lane_h).floor() as usize;
        if lane < state.pinned.len() {
            state.selected_track = lane;
        }
    }

    let shift = response.ctx.input(|input| input.modifiers.shift);
    if state.tool == TimelineTool::Select
        && pos.x >= body.left()
        && (response.clicked() || response.dragged())
        && !shift
    {
        let ms = x_to_time(body, state.view_start_ms, state.view_end_ms, pos.x) as u32;
        state.place_playhead(nearest_frame(show, ms));
        state.selected = vec![state.playhead];
    }

    if body.contains(pos) {
        dirty |= edit_body(show, state, &response, body, body_top, lane_h, pos);
    }

    if response.drag_stopped() {
        state.finish_gesture(undo, show);
    }

    let _ = end_ms;
    dirty
}

fn edit_body(
    show: &mut Show,
    state: &mut NcodViewState,
    response: &Response,
    body: Rect,
    body_top: f32,
    lane_h: f32,
    pos: egui::Pos2,
) -> bool {
    let mut dirty = false;
    let lane = ((pos.y - body_top) / lane_h).floor() as usize;
    if lane >= state.pinned.len() {
        return false;
    }
    let channel = state.pinned[lane];
    if channel == 0 || channel > 512 {
        return false;
    }
    let slot = usize::from(channel - 1);
    let lane_rect = Rect::from_min_size(
        pos2(body.left(), body_top + lane as f32 * lane_h),
        vec2(body.width(), lane_h),
    );
    let pointer_ms = snap_ms(
        x_to_time(body, state.view_start_ms, state.view_end_ms, pos.x) as u32,
        state.snap_ms,
    );
    let value = lane_value(lane_rect, pos.y);

    match state.tool {
        TimelineTool::Select => {
            let shift = ui_shift(response);
            let seek_ms = x_to_time(body, state.view_start_ms, state.view_end_ms, pos.x) as u32;
            if shift && (response.drag_started() || response.dragged()) {
                if response.drag_started() {
                    state.gesture = Some(Gesture::Range { start_ms: seek_ms });
                }
                if let Some(Gesture::Range { start_ms }) = state.gesture {
                    state.range_ms = Some((start_ms.min(seek_ms), start_ms.max(seek_ms)));
                }
            } else if response.clicked() || response.dragged() {
                state.place_playhead(nearest_frame(show, seek_ms));
                state.selected = vec![state.playhead];
                state.selected_track = lane;
            }
        }
        TimelineTool::Brush => {
            if response.drag_started() || response.clicked() {
                state.selected_track = lane;
                state.gesture = Some(Gesture::Paint {
                    slot,
                    originals: Vec::new(),
                });
            }
            if response.dragged() || response.clicked() {
                state.selected_track = lane;
                let index = nearest_frame(show, pointer_ms);
                paint_frame(show, state, index, slot, value);
                state.place_playhead(index);
                dirty = true;
            }
        }
        TimelineTool::Edit => {
            state.selected_track = lane;
            if response.drag_started() || response.clicked() {
                if let Some((fi, _)) = hit_keyframe(show, slot, lane_rect, state, pos)
                {
                    let frame = &show.frames[fi];
                    state.place_playhead(fi);
                    state.selected = vec![fi];
                    state.gesture = Some(Gesture::Keyframe {
                        frame: fi,
                        slot,
                        before_value: frame.slots[slot],
                        before_ms: frame.at_ms,
                        origin: pos,
                        move_time: None,
                    });
                }
            }
            if response.dragged() {
                let edit = match &state.gesture {
                    Some(Gesture::Keyframe {
                        frame,
                        slot,
                        origin,
                        move_time,
                        ..
                    }) => Some((*frame, *slot, *origin, *move_time)),
                    _ => None,
                };
                if let Some((frame, slot, origin, move_time)) = edit {
                    let move_time = if let Some(axis) = move_time {
                        axis
                    } else {
                        let delta = pos - origin;
                        let time = delta.x.abs() > delta.y.abs() && delta.x.abs() > 6.0;
                        if delta.x.abs() > 6.0 || delta.y.abs() > 6.0 {
                            if let Some(Gesture::Keyframe { move_time, .. }) = &mut state.gesture {
                                *move_time = Some(time);
                            }
                        }
                        time
                    };
                    if move_time {
                        let at = clamp_time(show, frame, pointer_ms);
                        if let Some(current) = show.frames.get_mut(frame) {
                            current.at_ms = at;
                            dirty = true;
                        }
                    } else if let Some(current) = show.frames.get_mut(frame) {
                        current.slots[slot] = value;
                        dirty = true;
                    }
                }
            }
        }
    }
    dirty
}

fn paint_frame(show: &mut Show, state: &mut NcodViewState, index: usize, slot: usize, value: u8) {
    let Some(frame) = show.frames.get_mut(index) else {
        return;
    };
    if let Some(Gesture::Paint { originals, .. }) = &mut state.gesture {
        if originals.iter().all(|(frame_index, _)| *frame_index != index) {
            originals.push((index, frame.slots[slot]));
        }
    }
    frame.slots[slot] = value;
}

fn ui_shift(response: &Response) -> bool {
    response.ctx.input(|input| input.modifiers.shift)
}

fn draw_level_scale(painter: &Painter, lane: Rect) {
    for value in [0u8, 128, 255] {
        let y = lane.bottom() - (value as f32 / 255.0) * (lane.height() - 8.0) - 4.0;
        painter.line_segment(
            [pos2(lane.left(), y), pos2(lane.right(), y)],
            egui::Stroke::new(1.0_f32, Color32::from_white_alpha(18)),
        );
        painter.text(
            pos2(lane.left() + 4.0, y),
            egui::Align2::LEFT_CENTER,
            value.to_string(),
            egui::FontId::monospace(10.0),
            Color32::from_white_alpha(90),
        );
    }
}

fn lane_value(lane: Rect, y: f32) -> u8 {
    let span = (lane.height() - 8.0).max(1.0);
    let u = ((lane.bottom() - 4.0 - y) / span).clamp(0.0, 1.0);
    (u * 255.0).round() as u8
}

fn clamp_time(show: &Show, index: usize, at_ms: u32) -> u32 {
    let prev = show
        .frames
        .get(index.wrapping_sub(1))
        .filter(|_| index > 0)
        .map(|frame| frame.at_ms.saturating_add(1))
        .unwrap_or(0);
    let next = show
        .frames
        .get(index + 1)
        .map(|frame| frame.at_ms.saturating_sub(1))
        .unwrap_or(u32::MAX);
    at_ms.clamp(prev, next.max(prev))
}

fn fade_range(show: &mut Show, state: &mut NcodViewState, undo: &mut UndoStack) -> bool {
    let Some((start, end)) = state.range_ms else {
        return false;
    };
    let channel = state.pinned.get(state.selected_track).copied().unwrap_or(0);
    if channel == 0 || channel > 512 || end <= start {
        return false;
    }
    let slot = usize::from(channel - 1);
    let first = nearest_frame(show, start);
    let last = nearest_frame(show, end);
    if last <= first {
        return false;
    }
    let from = show.frames[first].slots[slot];
    let to = show.frames[last].slots[slot];
    let mut batch = Vec::new();
    let span = (last - first) as f32;
    for index in first..=last {
        let before = show.frames[index].slots[slot];
        let u = (index - first) as f32 / span;
        let after = (f32::from(from) + (f32::from(to) - f32::from(from)) * u).round() as u8;
        if before != after {
            show.frames[index].slots[slot] = after;
            batch.push(super::undo::UndoOp::Slot {
                frame: index,
                slot,
                before,
                after,
            });
        }
    }
    let changed = !batch.is_empty();
    undo.push(batch);
    changed
}

fn hit_keyframe(
    show: &Show,
    slot: usize,
    lane_rect: Rect,
    state: &NcodViewState,
    pos: egui::Pos2,
) -> Option<(usize, f32)> {
    let hit_r = 8.0;
    let mut best: Option<(usize, f32)> = None;
    for_each_keyframe(show, state.view_start_ms, state.view_end_ms, lane_rect.width(), |fi| {
        let frame = &show.frames[fi];
        let x = time_to_x(
            lane_rect,
            state.view_start_ms,
            state.view_end_ms,
            f64::from(frame.at_ms),
        );
        let y = y_of(lane_rect, frame.slots[slot]);
        let d = pos2(x, y).distance(pos);
        if d < hit_r && best.map(|(_, bd)| d < bd).unwrap_or(true) {
            best = Some((fi, d));
        }
    });
    best
}

fn keyframe_bucket_ms(start_ms: f64, end_ms: f64, width: f32) -> u32 {
    let target = f64::from((width / 6.0).max(8.0));
    ((end_ms - start_ms) / target).max(1.0) as u32
}

fn for_each_keyframe(
    show: &Show,
    start_ms: f64,
    end_ms: f64,
    width: f32,
    mut visit: impl FnMut(usize),
) {
    let (start_i, end_i) = frame_range(show, start_ms, end_ms);
    if start_i >= end_i {
        return;
    }
    let bucket = keyframe_bucket_ms(start_ms, end_ms, width).max(1);
    let mut last = None;
    for fi in start_i..end_i {
        let id = show.frames[fi].at_ms / bucket;
        if last == Some(id) {
            continue;
        }
        last = Some(id);
        visit(fi);
    }
}

fn draw_keyframes(
    painter: &Painter,
    lane_rect: Rect,
    show: &Show,
    lane: usize,
    slot: usize,
    state: &NcodViewState,
    start_ms: f64,
    end_ms: f64,
    color: Color32,
) {
    for_each_keyframe(show, start_ms, end_ms, lane_rect.width(), |fi| {
        let frame = &show.frames[fi];
        let x = time_to_x(lane_rect, start_ms, end_ms, f64::from(frame.at_ms));
        let y = y_of(lane_rect, frame.slots[slot]);
        let selected = !state.playing
            && lane == state.selected_track
            && state.selected.contains(&fi);
        let size = if selected { 5.5 } else { 3.5 };
        let diamond = diamond_shape(pos2(x, y), size);
        painter.add(egui::Shape::convex_polygon(
            diamond,
            if selected {
                theme::PLAYHEAD
            } else {
                color
            },
            egui::Stroke::new(1.0_f32, Color32::from_black_alpha(120)),
        ));
    });
}

fn diamond_shape(center: egui::Pos2, r: f32) -> Vec<egui::Pos2> {
    vec![
        pos2(center.x, center.y - r),
        pos2(center.x + r, center.y),
        pos2(center.x, center.y + r),
        pos2(center.x - r, center.y),
    ]
}

fn draw_lane_envelope(
    painter: &Painter,
    rect: Rect,
    show: &Show,
    slot: usize,
    start_ms: f64,
    end_ms: f64,
    color: Color32,
) {
    let columns = rect.width().max(1.0) as usize;
    let (start_i, end_i) = frame_range(show, start_ms, end_ms);
    if start_i >= end_i {
        return;
    }
    if end_i - start_i <= columns {
        let points = show.frames[start_i..end_i]
            .iter()
            .map(|frame| {
                pos2(
                    time_to_x(rect, start_ms, end_ms, f64::from(frame.at_ms)),
                    y_of(rect, frame.slots[slot]),
                )
            })
            .collect::<Vec<_>>();
        if points.len() >= 2 {
            painter.add(egui::Shape::line(points, egui::Stroke::new(1.4_f32, color)));
        }
        return;
    }
    let span = (end_ms - start_ms).max(1.0);
    let mut upper = Vec::with_capacity(columns);
    let mut lower = Vec::with_capacity(columns);
    for column in 0..columns {
        let t0 = start_ms + (column as f64 / columns as f64) * span;
        let t1 = start_ms + ((column + 1) as f64 / columns as f64) * span;
        let (lo, hi) = column_range(show, start_i, end_i, slot, t0 as u32, t1 as u32);
        let x = rect.left() + column as f32;
        upper.push(pos2(x, y_of(rect, hi)));
        lower.push(pos2(x, y_of(rect, lo)));
    }
    let band = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 70);
    for (hi, lo) in upper.iter().zip(&lower) {
        painter.line_segment([*hi, *lo], egui::Stroke::new(1.0_f32, band));
    }
    if upper.len() >= 2 {
        painter.add(egui::Shape::line(upper, egui::Stroke::new(1.4_f32, color)));
    }
}

fn y_of(rect: Rect, value: u8) -> f32 {
    rect.bottom() - 4.0 - (value as f32 / 255.0) * (rect.height() - 8.0)
}

fn column_range(show: &Show, start_i: usize, end_i: usize, slot: usize, t0: u32, t1: u32) -> (u8, u8) {
    let from = start_i
        + show.frames[start_i..end_i].partition_point(|frame| frame.at_ms < t0);
    let to = start_i + show.frames[start_i..end_i].partition_point(|frame| frame.at_ms < t1);
    let slice = &show.frames[from.min(end_i)..to.min(end_i)];
    if slice.is_empty() {
        let held = held_value(show, slot, t0);
        return (held, held);
    }
    let mut lo = 255u8;
    let mut hi = 0u8;
    for frame in slice {
        lo = lo.min(frame.slots[slot]);
        hi = hi.max(frame.slots[slot]);
    }
    (lo, hi)
}

fn held_value(show: &Show, slot: usize, t_ms: u32) -> u8 {
    let index = show.frames.partition_point(|f| f.at_ms <= t_ms).saturating_sub(1);
    show.frames.get(index).map(|f| f.slots[slot]).unwrap_or(0)
}

fn frame_range(show: &Show, start_ms: f64, end_ms: f64) -> (usize, usize) {
    let start = show
        .frames
        .partition_point(|f| f.at_ms < start_ms as u32);
    let end = show
        .frames
        .partition_point(|f| f.at_ms <= end_ms as u32);
    (start, end.min(show.frames.len()))
}

fn handle_zoom_pan(
    response: &Response,
    ui: &Ui,
    state: &mut NcodViewState,
    end_ms: f64,
    graph: Rect,
) -> bool {
    let mut changed = false;
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let over_graph = pointer.is_some_and(|pos| graph.contains(pos));
    if over_graph {
        let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
        let anchor_x = pointer.map(|pos| pos.x).unwrap_or_else(|| graph.center().x);
        if (zoom - 1.0).abs() > 0.000_1 {
            let span = view_span(state, end_ms);
            let new_span = (span / f64::from(zoom)).clamp(20.0, end_ms);
            place_span(state, end_ms, graph, anchor_x, span, new_span);
            changed = true;
        }
        if scroll.y != 0.0 {
            let span = view_span(state, end_ms);
            let new_span = (span * (-f64::from(scroll.y) / 200.0).exp()).clamp(20.0, end_ms);
            place_span(state, end_ms, graph, anchor_x, span, new_span);
            changed = true;
        }
        if scroll.x != 0.0 {
            pan_pixels(state, end_ms, graph.width(), scroll.x);
            changed = true;
        }
        if scroll != egui::Vec2::ZERO {
            ui.ctx().input_mut(|input| input.smooth_scroll_delta = egui::Vec2::ZERO);
        }
    } else if response.hovered() {
        let scroll_x = ui.input(|i| i.smooth_scroll_delta.x);
        if scroll_x != 0.0 {
            pan_pixels(state, end_ms, graph.width(), scroll_x);
            ui.ctx().input_mut(|input| input.smooth_scroll_delta.x = 0.0);
            changed = true;
        }
    }
    if response.dragged_by(PointerButton::Middle) || response.dragged_by(PointerButton::Secondary) {
        let delta = response.drag_delta().x;
        if delta != 0.0 {
            pan_pixels(state, end_ms, graph.width(), delta);
            changed = true;
        }
    }
    changed
}

fn view_span(state: &NcodViewState, end_ms: f64) -> f64 {
    (state.view_end_ms - state.view_start_ms).clamp(20.0, end_ms)
}

fn place_span(
    state: &mut NcodViewState,
    end_ms: f64,
    graph: Rect,
    anchor_x: f32,
    old_span: f64,
    new_span: f64,
) {
    let width = f64::from(graph.width().max(1.0));
    let u = (f64::from(anchor_x - graph.left()) / width).clamp(0.0, 1.0);
    let anchor_ms = state.view_start_ms + u * old_span;
    set_window(state, anchor_ms - u * new_span, new_span, end_ms);
}

fn pan_pixels(state: &mut NcodViewState, end_ms: f64, width: f32, pixels: f32) {
    let span = view_span(state, end_ms);
    let delta_ms = -f64::from(pixels) * span / f64::from(width.max(1.0));
    set_window(state, state.view_start_ms + delta_ms, span, end_ms);
}

fn set_window(state: &mut NcodViewState, start: f64, span: f64, end_ms: f64) {
    let span = span.clamp(20.0, end_ms);
    let mut start = start;
    if start < 0.0 {
        start = 0.0;
    }
    if start + span > end_ms {
        start = (end_ms - span).max(0.0);
    }
    state.view_start_ms = start;
    state.view_end_ms = start + span;
}

fn center_window_on(state: &mut NcodViewState, at_ms: f64, end_ms: f64) {
    let span = (state.view_end_ms - state.view_start_ms).clamp(1.0, end_ms);
    let mut start = at_ms - span * 0.5;
    if start < 0.0 {
        start = 0.0;
    }
    if start + span > end_ms {
        start = (end_ms - span).max(0.0);
    }
    state.view_start_ms = start;
    state.view_end_ms = start + span;
}

fn draw_ruler(painter: &Painter, rect: Rect, start_ms: f64, end_ms: f64, grid_ms: u32) {
    let step = nice_step(end_ms - start_ms, rect.width());
    let mut t = (start_ms / step).floor() * step;
    while t <= end_ms {
        if t >= start_ms {
            let x = time_to_x(rect, start_ms, end_ms, t);
            let is_snap =
                grid_ms > 0 && (t as u32).abs_diff(snap_ms(t as u32, grid_ms)) < grid_ms / 4;
            let col = if is_snap {
                theme::ACCENT
            } else {
                Color32::from_gray(120)
            };
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                egui::Stroke::new(1.0_f32, col),
            );
            if step >= 500.0 {
                painter.text(
                    pos2(x + 4.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    timecode::format_ms(t as u32),
                    egui::FontId::monospace(10.0),
                    Color32::from_gray(170),
                );
            }
        }
        t += step;
    }
}

fn draw_grid(painter: &Painter, rect: Rect, start_ms: f64, end_ms: f64, grid_ms: u32) {
    if grid_ms > 0 {
        let mut t = f64::from(snap_ms(start_ms as u32, grid_ms));
        while t <= end_ms {
            if t >= start_ms {
                let x = time_to_x(rect, start_ms, end_ms, t);
                painter.line_segment(
                    [pos2(x, rect.top()), pos2(x, rect.bottom())],
                    egui::Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(100, 160, 220, 35)),
                );
            }
            t += f64::from(grid_ms);
        }
    }
    let major = nice_step(end_ms - start_ms, rect.width());
    let minor = major / 5.0;
    let mut t = (start_ms / minor).floor() * minor;
    while t <= end_ms {
        if t >= start_ms {
            let x = time_to_x(rect, start_ms, end_ms, t);
            let idx = (t / minor).round();
            let major_line = (idx * minor - t).abs() < minor * 0.05;
            let color = if major_line {
                theme::GRID_MAJOR
            } else {
                theme::GRID_MINOR
            };
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                egui::Stroke::new(1.0_f32, color),
            );
        }
        t += minor;
    }
}

fn nice_step(span_ms: f64, width_px: f32) -> f64 {
    let target_cols = (width_px / 90.0).max(4.0) as f64;
    let raw = span_ms / target_cols;
    let magnitude = 10_f64.powf(raw.log10().floor());
    let norm = raw / magnitude;
    let nice = if norm < 1.5 {
        1.0
    } else if norm < 3.5 {
        2.0
    } else if norm < 7.5 {
        5.0
    } else {
        10.0
    };
    nice * magnitude
}

fn time_to_x(rect: Rect, start_ms: f64, end_ms: f64, t_ms: f64) -> f32 {
    let span = (end_ms - start_ms).max(1.0);
    let u = ((t_ms - start_ms) / span).clamp(0.0, 1.0);
    rect.left() + (u * f64::from(rect.width())) as f32
}

fn x_to_time(rect: Rect, start_ms: f64, end_ms: f64, x: f32) -> f64 {
    let span = end_ms - start_ms;
    let u = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    start_ms + f64::from(u) * span
}

fn nearest_frame(show: &Show, at_ms: u32) -> usize {
    show.frames
        .iter()
        .enumerate()
        .min_by_key(|(_, frame)| frame.at_ms.abs_diff(at_ms))
        .map(|(index, _)| index)
        .unwrap_or(0)
}