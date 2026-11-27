use crate::show::Show;
use egui::{pos2, vec2, Color32, Painter, PointerButton, Rect, Response, Sense, Ui};
use std::collections::{HashMap, HashSet};

use super::curve_editor;
use super::fader;
use super::ncod_view::{Gesture, MarqueeMode, NcodViewState};
use super::theme;
use super::timecode;
use super::undo::UndoStack;

pub const HEADER_W: f32 = 196.0;
pub const RULER_H: f32 = 26.0;
pub const LANE_H: f32 = 84.0;
pub const MIN_LANE_H: f32 = 48.0;
pub const MAX_LANE_H: f32 = 480.0;
const SEAM_HIT: f32 = 5.0;
pub const MINIMAP_H: f32 = 36.0;
const PLOT_RIGHT_PAD: f32 = 10.0;

pub fn snap_ms(time_ms: u32, grid_ms: u32) -> u32 {
    if grid_ms == 0 {
        return time_ms;
    }
    let rounded = ((time_ms as f64 / grid_ms as f64).round() * grid_ms as f64) as u32;
    rounded
}

fn visible_width(ui: &Ui) -> f32 {
    let visible = (ui.clip_rect().right() - ui.cursor().min.x).max(0.0);
    ui.available_width().min(visible)
}

pub fn ui(ui: &mut Ui, show: &mut Show, state: &mut NcodViewState, undo: &mut UndoStack) -> bool {
    curve_editor::ui(ui.ctx(), show, state);
    let mut dirty = false;
    ui.horizontal_wrapped(|ui| {
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
        ui.add(
            egui::Label::new(theme::label_muted(
                "Scroll zooms · drag a track edge to resize",
            ))
            .truncate(),
        );
    });
    dirty |= minimap(ui, show, state);
    let spacing_y = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 0.0;
    let (ruler_row, ruler_response) =
        ui.allocate_exact_size(vec2(visible_width(ui), RULER_H), Sense::click_and_drag());
    let mut lanes = None;
    let scroll_style = ui.spacing().scroll;
    ui.style_mut().spacing.scroll.floating = false;
    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .drag_to_scroll(false)
        .show(ui, |ui| {
            lanes = Some(timeline_lanes(
                ui,
                show,
                state,
                undo,
                ruler_row,
                &ruler_response,
            ));
        });
    ui.style_mut().spacing.scroll = scroll_style;
    ui.spacing_mut().item_spacing.y = spacing_y;
    dirty |= shape_menu(ui.ctx(), show, state, undo);
    let Some(lanes) = lanes else {
        return dirty;
    };
    dirty |= lanes.dirty;

    let ruler = Rect::from_min_max(
        pos2(lanes.body.left(), ruler_row.top()),
        pos2(lanes.body.right(), ruler_row.bottom()),
    );
    let corner = Rect::from_min_max(ruler_row.min, pos2(ruler.left(), ruler_row.bottom()));
    let painter = ui.painter();
    painter.rect_filled(ruler_row, 0.0, theme::RULER_BG);
    painter.rect_filled(corner, 0.0, theme::BG_PANEL);
    draw_ruler(
        &painter,
        ruler,
        state.view_start_ms,
        state.view_end_ms,
        state.snap_ms,
    );
    draw_range(
        &painter,
        ruler,
        state,
        Color32::from_rgba_unmultiplied(100, 160, 220, 90),
    );
    let play_x = time_to_x(
        ruler,
        state.view_start_ms,
        state.view_end_ms,
        f64::from(state.play_ms),
    );
    painter.line_segment(
        [pos2(play_x, ruler.top()), pos2(play_x, ruler.bottom())],
        egui::Stroke::new(2.0_f32, theme::PLAYHEAD),
    );
    painter.circle_filled(pos2(play_x, ruler.top() + 6.0), 5.0, theme::PLAYHEAD);
    painter.line_segment(
        [ruler_row.left_bottom(), ruler_row.right_bottom()],
        egui::Stroke::new(1.0_f32, theme::GRID_MAJOR),
    );

    let holding = ruler_response.dragged()
        || ruler_response.is_pointer_button_down_on()
        || lanes.response.dragged()
        || lanes.response.is_pointer_button_down_on();
    if state.follow_playhead && state.playing && !holding {
        center_window_on(
            state,
            f64::from(state.play_ms),
            f64::from(show.end_ms().max(1)),
        );
    }
    dirty
}

struct Lanes {
    dirty: bool,
    body: Rect,
    response: Response,
}

struct TrackRows {
    heights: Vec<f32>,
    tops: Vec<f32>,
}

impl TrackRows {
    fn total(&self) -> f32 {
        match (self.tops.last(), self.heights.last()) {
            (Some(top), Some(height)) => top + height,
            _ => LANE_H,
        }
    }

    fn height(&self, lane: usize) -> f32 {
        self.heights.get(lane).copied().unwrap_or(LANE_H)
    }

    fn top(&self, lane: usize) -> f32 {
        self.tops.get(lane).copied().unwrap_or(0.0)
    }

    fn value_span(&self, lane: usize) -> f32 {
        (self.height(lane) - 8.0).max(1.0)
    }

    fn value_span_for_slot(&self, state: &NcodViewState, slot: usize) -> f32 {
        state
            .pinned
            .iter()
            .position(|channel| usize::from(*channel) == slot + 1)
            .map(|lane| self.value_span(lane))
            .unwrap_or(LANE_H - 8.0)
    }

    fn rect(&self, area: Rect, lane: usize) -> Rect {
        Rect::from_min_size(
            pos2(area.left(), area.top() + self.top(lane)),
            vec2(area.width(), self.height(lane)),
        )
    }

    fn index_at(&self, y: f32, top: f32) -> Option<usize> {
        if self.heights.is_empty() || y < top {
            return None;
        }
        let mut cursor = top;
        for (lane, height) in self.heights.iter().enumerate() {
            let bottom = cursor + *height;
            if y <= bottom {
                return Some(lane);
            }
            cursor = bottom;
        }
        None
    }

    fn nearest(&self, y: f32, top: f32) -> Option<usize> {
        if self.heights.is_empty() {
            return None;
        }
        if y <= top {
            return Some(0);
        }
        self.index_at(y, top).or(Some(self.heights.len() - 1))
    }

    fn seam(&self, area: Rect, pos: egui::Pos2) -> Option<usize> {
        if pos.x < area.left() || pos.x > area.right() {
            return None;
        }
        for lane in 0..self.heights.len() {
            let lane_top = area.top() + self.top(lane);
            let bottom = lane_top + self.height(lane);
            if (pos.y - bottom).abs() <= SEAM_HIT && pos.y >= lane_top + SEAM_HIT {
                return Some(lane);
            }
        }
        None
    }
}

fn resize_seam(
    show: &Show,
    state: &NcodViewState,
    rows: &TrackRows,
    area: Rect,
    header: Rect,
    body: Rect,
    pos: egui::Pos2,
) -> Option<usize> {
    let lane = rows.seam(area, pos)?;
    let header_lane = rows.rect(header, lane);
    let (curve, solo, level) = header_controls(header_lane);
    if curve.contains(pos)
        || solo.contains(pos)
        || level.contains(pos)
        || hit_pinned_key(show, state, body, rows, pos).is_some()
    {
        return None;
    }
    Some(lane)
}

fn track_rows(state: &NcodViewState) -> TrackRows {
    let mut tops = Vec::with_capacity(state.pinned.len());
    let mut heights = Vec::with_capacity(state.pinned.len());
    let mut y = 0.0;
    for &channel in &state.pinned {
        tops.push(y);
        let height = state.track_height(channel);
        heights.push(height);
        y += height;
    }
    TrackRows { heights, tops }
}

