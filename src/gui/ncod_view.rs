use crate::ncod::{Frame, Show};
use egui::{Color32, Context, RichText, Slider, Ui};

use super::playback::{PlaybackEvent, PlaybackHandle, PlaybackSettings};
use super::theme;
use super::timeline;
use super::timecode;
use super::transport;
use super::undo::{UndoOp, UndoStack};

const LARGE_SHOW_FRAMES: usize = 5_000;
const ASYNC_PARSE_BYTES: usize = 400_000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TimelineTool {
    Select,
    Edit,
    Brush,
}

pub enum Gesture {
    Value {
        frame: usize,
        slot: usize,
        before: u8,
    },
    Keyframe {
        frame: usize,
        slot: usize,
        before_value: u8,
        before_ms: u32,
        origin: egui::Pos2,
        move_time: Option<bool>,
    },
    Paint {
        slot: usize,
        originals: Vec<(usize, u8)>,
    },
    Range {
        start_ms: u32,
    },
}

pub struct NcodViewState {
    pub selected: Vec<usize>,
    pub playhead: usize,
    pub selected_track: usize,
    pub channel_start: u16,
    pub channel_count: u16,
    pub use_active_only: bool,
    pub pinned: Vec<u16>,
    pub view_start_ms: f64,
    pub view_end_ms: f64,
    pub follow_playhead: bool,
    pub frame_goto: u32,
    pub list_scroll_to: Option<usize>,
    pub tool: TimelineTool,
    pub snap_ms: u32,
    pub show_keyframes: bool,
    pub range_ms: Option<(u32, u32)>,
    pub gesture: Option<Gesture>,
    pub inspector_drag: Option<(usize, usize, u8)>,
    pub playback: PlaybackSettings,
    pub playing: bool,
    pub(crate) play_gen: u64,
    pub(crate) playback_rx: Option<std::sync::mpsc::Receiver<PlaybackEvent>>,
    pub(crate) playback_handle: Option<PlaybackHandle>,
}

impl NcodViewState {
    pub fn finish_gesture(&mut self, undo: &mut UndoStack, show: &Show) {
        let Some(gesture) = self.gesture.take() else {
            return;
        };
        match gesture {
            Gesture::Value { frame, slot, before } => {
                let after = show.frames.get(frame).map(|f| f.slots[slot]).unwrap_or(before);
                if before != after {
                    undo.push(vec![UndoOp::Slot {
                        frame,
                        slot,
                        before,
                        after,
                    }]);
                }
            }
            Gesture::Keyframe {
                frame,
                slot,
                before_value,
                before_ms,
                ..
            } => {
                let mut batch = Vec::new();
                if let Some(current) = show.frames.get(frame) {
                    if current.slots[slot] != before_value {
                        batch.push(UndoOp::Slot {
                            frame,
                            slot,
                            before: before_value,
                            after: current.slots[slot],
                        });
                    }
                    if current.at_ms != before_ms {
                        batch.push(UndoOp::Time {
                            frame,
                            before: before_ms,
                            after: current.at_ms,
                        });
                    }
                }
                undo.push(batch);
            }
            Gesture::Paint { slot, originals } => {
                let batch = originals
                    .into_iter()
                    .filter_map(|(frame, before)| {
                        let after = show.frames.get(frame)?.slots[slot];
                        (before != after).then_some(UndoOp::Slot {
                            frame,
                            slot,
                            before,
                            after,
                        })
                    })
                    .collect::<Vec<_>>();
                undo.push(batch);
            }
            Gesture::Range { .. } => {}
        }
    }

    pub fn finish_inspector_drag(&mut self, undo: &mut UndoStack, show: &Show) {
        let Some((frame, slot, before)) = self.inspector_drag.take() else {
            return;
        };
        let after = show
            .frames
            .get(frame)
            .map(|f| f.slots[slot])
            .unwrap_or(before);
        if before != after {
            undo.push(vec![UndoOp::Slot {
                frame,
                slot,
                before,
                after,
            }]);
        }
    }
}

impl Default for NcodViewState {
    fn default() -> Self {
        Self {
            selected: vec![0],
            playhead: 0,
            selected_track: 0,
            channel_start: 1,
            channel_count: 8,
            use_active_only: true,
            pinned: vec![1, 2, 3, 4],
            view_start_ms: 0.0,
            view_end_ms: 15_000.0,
            follow_playhead: true,
            frame_goto: 0,
            list_scroll_to: None,
            tool: TimelineTool::Select,
            snap_ms: 100,
            show_keyframes: true,
            range_ms: None,
            gesture: None,
            inspector_drag: None,
            playback: PlaybackSettings::default(),
            playing: false,
            play_gen: 0,
            playback_rx: None,
            playback_handle: None,
        }
    }
}