fn minimap(ui: &mut Ui, show: &Show, state: &mut NcodViewState) -> bool {
    let mut changed = false;
    let end_ms = f64::from(show.end_ms().max(1));
    let w = visible_width(ui);
    let (rect, response) = ui.allocate_exact_size(vec2(w, MINIMAP_H), Sense::click_and_drag());
    let painter = ui.painter_at(rect).with_clip_rect(rect);
    painter.rect_filled(rect, 4.0, Color32::from_rgb(18, 18, 20));

    let tracks = state.selected_tracks.clone();
    for track in &tracks {
        let Some(channel) = state.pinned.get(*track).copied() else {
            continue;
        };
        if !(1..=512).contains(&channel) {
            continue;
        }
        let slot = usize::from(channel - 1);
        let keys = show.channel_keys(slot);
        let cols = (rect.width().max(1.0) as usize).min(800);
        let mut points = Vec::with_capacity(cols);
        for col in 0..cols {
            let t = ((col as f64 + 0.5) / cols as f64) * end_ms;
            let value = crate::show::lerp_keys(&keys, t as u32);
            let x = rect.left() + (col as f32 / cols as f32) * rect.width();
            let y = rect.bottom() - 3.0 - (value as f32 / 255.0) * (rect.height() - 8.0);
            points.push(pos2(x, y));
        }
        if points.len() >= 2 {
            painter.add(egui::Shape::line(
                points,
                egui::Stroke::new(1.2_f32, curve_color(state, *track, channel)),
            ));
        }
    }
    let label = match tracks.len() {
        0 => "full show".to_string(),
        1 => {
            let channel = state.pinned.get(tracks[0]).copied().unwrap_or(1);
            format!("DMX {channel}  ·  full show")
        }
        count => format!("{count} tracks  ·  full show"),
    };
    painter.text(
        pos2(rect.left() + 6.0, rect.top() + 8.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::monospace(10.0),
        Color32::from_white_alpha(140),
    );

    let v0 = time_to_x(rect, 0.0, end_ms, state.view_start_ms);
    let v1 = time_to_x(rect, 0.0, end_ms, state.view_end_ms);
    let viewport = zoom_frame(rect, v0, v1);
    painter.rect_stroke(viewport, 2.0, egui::Stroke::new(1.5_f32, theme::ACCENT));

    let play_ms = state.play_ms;
    let px = time_to_x(rect, 0.0, end_ms, f64::from(play_ms));
    painter.line_segment(
        [pos2(px, rect.top()), pos2(px, rect.bottom())],
        egui::Stroke::new(1.5_f32, theme::PLAYHEAD),
    );

    let hit = zoom_frame_hit(viewport);
    if response.drag_started() {
        let on_frame = ui
            .input(|i| i.pointer.press_origin())
            .is_some_and(|origin| hit.contains(origin));
        ui.data_mut(|data| data.insert_temp(response.id, on_frame));
    }
    let dragging_frame =
        response.dragged() && ui.data(|data| data.get_temp::<bool>(response.id).unwrap_or(false));
    if dragging_frame {
        let delta = response.drag_delta();
        if delta.y != 0.0 {
            let span = view_span(state, end_ms);
            let new_span = (span * (f64::from(delta.y) / 96.0).exp()).clamp(20.0, end_ms);
            let center = state.view_start_ms + span * 0.5;
            set_window(state, center - new_span * 0.5, new_span, end_ms);
            changed = true;
        }
        if delta.x != 0.0 {
            let span = view_span(state, end_ms);
            let delta_ms = f64::from(delta.x) / f64::from(rect.width().max(1.0)) * end_ms;
            set_window(state, state.view_start_ms + delta_ms, span, end_ms);
            changed = true;
        }
    } else if let Some(pos) = response.interact_pointer_pos() {
        if response.clicked() || response.dragged() {
            let ms = x_to_time(rect, 0.0, end_ms, pos.x);
            let span = state.view_end_ms - state.view_start_ms;
            state.view_start_ms = (ms - span * 0.5).clamp(0.0, end_ms);
            state.view_end_ms = (state.view_start_ms + span).clamp(span, end_ms);
            changed = true;
        }
    }
    if dragging_frame
        || ui
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|pos| hit.contains(pos))
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    changed
}

fn zoom_frame(rect: Rect, v0: f32, v1: f32) -> Rect {
    let min_w = 8.0;
    let (left, right) = if v1 - v0 >= min_w {
        (v0, v1)
    } else {
        let mid = (v0 + v1) * 0.5;
        let left = (mid - min_w * 0.5).clamp(rect.left(), (rect.right() - min_w).max(rect.left()));
        (left, left + min_w)
    };
    Rect::from_min_max(
        pos2(left, rect.top() + 2.0),
        pos2(right.min(rect.right()), rect.bottom() - 2.0),
    )
}

fn zoom_frame_hit(frame: Rect) -> Rect {
    let min_w = 16.0;
    Rect::from_center_size(
        frame.center(),
        vec2(frame.width().max(min_w), frame.height() + 8.0),
    )
}

fn timeline_lanes(
    ui: &mut Ui,
    show: &mut Show,
    state: &mut NcodViewState,
    undo: &mut UndoStack,
    ruler_row: Rect,
    ruler_response: &Response,
) -> Lanes {
    let mut dirty = false;
    let end_ms = f64::from(show.end_ms().max(1));
    let rows = track_rows(state);
    let (rect, response) = ui.allocate_exact_size(
        vec2(visible_width(ui), rows.total()),
        Sense::click_and_drag(),
    );
    let header = Rect::from_min_max(rect.min, pos2(rect.left() + HEADER_W, rect.bottom()));
    let full_body = Rect::from_min_max(pos2(rect.left() + HEADER_W, rect.top()), rect.max);
    let body =
        full_body.with_max_x((full_body.right() - PLOT_RIGHT_PAD).max(full_body.left() + 1.0));
    #[cfg(test)]
    ui.ctx().data_mut(|data| {
        data.insert_temp(egui::Id::new("timeline_plot"), body);
        data.insert_temp(egui::Id::new("timeline_clip"), ui.clip_rect());
    });
    let lane_area = Rect::from_min_max(header.min, full_body.max);
    let hot_seam = response
        .hover_pos()
        .and_then(|pos| resize_seam(show, state, &rows, lane_area, header, body, pos));
    if state.lane_resize.is_some() || hot_seam.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let ruler = Rect::from_min_max(
        pos2(body.left(), ruler_row.top()),
        pos2(body.right(), ruler_row.bottom()),
    );
    let mut viewport = ui.clip_rect();
    viewport.min.y = viewport.min.y.max(ruler.bottom());

    dirty |= handle_zoom_pan(ruler_response, ui, state, end_ms, ruler);
    dirty |= handle_zoom_pan(&response, ui, state, end_ms, body.intersect(viewport));
    dirty |= handle_pointer(
        show,
        state,
        undo,
        ruler_response,
        &response,
        header,
        body,
        ruler,
        viewport,
        &rows,
    );
    if state.lane_resize.is_none() && hot_seam.is_none() {
        let command = ui.input(|input| input.modifiers.command);
        let plot = full_body.intersect(viewport);
        if command && response.hover_pos().is_some_and(|pos| plot.contains(pos)) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    }

    let painter = ui.painter_at(rect);
    painter.rect_filled(header, 0.0, theme::BG_PANEL);
    painter.rect_filled(full_body, 0.0, theme::BG_DEEP);
    draw_grid(
        &painter,
        body,
        state.view_start_ms,
        state.view_end_ms,
        state.snap_ms,
    );

    let solo_active = !state.solo.is_empty();
    for (lane, &channel) in state.pinned.iter().enumerate() {
        let lane_rect = rows.rect(body, lane);
        let lane_fill = lane_rect.with_max_x(full_body.right());
        let header_lane = rows.rect(header, lane);
        let selected = state.track_is_selected(lane);
        let mcp = state.mcp_touched.contains(&channel);
        let bg = if mcp {
            Color32::from_rgb(62, 22, 22)
        } else if selected {
            Color32::from_rgb(36, 52, 70)
        } else if lane % 2 == 0 {
            theme::BG_LANE
        } else {
            theme::BG_LANE_ALT
        };
        painter.rect_filled(lane_fill, 0.0, bg);
        painter.rect_filled(
            header_lane,
            0.0,
            if mcp {
                Color32::from_rgb(84, 26, 26)
            } else if selected {
                Color32::from_rgb(46, 74, 108)
            } else {
                bg
            },
        );
        if mcp || selected {
            painter.rect_filled(
                Rect::from_min_size(header_lane.min, vec2(4.0, header_lane.height())),
                0.0,
                if mcp { theme::MCP_TRACK } else { theme::ACCENT },
            );
        }
        draw_level_scale(&painter, lane_rect);
        let color = curve_color(state, lane, channel);
        let now = show.value_at(usize::from(channel.saturating_sub(1)), state.play_ms);
        let sent = super::playback::scale_level(now, state.levels.get(channel));
        let sent = state.curves.lookup(channel, sent);
        let readout = if state.curves.is_shaped(channel) {
            format!("{now}→{sent}")
        } else {
            now.to_string()
        };
        let (curve_rect, solo_rect, level_rect) = header_controls(header_lane);
        let labels = painter.with_clip_rect(Rect::from_min_max(
            header_lane.min,
            pos2(curve_rect.left() - 4.0, header_lane.bottom()),
        ));
        let name_color = if mcp {
            theme::MCP_TRACK
        } else if state.track_is_selected(lane) {
            Color32::WHITE
        } else {
            Color32::from_gray(185)
        };
        if header_lane.height() < 64.0 {
            labels.text(
                pos2(header_lane.left() + 10.0, header_lane.center().y),
                egui::Align2::LEFT_CENTER,
                format!("DMX {channel}  {readout}"),
                egui::FontId::monospace(11.0),
                name_color,
            );
        } else {
            labels.text(
                pos2(header_lane.left() + 10.0, header_lane.center().y - 7.0),
                egui::Align2::LEFT_CENTER,
                format!("DMX {channel}"),
                egui::FontId::monospace(12.0),
                name_color,
            );
            labels.text(
                pos2(header_lane.left() + 10.0, header_lane.center().y + 8.0),
                egui::Align2::LEFT_CENTER,
                format!("{readout}"),
                egui::FontId::monospace(11.0),
                color,
            );
        }
        let before = state.levels.get(channel);
        let mut level = before;
        let level_response = fader::track(
            ui,
            ui.id().with(("track-level", channel)),
            level_rect,
            &mut level,
            color,
        );
        if level_response.double_clicked() || level != before {
            state.set_track_level(channel, level);
        }
        let soloed = state.solo.contains(channel);
        let solo_button = egui::Button::new(egui::RichText::new("S").monospace().size(12.0).color(
            if soloed {
                Color32::BLACK
            } else {
                Color32::from_gray(170)
            },
        ))
        .min_size(vec2(22.0, 22.0))
        .fill(if soloed {
            theme::SOLO
        } else {
            Color32::from_rgb(48, 48, 52)
        });
        if ui.put(solo_rect, solo_button).clicked() {
            state.solo_channel(channel);
        }
        let shaped = state.curves.is_shaped(channel);
        let curve_button = egui::Button::new(
            egui::RichText::new("C")
                .monospace()
                .size(12.0)
                .color(if shaped {
                    Color32::BLACK
                } else {
                    Color32::from_gray(170)
                }),
        )
        .min_size(vec2(22.0, 22.0))
        .fill(if shaped {
            theme::ACCENT
        } else {
            Color32::from_rgb(48, 48, 52)
        });
        if ui
            .put(curve_rect, curve_button)
            .on_hover_text(
                "Transmission curve. Maps the programmed level to the DMX sent to the lamp.",
            )
            .clicked()
        {
            state.curve_editor = if state.curve_editor == Some(channel) {
                None
            } else {
                Some(channel)
            };
        }
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
            if state.show_keyframes {
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
        if solo_active && !soloed {
            painter.rect_filled(lane_fill, 0.0, Color32::from_black_alpha(110));
        }
        if level == 0 {
            painter.rect_filled(lane_fill, 0.0, Color32::from_black_alpha(120));
        }
        if mcp || selected {
            let mark = if mcp { theme::MCP_TRACK } else { theme::ACCENT };
            painter.rect_filled(
                Rect::from_min_size(lane_fill.left_top(), vec2(4.0, lane_fill.height())),
                0.0,
                mark,
            );
            painter.rect_stroke(lane_fill, 0.0, egui::Stroke::new(1.5_f32, mark));
            painter.rect_stroke(header_lane, 0.0, egui::Stroke::new(1.5_f32, mark));
        }
        let seam_hot = hot_seam == Some(lane)
            || state
                .lane_resize
                .as_ref()
                .is_some_and(|resize| resize.tracks.iter().any(|(id, _)| *id == channel));
        painter.hline(
            (header_lane.left() + 10.0)..=lane_fill.right(),
            header_lane.bottom() - 1.0,
            egui::Stroke::new(
                if seam_hot { 2.0 } else { 1.0_f32 },
                if seam_hot {
                    theme::ACCENT
                } else {
                    Color32::from_white_alpha(70)
                },
            ),
        );
    }

    draw_range(
        &painter,
        body,
        state,
        Color32::from_rgba_unmultiplied(100, 160, 220, 48),
    );
    draw_marquee(&painter, state);

    let play_x = time_to_x(
        body,
        state.view_start_ms,
        state.view_end_ms,
        f64::from(state.play_ms),
    );
    painter.line_segment(
        [pos2(play_x, body.top()), pos2(play_x, body.bottom())],
        egui::Stroke::new(2.0_f32, theme::PLAYHEAD),
    );

    Lanes {
        dirty,
        body,
        response,
    }
}

fn handle_pointer(
    show: &mut Show,
    state: &mut NcodViewState,
    undo: &mut UndoStack,
    ruler_response: &Response,
    lanes_response: &Response,
    header: Rect,
    body: Rect,
    ruler: Rect,
    viewport: Rect,
    rows: &TrackRows,
) -> bool {
    if shape_menu_open(&lanes_response.ctx) {
        return false;
    }
    let mut dirty = false;
    let body_top = body.top();
    let body_hit = body.intersect(viewport);
    let header_hit = header.intersect(viewport);
    let (pressed, released, shift, command) = ruler_response.ctx.input(|input| {
        (
            input.pointer.primary_pressed(),
            input.pointer.primary_released(),
            input.modifiers.shift,
            input.modifiers.command,
        )
    });
    let pos = ruler_response
        .interact_pointer_pos()
        .or_else(|| lanes_response.interact_pointer_pos());
    let holding =
        ruler_response.is_pointer_button_down_on() || lanes_response.is_pointer_button_down_on();

    if pressed {
        if let Some(pos) = pos {
            let lane_area = Rect::from_min_max(
                header.min,
                pos2(body.right() + PLOT_RIGHT_PAD, body.bottom()),
            );
            if let Some(lane) = resize_seam(show, state, rows, lane_area, header, body, pos) {
                state.finish_gesture(undo, show);
                state.begin_track_resize(lane, pos.y);
            } else if header_hit.contains(pos) && !ruler.contains(pos) {
                if let Some(lane) = rows.index_at(pos.y, body_top) {
                    let header_lane = rows.rect(header, lane);
                    let (curve_rect, solo_rect, level_rect) = header_controls(header_lane);
                    if !curve_rect.contains(pos)
                        && !solo_rect.contains(pos)
                        && !level_rect.contains(pos)
                    {
                        if command {
                            state.toggle_track(lane);
                        } else if shift {
                            state.select_track_range(lane);
                        } else {
                            state.select_track(lane);
                        }
                    }
                }
            } else {
                dirty |= begin_gesture(
                    show, state, body, body_hit, ruler, rows, pos, shift, command,
                );
            }
        }
    }

    if state.lane_resize.is_some() {
        if let Some(pos) =
            pos.or_else(|| ruler_response.ctx.input(|input| input.pointer.latest_pos()))
        {
            state.drag_track_resize(pos.y);
        }
        let stop =
            released || ruler_response.drag_stopped() || lanes_response.drag_stopped() || !holding;
        if stop {
            let reset = lanes_response.double_clicked() || ruler_response.double_clicked();
            state.end_track_resize(reset);
        } else {
            ruler_response
                .ctx
                .set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        return dirty;
    }

    let tracking = state.gesture.is_some() || holding;
    if tracking {
        if let Some(pos) =
            pos.or_else(|| ruler_response.ctx.input(|input| input.pointer.latest_pos()))
        {
            let shift_now = ruler_response.ctx.input(|input| input.modifiers.shift);
            dirty |= continue_gesture(
                show, state, body, body_hit, ruler, rows, pos, holding, shift_now,
            );
        }
    }

    if released || ruler_response.drag_stopped() || lanes_response.drag_stopped() {
        if let Some(Gesture::Marquee {
            origin,
            current,
            mode,
            ..
        }) = &state.gesture
        {
            let origin = *origin;
            let current = *current;
            let mode = *mode;
            if !marquee_moved(origin, current)
                && mode == MarqueeMode::Replace
                && body_hit.contains(origin)
            {
                state.selected_keys.clear();
                let ms = x_to_time(body, state.view_start_ms, state.view_end_ms, origin.x) as u32;
                state.place_time(show, ms);
            }
        }
        state.finish_gesture(undo, show);
    }
    open_shape_menu(lanes_response, show, state, body, body_hit, rows);
    dirty
}

fn begin_gesture(
    show: &mut Show,
    state: &mut NcodViewState,
    body: Rect,
    body_hit: Rect,
    ruler: Rect,
    rows: &TrackRows,
    pos: egui::Pos2,
    shift: bool,
    command: bool,
) -> bool {
    let on_ruler = ruler.contains(pos);
    let on_body = !on_ruler && body_hit.contains(pos);
    let near_edge = !on_ruler && body_hit.expand2(vec2(16.0, 10.0)).contains(pos);
    if !on_ruler && !on_body && !near_edge {
        return false;
    }
    if on_ruler {
        if shift {
            let seek_ms = x_to_time(body, state.view_start_ms, state.view_end_ms, pos.x) as u32;
            state.gesture = Some(Gesture::Range { start_ms: seek_ms });
            state.range_ms = Some((seek_ms, seek_ms));
        } else {
            state.gesture = Some(Gesture::Scrub);
            scrub(show, state, body, pos);
        }
        return false;
    }
    if command && (on_body || near_edge) {
        return start_brush(show, state, body, rows, pos);
    }
    if let Some((frame, slot, lane)) = hit_pinned_key(show, state, body, rows, pos) {
        if shift {
            if !state.selected_keys.iter().any(|key| *key == (frame, slot)) {
                state.selected_keys.push((frame, slot));
            }
        } else if !state.selected_keys.iter().any(|key| *key == (frame, slot)) {
            state.selected_keys = vec![(frame, slot)];
        }
        state.focus_track(lane);
        state.place_playhead(show, frame);
        state.selected = vec![frame];
        state.gesture = Some(capture_keys(show, state, pos));
        return false;
    }
    if !on_body {
        return false;
    }
    if near_playhead(state, body, pos.x) {
        state.gesture = Some(Gesture::Scrub);
        scrub(show, state, body, pos);
        return false;
    }
    let mode = if shift {
        MarqueeMode::Add
    } else {
        MarqueeMode::Replace
    };
    let base = state.selected_keys.clone();
    if mode == MarqueeMode::Replace {
        state.selected_keys.clear();
    }
    state.gesture = Some(Gesture::Marquee {
        origin: pos,
        current: pos,
        mode,
        base,
    });
    false
}

fn start_brush(
    show: &mut Show,
    state: &mut NcodViewState,
    body: Rect,
    rows: &TrackRows,
    pos: egui::Pos2,
) -> bool {
    if let Some(lane) = rows.index_at(pos.y, body.top()) {
        state.focus_track(lane);
    }
    state.gesture = Some(Gesture::Paint {
        originals: Vec::new(),
        seen: HashSet::new(),
        cursor: None,
        origin: pos,
        stroking: false,
    });
    paint_pointer(show, state, body, rows, pos)
}

fn continue_gesture(
    show: &mut Show,
    state: &mut NcodViewState,
    body: Rect,
    body_hit: Rect,
    ruler: Rect,
    rows: &TrackRows,
    pos: egui::Pos2,
    holding: bool,
    shift: bool,
) -> bool {
    let live = match &state.gesture {
        Some(Gesture::Range { start_ms }) => LiveGesture::Range(*start_ms),
        Some(Gesture::Marquee { origin, .. }) => LiveGesture::Marquee(*origin),
        Some(Gesture::Keys {
            origin,
            points,
            move_time,
            ..
        }) => LiveGesture::Keys {
            origin: *origin,
            points: points.clone(),
            move_time: *move_time,
        },
        Some(Gesture::Keyframe {
            frame,
            slot,
            before_value,
            origin,
            move_time,
            ..
        }) => LiveGesture::Keyframe {
            frame: *frame,
            slot: *slot,
            before_value: *before_value,
            origin: *origin,
            move_time: *move_time,
        },
        Some(Gesture::Paint { .. }) => LiveGesture::Paint,
        Some(Gesture::Scrub) | Some(Gesture::Value { .. }) | None => LiveGesture::Scrub,
    };
    match live {
        LiveGesture::Range(start_ms) => {
            let seek_ms = x_to_time(body, state.view_start_ms, state.view_end_ms, pos.x) as u32;
            state.range_ms = Some((start_ms.min(seek_ms), start_ms.max(seek_ms)));
            false
        }
        LiveGesture::Marquee(origin) => {
            let (mode, base) = match &state.gesture {
                Some(Gesture::Marquee { mode, base, .. }) => (*mode, base.clone()),
                _ => return false,
            };
            if let Some(Gesture::Marquee { current, .. }) = &mut state.gesture {
                *current = pos;
            }
            if marquee_moved(origin, pos) {
                let hits = keys_in_rect(show, state, body, rows, origin, pos);
                state.selected_keys = combine_marquee(&base, hits, mode);
            }
            false
        }
        LiveGesture::Keys {
            origin,
            points,
            move_time,
        } => drag_keys(
            show, state, body, rows, pos, origin, &points, move_time, shift,
        ),
        LiveGesture::Keyframe {
            frame,
            slot,
            before_value,
            origin,
            move_time,
        } => drag_keyframe(
            show,
            state,
            body,
            rows,
            pos,
            frame,
            slot,
            before_value,
            origin,
            move_time,
        ),
        LiveGesture::Paint => {
            let (origin, stroking) = match &state.gesture {
                Some(Gesture::Paint {
                    origin, stroking, ..
                }) => (*origin, *stroking),
                _ => return false,
            };
            if !stroking && !marquee_moved(origin, pos) {
                return false;
            }
            if let Some(Gesture::Paint { stroking, .. }) = &mut state.gesture {
                *stroking = true;
            }
            paint_pointer(show, state, body, rows, pos)
        }
        LiveGesture::Scrub => {
            if holding && (body_hit.contains(pos) || ruler.contains(pos)) {
                scrub(show, state, body, pos);
            }
            false
        }
    }
}

enum LiveGesture {
    Range(u32),
    Keyframe {
        frame: usize,
        slot: usize,
        before_value: u8,
        origin: egui::Pos2,
        move_time: Option<bool>,
    },
    Marquee(egui::Pos2),
    Keys {
        origin: egui::Pos2,
        points: Vec<(usize, usize, u8, u32)>,
        move_time: Option<bool>,
    },
    Paint,
    Scrub,
}

fn drag_keyframe(
    show: &mut Show,
    state: &mut NcodViewState,
    body: Rect,
    rows: &TrackRows,
    pos: egui::Pos2,
    frame: usize,
    slot: usize,
    before_value: u8,
    origin: egui::Pos2,
    move_time: Option<bool>,
) -> bool {
    let delta = pos - origin;
    let move_time = if let Some(axis) = move_time {
        axis
    } else if delta.x.abs() > 4.0 || delta.y.abs() > 4.0 {
        let time = delta.x.abs() > delta.y.abs();
        if let Some(Gesture::Keyframe { move_time, .. }) = &mut state.gesture {
            *move_time = Some(time);
        }
        time
    } else {
        return false;
    };
    if move_time {
        let pointer_ms = snap_ms(
            x_to_time(body, state.view_start_ms, state.view_end_ms, pos.x) as u32,
            state.snap_ms,
        );
        let (min_ms, max_ms) = show.frame_time_bounds(frame);
        let at = pointer_ms.clamp(min_ms, max_ms);
        let Some(current) = show.frames.get_mut(frame) else {
            return false;
        };
        if current.at_ms == at {
            return false;
        }
        current.at_ms = at;
        if state.playhead == frame {
            state.play_ms = at;
            if state.playing {
                state.arm_playback_clock();
                if let Some(handle) = &state.playback_handle {
                    state.play_gen = handle.seek_time(at);
                }
            }
        }
        return true;
    }
    let span = rows.value_span_for_slot(state, slot);
    let delta_value = (origin.y - pos.y) / span * 255.0;
    let after = (f32::from(before_value) + delta_value)
        .round()
        .clamp(0.0, 255.0) as u8;
    let Some(current) = show.frames.get_mut(frame) else {
        return false;
    };
    if current.slots[slot] == after {
        return false;
    }
    current.set_key(slot, after);
    true
}

fn near_playhead(state: &NcodViewState, body: Rect, x: f32) -> bool {
    let play_x = time_to_x(
        body,
        state.view_start_ms,
        state.view_end_ms,
        f64::from(state.play_ms),
    );
    (x - play_x).abs() <= 7.0
}

fn scrub(show: &mut Show, state: &mut NcodViewState, body: Rect, pos: egui::Pos2) {
    let ms = x_to_time(body, state.view_start_ms, state.view_end_ms, pos.x) as u32;
    state.place_time(show, ms);
}

fn paint_pointer(
    show: &mut Show,
    state: &mut NcodViewState,
    body: Rect,
    rows: &TrackRows,
    pos: egui::Pos2,
) -> bool {
    let Some(lane) = rows.nearest(pos.y, body.top()) else {
        return false;
    };
    let Some(slot) = channel_slot(state, lane) else {
        return false;
    };
    let lane_rect = rows.rect(body, lane);
    let pointer_ms = snap_ms(
        x_to_time(body, state.view_start_ms, state.view_end_ms, pos.x) as u32,
        state.snap_ms,
    );
    let index = nearest_frame(show, pointer_ms);
    let dirty = paint_stroke(show, state, index, slot, lane_value(lane_rect, pos.y));
    state.place_playhead(show, index);
    dirty
}

fn paint_stroke(
    show: &mut Show,
    state: &mut NcodViewState,
    index: usize,
    slot: usize,
    value: u8,
) -> bool {
    let cursor = match &state.gesture {
        Some(Gesture::Paint { cursor, .. }) => *cursor,
        _ => None,
    };
    let mut dirty = false;
    if let Some((prev_index, prev_slot, prev_value)) = cursor {
        if prev_slot == slot && prev_index != index {
            let (start, end, start_value, end_value) = if prev_index < index {
                (prev_index, index, prev_value, value)
            } else {
                (index, prev_index, value, prev_value)
            };
            let span = (end - start) as f32;
            for frame_index in start..=end {
                let u = (frame_index - start) as f32 / span;
                let sample = (f32::from(start_value)
                    + (f32::from(end_value) - f32::from(start_value)) * u)
                    .round() as u8;
                dirty |= paint_frame(show, state, frame_index, slot, sample);
            }
        } else {
            dirty |= paint_frame(show, state, index, slot, value);
        }
    } else {
        dirty |= paint_frame(show, state, index, slot, value);
    }
    if let Some(Gesture::Paint { cursor, .. }) = &mut state.gesture {
        *cursor = Some((index, slot, value));
    }
    dirty
}

fn paint_frame(
    show: &mut Show,
    state: &mut NcodViewState,
    index: usize,
    slot: usize,
    value: u8,
) -> bool {
    let before = match show.frames.get(index) {
        Some(frame) => frame.slots[slot],
        None => return false,
    };
    if let Some(Gesture::Paint {
        originals, seen, ..
    }) = &mut state.gesture
    {
        if seen.insert((index, slot)) {
            originals.push((index, slot, before));
        }
    }
    if before == value {
        return false;
    }
    show.frames[index].set_key(slot, value);
    true
}

fn header_controls(header_lane: Rect) -> (Rect, Rect, Rect) {
    let level = Rect::from_min_max(
        pos2(header_lane.right() - 32.0, header_lane.top() + 6.0),
        pos2(header_lane.right() - 6.0, header_lane.bottom() - 6.0),
    );
    let solo = Rect::from_min_size(
        pos2(level.left() - 26.0, header_lane.center().y - 11.0),
        vec2(22.0, 22.0),
    );
    let curve = Rect::from_min_size(
        pos2(solo.left() - 26.0, header_lane.center().y - 11.0),
        vec2(22.0, 22.0),
    );
    (curve, solo, level)
}

fn curve_color(state: &NcodViewState, lane: usize, channel: u16) -> Color32 {
    if state.mcp_touched.contains(&channel) {
        theme::MCP_TRACK
    } else {
        theme::track_color(lane)
    }
}

fn channel_slot(state: &NcodViewState, lane: usize) -> Option<usize> {
    let channel = *state.pinned.get(lane)?;
    (1..=512)
        .contains(&channel)
        .then_some(usize::from(channel - 1))
}

fn draw_range(painter: &Painter, rect: Rect, state: &NcodViewState, fill: Color32) {
    let Some((start, end)) = state.range_ms else {
        return;
    };
    let x0 = time_to_x(
        rect,
        state.view_start_ms,
        state.view_end_ms,
        f64::from(start),
    );
    let x1 = time_to_x(rect, state.view_start_ms, state.view_end_ms, f64::from(end));
    let left = x0.min(x1);
    let right = x0.max(x1).max(left + 1.0);
    painter.rect_filled(
        Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom())),
        0.0,
        fill,
    );
    let edge = egui::Stroke::new(1.5_f32, theme::ACCENT);
    painter.line_segment([pos2(left, rect.top()), pos2(left, rect.bottom())], edge);
    painter.line_segment([pos2(right, rect.top()), pos2(right, rect.bottom())], edge);
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

fn fade_range(show: &mut Show, state: &mut NcodViewState, undo: &mut UndoStack) -> bool {
    let Some((start, end)) = state.range_ms else {
        return false;
    };
    let slots = state.selected_slots();
    if slots.is_empty() || end <= start {
        return false;
    }
    let first = nearest_frame(show, start);
    let last = nearest_frame(show, end);
    if last <= first {
        return false;
    }
    let mut batch = Vec::new();
    let span = (last - first) as f32;
    for slot in slots {
        let from = show.frames[first].slots[slot];
        let to = show.frames[last].slots[slot];
        for index in first..=last {
            let before = show.frames[index].slots[slot];
            let u = (index - first) as f32 / span;
            let after = (f32::from(from) + (f32::from(to) - f32::from(from)) * u).round() as u8;
            if before != after {
                show.frames[index].set_key(slot, after);
                batch.push(super::undo::UndoOp::Slot {
                    frame: index,
                    slot,
                    before,
                    after,
                });
            }
        }
    }
    let changed = !batch.is_empty();
    undo.push(batch);
    changed
}

fn capture_keys(show: &Show, state: &NcodViewState, origin: egui::Pos2) -> Gesture {
    let points = state
        .selected_keys
        .iter()
        .filter_map(|(frame, slot)| {
            let current = show.frames.get(*frame)?;
            Some((*frame, *slot, current.slots[*slot], current.at_ms))
        })
        .collect();
    Gesture::Keys {
        origin,
        points,
        move_time: None,
        scale: None,
    }
}

fn drag_keys(
    show: &mut Show,
    state: &mut NcodViewState,
    body: Rect,
    rows: &TrackRows,
    pos: egui::Pos2,
    origin: egui::Pos2,
    points: &[(usize, usize, u8, u32)],
    move_time: Option<bool>,
    shift: bool,
) -> bool {
    let _ = move_time;
    let delta = pos - origin;
    if delta.x.abs() <= 4.0 && delta.y.abs() <= 4.0 {
        return false;
    }
    for (frame, slot, value, at_ms) in points {
        let Some(current) = show.frames.get_mut(*frame) else {
            continue;
        };
        current.set_key(*slot, *value);
        current.at_ms = *at_ms;
    }
    if shift {
        let span_ms = (state.view_end_ms - state.view_start_ms).max(1.0);
        let mut delta_ms =
            (f64::from(delta.x) / f64::from(body.width().max(1.0)) * span_ms).round() as i64;
        if state.snap_ms > 0 {
            let grid = i64::from(state.snap_ms);
            delta_ms = (delta_ms as f64 / grid as f64).round() as i64 * grid;
        }
        let originals: Vec<(usize, u32)> = points
            .iter()
            .map(|(frame, _, _, at_ms)| (*frame, *at_ms))
            .collect();
        let applied = shared_time_delta(show, &originals, delta_ms);
        let mut moved_playhead = None;
        let mut seen = HashSet::new();
        for (frame, _, _, at_ms) in points {
            if !seen.insert(*frame) {
                continue;
            }
            let at = (i64::from(*at_ms) + applied).clamp(0, i64::from(u32::MAX)) as u32;
            let Some(current) = show.frames.get_mut(*frame) else {
                continue;
            };
            current.at_ms = at;
            if state.playhead == *frame {
                moved_playhead = Some(at);
            }
        }
        if let Some(at) = moved_playhead {
            state.play_ms = at;
            if state.playing {
                state.arm_playback_clock();
                if let Some(handle) = &state.playback_handle {
                    state.play_gen = handle.seek_time(at);
                }
            }
        }
        return applied != 0;
    }
    let mut changed = false;
    for (frame, slot, value, _) in points {
        let span = rows.value_span_for_slot(state, *slot);
        let delta_value = (origin.y - pos.y) / span * 255.0;
        let after = (f32::from(*value) + delta_value).round().clamp(0.0, 255.0) as u8;
        let Some(current) = show.frames.get_mut(*frame) else {
            continue;
        };
        if current.slots[*slot] != after {
            current.set_key(*slot, after);
            changed = true;
        }
    }
    changed
}

fn shared_time_delta(show: &Show, originals: &[(usize, u32)], delta: i64) -> i64 {
    let chosen: HashSet<usize> = originals.iter().map(|(frame, _)| *frame).collect();
    let mut lo = i64::MIN / 4;
    let mut hi = i64::MAX / 4;
    for (index, at_ms) in originals {
        let at = i64::from(*at_ms);
        let min_ms = if *index == 0 || chosen.contains(&index.wrapping_sub(1)) {
            0
        } else {
            i64::from(show.frames[*index - 1].at_ms.saturating_add(1))
        };
        let max_ms = if *index + 1 >= show.frames.len() || chosen.contains(&(*index + 1)) {
            i64::from(u32::MAX)
        } else {
            i64::from(show.frames[*index + 1].at_ms.saturating_sub(1))
        };
        lo = lo.max(min_ms - at);
        hi = hi.min(max_ms - at);
    }
    if lo > hi {
        0
    } else {
        delta.clamp(lo, hi)
    }
}

fn keys_in_rect(
    show: &Show,
    state: &NcodViewState,
    body: Rect,
    rows: &TrackRows,
    origin: egui::Pos2,
    current: egui::Pos2,
) -> Vec<(usize, usize)> {
    let marquee = Rect::from_two_pos(origin, current);
    let rightward = current.x >= origin.x;
    let mut keys = Vec::new();
    for (lane, _) in state.pinned.iter().enumerate() {
        let Some(slot) = channel_slot(state, lane) else {
            continue;
        };
        let lane_rect = rows.rect(body, lane);
        if lane_rect.bottom() < marquee.top() || lane_rect.top() > marquee.bottom() {
            continue;
        }
        for (frame, sample) in show.frames.iter().enumerate() {
            if !is_envelope_point(show, frame, slot) {
                continue;
            }
            let x = time_to_x(
                lane_rect,
                state.view_start_ms,
                state.view_end_ms,
                f64::from(sample.at_ms),
            );
            let y = y_of(lane_rect, sample.slots[slot]);
            let point = Rect::from_center_size(pos2(x, y), vec2(8.0, 8.0));
            let inside = if rightward {
                marquee.intersects(point)
            } else {
                marquee.contains_rect(point)
            };
            if inside {
                keys.push((frame, slot));
            }
        }
    }
    keys
}

fn marquee_moved(origin: egui::Pos2, current: egui::Pos2) -> bool {
    (current - origin).length_sq() >= 25.0
}

fn combine_marquee(
    base: &[(usize, usize)],
    hits: Vec<(usize, usize)>,
    mode: MarqueeMode,
) -> Vec<(usize, usize)> {
    match mode {
        MarqueeMode::Replace => hits,
        MarqueeMode::Add => {
            let mut out = base.to_vec();
            for key in hits {
                if !out.contains(&key) {
                    out.push(key);
                }
            }
            out
        }
        MarqueeMode::Toggle => {
            let mut out = base.to_vec();
            for key in hits {
                if let Some(index) = out.iter().position(|item| *item == key) {
                    out.remove(index);
                } else {
                    out.push(key);
                }
            }
            out
        }
    }
}

fn hit_pinned_key(
    show: &Show,
    state: &NcodViewState,
    body: Rect,
    rows: &TrackRows,
    pos: egui::Pos2,
) -> Option<(usize, usize, usize)> {
    let mut best: Option<(usize, usize, usize, f32)> = None;
    for (lane, _) in state.pinned.iter().enumerate() {
        let Some(slot) = channel_slot(state, lane) else {
            continue;
        };
        let lane_rect = rows.rect(body, lane);
        let Some((frame, distance)) = hit_keyframe(show, slot, lane_rect, state, pos) else {
            continue;
        };
        let replace = best
            .map(|(_, _, _, previous)| distance < previous)
            .unwrap_or(true);
        if replace {
            best = Some((frame, slot, lane, distance));
        }
    }
    best.map(|(frame, slot, lane, _)| (frame, slot, lane))
}

fn draw_marquee(painter: &Painter, state: &NcodViewState) {
    let Some(Gesture::Marquee {
        origin, current, ..
    }) = &state.gesture
    else {
        return;
    };
    if !marquee_moved(*origin, *current) {
        return;
    }
    let rect = Rect::from_two_pos(*origin, *current);
    painter.rect_filled(
        rect,
        0.0,
        Color32::from_rgba_unmultiplied(100, 160, 220, 36),
    );
    painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0_f32, theme::ACCENT));
}