impl NcodViewState {
    pub fn reset_for_show(&mut self, show: &Show) {
        self.playhead = 0;
        self.frame_goto = 0;
        self.selected_track = 0;
        self.selected = if show.frames.is_empty() { vec![] } else { vec![0] };
        self.list_scroll_to = Some(0);
        let end_ms = f64::from(show.end_ms().max(1));
        self.view_start_ms = 0.0;
        self.view_end_ms = end_ms;
        if self.use_active_only {
            self.pinned = show.active_channels();
            if self.pinned.is_empty() {
                self.pinned = vec![1];
            }
        }
    }

    pub fn poll_playback(&mut self, frame_count: usize) {
        let events: Vec<PlaybackEvent> = match self.playback_rx.as_ref() {
            Some(rx) => rx.try_iter().collect(),
            None => return,
        };
        let mut finished = false;
        for event in events {
            match event {
                PlaybackEvent::Frame(index, gen) => {
                    if gen == self.play_gen {
                        self.playhead = index.min(frame_count.saturating_sub(1));
                        self.list_scroll_to = Some(self.playhead);
                    }
                }
                PlaybackEvent::Finished => finished = true,
                PlaybackEvent::Error(message) => {
                    eprintln!("playback: {message}");
                    finished = true;
                }
                PlaybackEvent::Started => {}
            }
        }
        if finished {
            self.playing = false;
            self.playback_handle = None;
            self.playback_rx = None;
        }
    }

    pub fn stop_playback(&mut self) {
        if let Some(handle) = self.playback_handle.take() {
            handle.stop();
        }
        self.playing = false;
        self.playback_rx = None;
    }

    pub fn place_playhead(&mut self, index: usize) {
        self.playhead = index;
        self.list_scroll_to = Some(index);
        if self.playing {
            if let Some(handle) = &self.playback_handle {
                self.play_gen = handle.seek(index);
            }
        }
    }

    pub fn handle_shortcuts(
        &mut self,
        ctx: &Context,
        show: &mut Show,
        undo: &mut UndoStack,
    ) -> bool {
        if ctx.wants_keyboard_input() {
            return false;
        }
        let mut toggle_play = false;
        let mut do_undo = false;
        let mut do_redo = false;
        let mut nudge = 0i16;
        ctx.input(|input| {
            if input.modifiers.command {
                if input.key_pressed(egui::Key::Z) {
                    if input.modifiers.shift {
                        do_redo = true;
                    } else {
                        do_undo = true;
                    }
                }
                if input.key_pressed(egui::Key::Y) {
                    do_redo = true;
                }
            }
            if input.key_pressed(egui::Key::V) {
                self.tool = TimelineTool::Select;
            }
            if input.key_pressed(egui::Key::K) {
                self.tool = TimelineTool::Edit;
            }
            if input.key_pressed(egui::Key::B) {
                self.tool = TimelineTool::Brush;
            }
            if input.key_pressed(egui::Key::OpenBracket) {
                let ms = show.frames.get(self.playhead).map(|f| f.at_ms).unwrap_or(0);
                let end = self.range_ms.map(|(_, end)| end).unwrap_or(ms.saturating_add(1_000));
                self.range_ms = Some((ms.min(end), ms.max(end)));
            }
            if input.key_pressed(egui::Key::CloseBracket) {
                let ms = show.frames.get(self.playhead).map(|f| f.at_ms).unwrap_or(0);
                let start = self.range_ms.map(|(start, _)| start).unwrap_or(0);
                self.range_ms = Some((start.min(ms), start.max(ms)));
            }
            if input.modifiers.alt && input.key_pressed(egui::Key::ArrowUp) {
                nudge = if input.modifiers.shift { 10 } else { 1 };
            }
            if input.modifiers.alt && input.key_pressed(egui::Key::ArrowDown) {
                nudge = -if input.modifiers.shift { 10 } else { 1 };
            }
            if !input.modifiers.alt && input.key_pressed(egui::Key::Space) {
                toggle_play = true;
            }
            if !input.modifiers.alt && input.key_pressed(egui::Key::ArrowLeft) {
                self.place_playhead(self.playhead.saturating_sub(1));
                self.selected = vec![self.playhead];
            }
            if !input.modifiers.alt && input.key_pressed(egui::Key::ArrowRight) {
                let last = show.frames.len().saturating_sub(1);
                self.place_playhead((self.playhead + 1).min(last));
                self.selected = vec![self.playhead];
            }
            if input.key_pressed(egui::Key::Home) {
                self.place_playhead(0);
                self.selected = vec![0];
            }
            if input.key_pressed(egui::Key::End) {
                let last = show.frames.len().saturating_sub(1);
                self.place_playhead(last);
                self.selected = vec![last];
            }
        });
        if toggle_play {
            if self.playing {
                self.stop_playback();
            } else {
                transport::toggle_play(show, self);
            }
        }
        if nudge != 0 {
            return nudge_playhead(show, self, undo, nudge);
        }
        if do_undo {
            return undo.undo(show);
        }
        if do_redo {
            return undo.redo(show);
        }
        false
    }