fn selected_on_slot(state: &NcodViewState, slot: usize) -> HashSet<usize> {
    state
        .selected_keys
        .iter()
        .filter(|(_, key_slot)| *key_slot == slot)
        .map(|(frame, _)| *frame)
        .collect()
}

fn hit_keyframe(
    show: &Show,
    slot: usize,
    lane_rect: Rect,
    state: &NcodViewState,
    pos: egui::Pos2,
) -> Option<(usize, f32)> {
    let selected = selected_on_slot(state, slot);
    let mut best: Option<(usize, f32)> = None;
    let mut consider = |fi: usize| {
        let Some(frame) = show.frames.get(fi) else {
            return;
        };
        let at = f64::from(frame.at_ms);
        if at < state.view_start_ms || at > state.view_end_ms {
            return;
        }
        let x = time_to_x(lane_rect, state.view_start_ms, state.view_end_ms, at);
        let y = y_of(lane_rect, frame.slots[slot]);
        let distance = pos2(x, y).distance(pos);
        let is_selected = selected.contains(&fi);
        if !is_selected && !frame.is_key(slot) {
            return;
        }
        let radius = key_grab_radius(lane_rect, x, y, is_selected);
        if distance > radius {
            return;
        }
        let replace = match best {
            None => true,
            Some((previous, previous_distance)) => {
                let previous_selected = selected.contains(&previous);
                if is_selected != previous_selected && (distance - previous_distance).abs() < 8.0 {
                    is_selected
                } else {
                    distance < previous_distance
                }
            }
        };
        if replace {
            best = Some((fi, distance));
        }
    };
    each_key_frame(show, slot, state.view_start_ms, state.view_end_ms, |fi| {
        consider(fi);
    });
    for &fi in &state.selected {
        consider(fi);
    }
    for &(fi, key_slot) in &state.selected_keys {
        if key_slot == slot {
            consider(fi);
        }
    }
    best
}

fn keyframe_bucket_ms(start_ms: f64, end_ms: f64, width: f32) -> u32 {
    let target = f64::from((width / 6.0).max(8.0));
    ((end_ms - start_ms) / target).max(1.0) as u32
}

fn each_key_frame(
    show: &Show,
    slot: usize,
    start_ms: f64,
    end_ms: f64,
    mut visit: impl FnMut(usize),
) {
    for (index, frame) in show.frames.iter().enumerate() {
        if !frame.is_key(slot) {
            continue;
        }
        let at = f64::from(frame.at_ms);
        if at < start_ms || at > end_ms {
            continue;
        }
        visit(index);
    }
}

fn for_each_keyframe(
    show: &Show,
    slot: usize,
    start_ms: f64,
    end_ms: f64,
    width: f32,
    mut visit: impl FnMut(usize),
) {
    let mut indexes = Vec::new();
    each_key_frame(show, slot, start_ms, end_ms, |index| indexes.push(index));
    let Some(&first) = indexes.first() else {
        return;
    };
    let last = indexes[indexes.len() - 1];
    visit(first);
    if last != first {
        visit(last);
    }
    let bucket = keyframe_bucket_ms(start_ms, end_ms, width).max(1);
    let mut seen = None;
    for index in indexes {
        let id = show.frames[index].at_ms / bucket;
        if seen == Some(id) {
            continue;
        }
        seen = Some(id);
        if index != first && index != last {
            visit(index);
        }
    }
}