    pub fn tool_hint(&self) -> &'static str {
        match self.tool {
            TimelineTool::Select => "Drag to scrub. Shift-drag sets a range. [ ] marks in/out",
            TimelineTool::Edit => "Keyframe: drag up/down for value, sideways for time",
            TimelineTool::Brush => "Brush: drag on a lane to paint DMX. Alt+arrows nudge",
        }
    }
}

fn nudge_playhead(show: &mut Show, state: &NcodViewState, undo: &mut UndoStack, delta: i16) -> bool {
    let channel = state.pinned.get(state.selected_track).copied().unwrap_or(0);
    if channel == 0 || channel > 512 {
        return false;
    }
    let slot = usize::from(channel - 1);
    let Some(frame) = show.frames.get_mut(state.playhead) else {
        return false;
    };
    let before = frame.slots[slot];
    let after = (i16::from(before) + delta).clamp(0, 255) as u8;
    if before == after {
        return false;
    }
    frame.slots[slot] = after;
    undo.push(vec![UndoOp::Slot {
        frame: state.playhead,
        slot,
        before,
        after,
    }]);
    true
}

pub fn layout(ctx: &Context, show: &mut Show, undo: &mut UndoStack, state: &mut NcodViewState) -> bool {
    let mut dirty = false;
    state.poll_playback(show.frames.len());
    if state.handle_shortcuts(ctx, show, undo) {
        dirty = true;
    }

    egui::TopBottomPanel::top("transport")
        .frame(egui::Frame::default().fill(theme::BG_PANEL).inner_margin(8.0))
        .show(ctx, |ui| {
            dirty |= transport::bar(ui, show, state, undo);
        });

    egui::SidePanel::left("channels")
        .default_width(188.0)
        .resizable(true)
        .frame(egui::Frame::default().fill(theme::BG_PANEL).inner_margin(10.0))
        .show(ctx, |ui| {
            dirty |= channels_panel(ui, show, state, undo);
        });

    egui::SidePanel::right("inspector")
        .default_width(260.0)
        .resizable(true)
        .frame(egui::Frame::default().fill(theme::BG_PANEL).inner_margin(10.0))
        .show(ctx, |ui| {
            dirty |= inspector_panel(ui, show, state, undo);
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(theme::BG_DEEP).inner_margin(6.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Timeline")
                        .strong()
                        .color(Color32::from_gray(210)),
                );
                ui.label(theme::label_muted(state.tool_hint()));
            });
            dirty |= timeline::ui(ui, show, state, undo);
        });

    dirty
}

pub const ASYNC_PARSE_THRESHOLD: usize = ASYNC_PARSE_BYTES;
pub const ASYNC_SAVE_FRAMES: usize = LARGE_SHOW_FRAMES;

fn channels_panel(
    ui: &mut Ui,
    show: &mut Show,
    state: &mut NcodViewState,
    undo: &mut UndoStack,
) -> bool {
    let mut dirty = false;
    ui.heading("Universe");
    ui.label(theme::label_muted("Click a cell to show that DMX channel"));
    if ui.button("Show channels that move").clicked() {
        state.pinned = show.active_channels();
        if state.pinned.is_empty() {
            state.pinned = vec![1];
        }
        state.selected_track = 0;
    }
    ui.horizontal(|ui| {
        ui.label("Ch");
        ui.add(egui::DragValue::new(&mut state.channel_start).range(1..=512));
        if ui.button("Show").clicked() {
            show_channel(state, state.channel_start);
        }
    });
    universe_grid(ui, show, state);

    ui.separator();
    ui.label(theme::label_muted("Frames"));
    ui.horizontal(|ui| {
        if ui.button("Add").clicked() {
            let at = show.frames.get(state.playhead).map(|f| f.at_ms).unwrap_or(0);
            let frame = Frame {
                at_ms: at.saturating_add(100),
                slots: [0; 512],
            };
            let index = show.frames.len();
            show.frames.push(frame.clone());
            undo.push(vec![UndoOp::Insert { index, frame }]);
            state.selected = vec![index];
            dirty = true;
        }
        if ui.button("Dup").clicked() && !state.selected.is_empty() {
            let index = state.selected[0];
            if let Some(frame) = show.frames.get(index).cloned() {
                let mut copy = frame;
                copy.at_ms = copy.at_ms.saturating_add(50);
                let at = show.frames.len();
                show.frames.push(copy.clone());
                undo.push(vec![UndoOp::Insert { index: at, frame: copy }]);
                state.selected = vec![at];
                dirty = true;
            }
        }
        if ui.button("Del").clicked() && show.frames.len() > 1 {
            let index = state.selected.first().copied().unwrap_or(state.playhead);
            if index < show.frames.len() {
                let frame = show.frames[index].clone();
                show.frames.remove(index);
                undo.push(vec![UndoOp::Remove { index, frame }]);
                state.place_playhead(state.playhead.min(show.frames.len().saturating_sub(1)));
                state.selected = vec![state.playhead];
                dirty = true;
            }
        }
    });

    dirty
}