fn key_grab_radius(lane: Rect, x: f32, y: f32, selected: bool) -> f32 {
    if selected {
        return 12.0;
    }
    let on_edge = (x - lane.left()).abs() <= 12.0
        || (x - lane.right()).abs() <= 12.0
        || (y - lane.top()).abs() <= 12.0
        || (y - lane.bottom()).abs() <= 12.0;
    if on_edge {
        16.0
    } else {
        7.0
    }
}

fn visible_selected(
    show: &Show,
    selected: &HashSet<usize>,
    start_ms: f64,
    end_ms: f64,
    bucket: u32,
) -> Vec<usize> {
    if selected.is_empty() {
        return Vec::new();
    }
    let bucket = bucket.max(1);
    let mut kept = Vec::new();
    let mut last = None;
    for (index, frame) in show.frames.iter().enumerate() {
        if !selected.contains(&index) {
            continue;
        }
        let at = f64::from(frame.at_ms);
        if at < start_ms || at > end_ms {
            continue;
        }
        let id = frame.at_ms / bucket;
        if last == Some(id) {
            continue;
        }
        last = Some(id);
        kept.push(index);
    }
    kept
}

fn draw_keyframes(
    painter: &Painter,
    lane_rect: Rect,
    show: &Show,
    _lane: usize,
    slot: usize,
    state: &NcodViewState,
    start_ms: f64,
    end_ms: f64,
    color: Color32,
) {
    let selected = selected_on_slot(state, slot);
    let paint = |fi: usize, is_selected: bool| {
        let Some(frame) = show.frames.get(fi) else {
            return;
        };
        let at = f64::from(frame.at_ms);
        if at < start_ms || at > end_ms {
            return;
        }
        if !is_selected && !frame.is_key(slot) {
            return;
        }
        let x = time_to_x(lane_rect, start_ms, end_ms, at);
        let y = y_of(lane_rect, frame.slots[slot]);
        let size = if is_selected || key_grab_radius(lane_rect, x, y, false) > 7.0 {
            5.5
        } else {
            3.5
        };
        painter.add(egui::Shape::convex_polygon(
            diamond_shape(pos2(x, y), size),
            if is_selected { theme::PLAYHEAD } else { color },
            egui::Stroke::new(1.0_f32, Color32::from_black_alpha(120)),
        ));
    };
    for_each_keyframe(show, slot, start_ms, end_ms, lane_rect.width(), |fi| {
        if !selected.contains(&fi) {
            paint(fi, false);
        }
    });
    let bucket = keyframe_bucket_ms(start_ms, end_ms, lane_rect.width());
    for fi in visible_selected(show, &selected, start_ms, end_ms, bucket) {
        paint(fi, true);
    }
}

fn is_envelope_point(show: &Show, frame: usize, slot: usize) -> bool {
    show.frames
        .get(frame)
        .is_some_and(|frame| frame.is_key(slot))
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
    let keys = show.channel_keys(slot);
    if keys.is_empty() {
        return;
    }
    let stroke = egui::Stroke::new(1.4_f32, color);
    if keys.len() == 1 {
        let y = y_of(rect, keys[0].1);
        painter.hline(rect.x_range(), y, stroke);
        return;
    }
    let columns = rect.width().max(1.0) as usize;
    if keys.len() <= columns {
        let points = keys
            .iter()
            .map(|(at_ms, level)| {
                pos2(
                    time_to_x(rect, start_ms, end_ms, f64::from(*at_ms)),
                    y_of(rect, *level),
                )
            })
            .collect::<Vec<_>>();
        painter.add(egui::Shape::line(points, stroke));
        return;
    }
    let span = (end_ms - start_ms).max(1.0);
    let mut upper = Vec::with_capacity(columns);
    let mut lower = Vec::with_capacity(columns);
    for column in 0..columns {
        let t0 = start_ms + (column as f64 / columns as f64) * span;
        let t1 = start_ms + ((column + 1) as f64 / columns as f64) * span;
        let (lo, hi) = key_column(&keys, t0 as u32, t1 as u32);
        let x = rect.left() + column as f32;
        upper.push(pos2(x, y_of(rect, hi)));
        lower.push(pos2(x, y_of(rect, lo)));
    }
    let band = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 70);
    for (hi, lo) in upper.iter().zip(&lower) {
        painter.line_segment([*hi, *lo], egui::Stroke::new(1.0_f32, band));
    }
    if upper.len() >= 2 {
        painter.add(egui::Shape::line(upper, stroke));
    }
}

fn y_of(rect: Rect, value: u8) -> f32 {
    rect.bottom() - 4.0 - (value as f32 / 255.0) * (rect.height() - 8.0)
}

fn key_column(keys: &[(u32, u8)], t0: u32, t1: u32) -> (u8, u8) {
    let left = crate::show::lerp_keys(keys, t0);
    let right = crate::show::lerp_keys(keys, t1);
    let mut lo = left.min(right);
    let mut hi = left.max(right);
    let from = keys.partition_point(|key| key.0 < t0);
    let to = keys.partition_point(|key| key.0 < t1);
    for (_, level) in &keys[from..to] {
        lo = lo.min(*level);
        hi = hi.max(*level);
    }
    (lo, hi)
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
        if (zoom - 1.0).abs() > 0.000_1 {
            let span = view_span(state, end_ms);
            let new_span = (span / f64::from(zoom)).clamp(20.0, end_ms);
            zoom_about_playhead(state, end_ms, new_span);
            changed = true;
        }
        if scroll.y != 0.0 {
            let span = view_span(state, end_ms);
            let new_span = (span * (-f64::from(scroll.y) / 200.0).exp()).clamp(20.0, end_ms);
            zoom_about_playhead(state, end_ms, new_span);
            changed = true;
        }
        if scroll.x != 0.0 {
            pan_pixels(state, end_ms, graph.width(), scroll.x);
            changed = true;
        }
        if scroll != egui::Vec2::ZERO {
            ui.ctx()
                .input_mut(|input| input.smooth_scroll_delta = egui::Vec2::ZERO);
        }
    } else if response.hovered() {
        let scroll_x = ui.input(|i| i.smooth_scroll_delta.x);
        if scroll_x != 0.0 {
            pan_pixels(state, end_ms, graph.width(), scroll_x);
            ui.ctx()
                .input_mut(|input| input.smooth_scroll_delta.x = 0.0);
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

fn zoom_about_playhead(state: &mut NcodViewState, end_ms: f64, new_span: f64) {
    let old_span = view_span(state, end_ms);
    let play = f64::from(state.play_ms);
    let u = (play - state.view_start_ms) / old_span;
    set_window(state, play - u * new_span, new_span, end_ms);
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

#[derive(Clone, Copy)]
struct ShapeMenu {
    pos: egui::Pos2,
}

enum PointShape {
    Random,
    Sine,
}

fn shape_menu_id() -> egui::Id {
    egui::Id::new("key-shape-menu")
}

fn shape_menu_open(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<ShapeMenu>(shape_menu_id()).is_some())
}

fn open_shape_menu(
    response: &Response,
    show: &Show,
    state: &NcodViewState,
    body: Rect,
    body_hit: Rect,
    rows: &TrackRows,
) {
    if state.gesture.is_some() || !response.secondary_clicked() {
        return;
    }
    let Some(pos) = response.interact_pointer_pos().or(response.hover_pos()) else {
        return;
    };
    if !body_hit.contains(pos) {
        return;
    }
    let Some((frame, slot, _)) = hit_pinned_key(show, state, body, rows, pos) else {
        return;
    };
    if !state.selected_keys.iter().any(|key| *key == (frame, slot)) {
        return;
    }
    response.ctx.data_mut(|data| {
        data.insert_temp(shape_menu_id(), ShapeMenu { pos });
    });
}

fn shape_menu(
    ctx: &egui::Context,
    show: &mut Show,
    state: &NcodViewState,
    undo: &mut UndoStack,
) -> bool {
    let id = shape_menu_id();
    let Some(menu) = ctx.data(|data| data.get_temp::<ShapeMenu>(id)) else {
        return false;
    };
    if state.selected_keys.is_empty() {
        ctx.data_mut(|data| data.remove::<ShapeMenu>(id));
        return false;
    }
    let mut dirty = false;
    let mut close = ctx.input(|input| input.key_pressed(egui::Key::Escape));
    let area = egui::Area::new(id.with("area"))
        .order(egui::Order::Foreground)
        .fixed_pos(menu.pos)
        .movable(false)
        .interactable(true)
        .fade_in(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(140.0);
                if ui
                    .add_sized(vec2(140.0, 22.0), egui::Button::new("Random"))
                    .on_hover_text("Give each selected point a random level.")
                    .clicked()
                {
                    dirty = apply_shape(show, state, undo, PointShape::Random);
                    close = true;
                }
                if ui
                    .add_sized(vec2(140.0, 22.0), egui::Button::new("Sine"))
                    .on_hover_text("One cycle across the selected points on each track.")
                    .clicked()
                {
                    dirty = apply_shape(show, state, undo, PointShape::Sine);
                    close = true;
                }
            });
        });
    let outside = ctx.input(|input| {
        input
            .pointer
            .any_pressed()
            .then(|| input.pointer.interact_pos())
    });
    if outside.is_some_and(|pos| pos.is_some_and(|pos| !area.response.rect.contains(pos))) {
        close = true;
    }
    if close {
        ctx.data_mut(|data| data.remove::<ShapeMenu>(id));
    }
    dirty
}