fn show_channel(state: &mut NcodViewState, channel: u16) {
    if let Some(index) = state.pinned.iter().position(|pinned| *pinned == channel) {
        state.selected_track = index;
        return;
    }
    state.pinned.push(channel);
    state.selected_track = state.pinned.len() - 1;
}

fn universe_grid(ui: &mut Ui, show: &Show, state: &mut NcodViewState) {
    let frame = show.frames.get(state.playhead);
    let cell = 11.0;
    let columns = 16;
    let rows = 512 / columns;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(cell * columns as f32, cell * rows as f32),
        egui::Sense::click(),
    );
    let painter = ui.painter_at(rect);
    for channel in 1..=512u16 {
        let index = usize::from(channel - 1);
        let column = index % columns;
        let row = index / columns;
        let cell_rect = egui::Rect::from_min_size(
            egui::pos2(
                rect.left() + column as f32 * cell,
                rect.top() + row as f32 * cell,
            ),
            egui::vec2(cell - 1.0, cell - 1.0),
        );
        let value = frame.map(|frame| frame.slots[index]).unwrap_or(0);
        let level = 28 + value / 2;
        let mut color = egui::Color32::from_gray(level);
        if state.pinned.contains(&channel) {
            let track = state.pinned.iter().position(|pinned| *pinned == channel).unwrap_or(0);
            color = theme::track_color(track);
            if value < 20 {
                color = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 90);
            }
        }
        painter.rect_filled(cell_rect, 1.0, color);
    }
    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let column = ((pos.x - rect.left()) / cell).floor() as usize;
            let row = ((pos.y - rect.top()) / cell).floor() as usize;
            if column < columns && row < rows {
                let channel = (row * columns + column + 1) as u16;
                if ui.input(|input| input.modifiers.alt) {
                    state.pinned.retain(|pinned| *pinned != channel);
                    state.selected_track = state.selected_track.min(state.pinned.len().saturating_sub(1));
                } else {
                    show_channel(state, channel);
                }
            }
        }
    }
    ui.label(theme::label_muted("Alt-click hides a channel"));
}

fn inspector_panel(
    ui: &mut Ui,
    show: &mut Show,
    state: &mut NcodViewState,
    undo: &mut UndoStack,
) -> bool {
    let mut dirty = false;
    ui.heading("Inspector");
    let index = state.selected.first().copied().unwrap_or(state.playhead);
    let Some(frame) = show.frames.get_mut(index) else {
        ui.label("No frame selected");
        return false;
    };

    ui.label(format!("Frame {index}"));
    ui.horizontal(|ui| {
        ui.label("Time");
        let mut at = frame.at_ms;
        if ui
            .add(egui::DragValue::new(&mut at).speed(1.0))
            .changed()
        {
            frame.at_ms = at;
            dirty = true;
        }
        ui.monospace(timecode::format_ms(frame.at_ms));
    });

    let track = state.selected_track.min(state.pinned.len().saturating_sub(1));
    let channel = state.pinned.get(track).copied().unwrap_or(1);
    if channel > 0 && channel <= 512 {
        let slot = usize::from(channel - 1);
        ui.separator();
        ui.colored_label(
            theme::track_color(track),
            format!("Channel {channel}"),
        );
        let mut value = frame.slots[slot];
        let response = ui.add(
            Slider::new(&mut value, 0..=255)
                .text("DMX")
                .show_value(true),
        );
        if response.drag_started() {
            state.inspector_drag = Some((index, slot, frame.slots[slot]));
        }
        if response.changed() || response.dragged() {
            frame.slots[slot] = value;
            dirty = true;
        }
        if response.drag_stopped() {
            state.finish_inspector_drag(undo, show);
        }
        ui.label(theme::label_muted("0 is off, 255 is full"));
    }

    dirty
}