fn apply_shape(
    show: &mut Show,
    state: &NcodViewState,
    undo: &mut UndoStack,
    shape: PointShape,
) -> bool {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(1);
    let batch = shape_selected(show, &state.selected_keys, shape, seed);
    let changed = !batch.is_empty();
    undo.push(batch);
    changed
}

fn shape_selected(
    show: &mut Show,
    keys: &[(usize, usize)],
    shape: PointShape,
    seed: u64,
) -> Vec<super::undo::UndoOp> {
    let spans = sine_spans(show, keys);
    let mut seen = HashSet::new();
    let mut batch = Vec::new();
    for (index, &(frame, slot)) in keys.iter().enumerate() {
        if !seen.insert((frame, slot)) || slot >= 512 {
            continue;
        }
        let Some(current) = show.frames.get(frame) else {
            continue;
        };
        let before = current.slots[slot];
        let after = match shape {
            PointShape::Random => random_level(seed, index),
            PointShape::Sine => {
                let Some((start, end)) = spans.get(&slot).copied() else {
                    continue;
                };
                let Some(level) = sine_level(current.at_ms, start, end) else {
                    continue;
                };
                level
            }
        };
        if before == after {
            continue;
        }
        show.frames[frame].set_key(slot, after);
        batch.push(super::undo::UndoOp::Slot {
            frame,
            slot,
            before,
            after,
        });
    }
    batch
}

fn sine_spans(show: &Show, keys: &[(usize, usize)]) -> HashMap<usize, (u32, u32)> {
    let mut spans = HashMap::new();
    for &(frame, slot) in keys {
        let Some(at_ms) = show.frames.get(frame).map(|frame| frame.at_ms) else {
            continue;
        };
        spans
            .entry(slot)
            .and_modify(|(start, end): &mut (u32, u32)| {
                *start = (*start).min(at_ms);
                *end = (*end).max(at_ms);
            })
            .or_insert((at_ms, at_ms));
    }
    spans
}

fn sine_level(at_ms: u32, start_ms: u32, end_ms: u32) -> Option<u8> {
    let span = end_ms.saturating_sub(start_ms);
    if span == 0 {
        return None;
    }
    let u = f64::from(at_ms.saturating_sub(start_ms)) / f64::from(span);
    let mut wave = (u * std::f64::consts::TAU).cos();
    if wave.abs() < 1e-9 {
        wave = 0.0;
    } else if wave > 1.0 - 1e-9 {
        wave = 1.0;
    } else if wave < -1.0 + 1e-9 {
        wave = -1.0;
    }
    Some(((1.0 - wave) * 127.5).round().clamp(0.0, 255.0) as u8)
}

fn random_level(seed: u64, index: usize) -> u8 {
    let mut state = seed ^ (index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    (splitmix(&mut state) & 0xff) as u8
}

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::{
        random_level, shape_selected, shared_time_delta, sine_level, track_rows, visible_selected,
        PointShape, LANE_H,
    };
    use crate::gui::ncod_view::NcodViewState;
    use crate::show::{Frame, Show};
    use egui::{pos2, Rect};
    use std::collections::HashSet;

    #[test]
    fn dense_selection_keeps_one_point_per_bucket() {
        let show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: (0..1_000)
                .map(|at_ms| Frame::full(at_ms, [0; 512]))
                .collect(),
        };
        let selected: HashSet<usize> = (0..1_000).collect();
        let kept = visible_selected(&show, &selected, 0.0, 999.0, 50);
        assert_eq!(kept.len(), 20);
        assert_eq!(kept.first().copied(), Some(0));
        assert_eq!(kept.last().copied(), Some(950));
    }

    #[test]
    fn grouped_keyframes_move_together_without_crossing_neighbors() {
        let show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: [0, 100, 200, 300]
                .into_iter()
                .map(|at_ms| Frame::full(at_ms, [0; 512]))
                .collect(),
        };
        assert_eq!(shared_time_delta(&show, &[(1, 100), (2, 200)], 1_000), 99);
        assert_eq!(shared_time_delta(&show, &[(1, 100), (2, 200)], -1_000), -99);
    }

    #[test]
    fn track_rows_follow_each_channel_height() {
        let mut state = NcodViewState::default();
        state.pinned = vec![4, 8, 9];
        state.track_heights.insert(8, 200.0);
        let rows = track_rows(&state);
        assert_eq!(rows.heights, vec![LANE_H, 200.0, LANE_H]);
        assert_eq!(rows.tops, vec![0.0, LANE_H, LANE_H + 200.0]);
        assert_eq!(rows.index_at(LANE_H + 10.0, 0.0), Some(1));
        assert_eq!(rows.nearest(10_000.0, 0.0), Some(2));
        let header = Rect::from_min_max(pos2(0.0, 0.0), pos2(148.0, rows.total()));
        assert_eq!(rows.seam(header, pos2(40.0, LANE_H)), Some(0));
        assert_eq!(rows.seam(header, pos2(40.0, LANE_H + 40.0)), None);
        assert_eq!(rows.seam(header, pos2(200.0, LANE_H)), None);
    }

    #[test]
    fn sine_is_one_cycle_from_black_through_full_and_back() {
        assert_eq!(sine_level(0, 0, 1000), Some(0));
        assert_eq!(sine_level(250, 0, 1000), Some(128));
        assert_eq!(sine_level(500, 0, 1000), Some(255));
        assert_eq!(sine_level(750, 0, 1000), Some(128));
        assert_eq!(sine_level(1000, 0, 1000), Some(0));
        assert_eq!(sine_level(40, 40, 40), None);
    }

    #[test]
    fn sine_rewrites_each_track_on_its_own_span() {
        let mut slots = [10u8; 512];
        slots[1] = 10;
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: [0, 250, 500, 750, 1000]
                .into_iter()
                .map(|at_ms| Frame::full(at_ms, slots))
                .collect(),
        };
        let keys = vec![(0, 0), (2, 0), (4, 0), (0, 1), (2, 1)];
        let batch = shape_selected(&mut show, &keys, PointShape::Sine, 1);
        assert_eq!(show.frames[0].slots[0], 0);
        assert_eq!(show.frames[2].slots[0], 255);
        assert_eq!(show.frames[4].slots[0], 0);
        assert_eq!(show.frames[1].slots[0], 10);
        assert_eq!(show.frames[0].slots[1], 0);
        assert_eq!(show.frames[2].slots[1], 0);
        assert!(batch.len() >= 4);
        assert!(batch
            .iter()
            .all(|op| matches!(op, super::super::undo::UndoOp::Slot { before: 10, .. })));
    }

    #[test]
    fn random_levels_are_stable_and_replace_the_selection() {
        let first: Vec<u8> = (0..8).map(|index| random_level(42, index)).collect();
        assert_eq!(
            first,
            (0..8)
                .map(|index| random_level(42, index))
                .collect::<Vec<_>>()
        );
        assert!(first.iter().any(|level| *level != first[0]));
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: (0..8)
                .map(|index| Frame::full(index * 100, [3; 512]))
                .collect(),
        };
        let keys: Vec<(usize, usize)> = (0..8).map(|index| (index, 4)).collect();
        let batch = shape_selected(&mut show, &keys, PointShape::Random, 42);
        assert!(!batch.is_empty());
        for (index, level) in first.iter().enumerate() {
            assert_eq!(show.frames[index].slots[4], *level);
        }
        assert_eq!(show.frames[0].slots[0], 3);
    }
}
