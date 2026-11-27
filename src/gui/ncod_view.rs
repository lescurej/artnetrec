use crate::curve::{ShowOutput, TrackCurves, TransferLaw};
use crate::show::Show;
use egui::{Color32, Context, RichText, Slider, Ui};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::playback::{
    Live, PlaybackEvent, PlaybackHandle, PlaybackSettings, SoloMask, TrackLevels,
};
use super::theme;
use super::timecode;
use super::timeline;
use super::transport;
use super::undo::{UndoOp, UndoStack};

const LARGE_SHOW_FRAMES: usize = 5_000;
const ASYNC_PARSE_BYTES: usize = 400_000;
const MCP_FLASH: Duration = Duration::from_millis(600);

pub const COPY_SHORTCUT: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::C);
pub const PASTE_SHORTCUT: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::V);
pub const DUPLICATE_SHORTCUT: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::D);

#[derive(Clone)]
struct CopiedKey {
    offset_ms: u32,
    slot: usize,
    level: u8,
}

struct KeyCopy {
    origin_ms: u32,
    keys: Vec<CopiedKey>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MarqueeMode {
    Replace,
    Add,
    Toggle,
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
        originals: Vec<(usize, usize, u8)>,
        seen: HashSet<(usize, usize)>,
        cursor: Option<(usize, usize, u8)>,
        origin: egui::Pos2,
        stroking: bool,
    },
    Range {
        start_ms: u32,
    },
    Marquee {
        origin: egui::Pos2,
        current: egui::Pos2,
        mode: MarqueeMode,
        base: Vec<(usize, usize)>,
    },
    Keys {
        origin: egui::Pos2,
        points: Vec<(usize, usize, u8, u32)>,
        move_time: Option<bool>,
        scale: Option<(usize, usize)>,
    },
    Scrub,
}

#[derive(Clone)]
pub struct LaneResize {
    pub origin_y: f32,
    pub above: usize,
    pub tracks: Vec<(u16, f32)>,
}

pub struct NcodViewState {
    pub selected: Vec<usize>,
    pub selected_keys: Vec<(usize, usize)>,
    key_clipboard: Vec<CopiedKey>,
    pub playhead: usize,
    pub play_ms: u32,
    play_clock: Option<(Instant, u32)>,
    pub selected_tracks: Vec<usize>,
    pub channel_start: u16,
    pub channel_count: u16,
    pub use_active_only: bool,
    pub pinned: Vec<u16>,
    pub track_heights: HashMap<u16, f32>,
    pub lane_resize: Option<LaneResize>,
    pub view_start_ms: f64,
    pub view_end_ms: f64,
    pub follow_playhead: bool,
    pub frame_goto: u32,
    pub list_scroll_to: Option<usize>,
    pub snap_ms: u32,
    pub show_keyframes: bool,
    pub range_ms: Option<(u32, u32)>,
    pub gesture: Option<Gesture>,
    pub inspector_drag: Option<(usize, Vec<(usize, u8)>)>,
    pub playback: PlaybackSettings,
    pub master: u8,
    pub(crate) master_level: Arc<AtomicU8>,
    pub solo: Arc<SoloMask>,
    pub levels: Arc<TrackLevels>,
    pub curves: Arc<TrackCurves>,
    pub curve_editor: Option<u16>,
    pub playing: bool,
    pub(crate) play_gen: u64,
    pub(crate) playback_rx: Option<std::sync::mpsc::Receiver<PlaybackEvent>>,
    pub(crate) playback_handle: Option<PlaybackHandle>,
    pub(crate) playback_frames:
        Option<std::sync::Arc<std::sync::RwLock<super::playback::SharedShow>>>,
    pub(crate) live: Option<Live>,
    output_tracks: Vec<Vec<(u32, u8)>>,
    output_at_ms: Option<u32>,
    output_levels: [u8; 512],
    pub mcp_touched: HashSet<u16>,
    mcp_until: Option<Instant>,
}

impl NcodViewState {
    pub fn finish_gesture(&mut self, undo: &mut UndoStack, show: &Show) {
        let Some(gesture) = self.gesture.take() else {
            return;
        };
        match gesture {
            Gesture::Value {
                frame,
                slot,
                before,
            } => {
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
            Gesture::Paint { originals, .. } => {
                let batch = originals
                    .into_iter()
                    .filter_map(|(frame, slot, before)| {
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
            Gesture::Range { .. } | Gesture::Marquee { .. } | Gesture::Scrub => {}
            Gesture::Keys { points, .. } => {
                let mut batch = Vec::new();
                let mut timed = HashSet::new();
                for (frame, slot, value, at_ms) in points {
                    let Some(current) = show.frames.get(frame) else {
                        continue;
                    };
                    if current.slots[slot] != value {
                        batch.push(UndoOp::Slot {
                            frame,
                            slot,
                            before: value,
                            after: current.slots[slot],
                        });
                    }
                    if timed.insert(frame) && current.at_ms != at_ms {
                        batch.push(UndoOp::Time {
                            frame,
                            before: at_ms,
                            after: current.at_ms,
                        });
                    }
                }
                undo.push(batch);
            }
        }
    }

    pub fn finish_inspector_drag(&mut self, undo: &mut UndoStack, show: &Show) {
        let Some((frame, before)) = self.inspector_drag.take() else {
            return;
        };
        let Some(current) = show.frames.get(frame) else {
            return;
        };
        let batch = before
            .into_iter()
            .filter_map(|(slot, was)| {
                let after = current.slots[slot];
                (was != after).then_some(UndoOp::Slot {
                    frame,
                    slot,
                    before: was,
                    after,
                })
            })
            .collect::<Vec<_>>();
        undo.push(batch);
    }

    pub fn primary_track(&self) -> usize {
        let fallback = self.pinned.len().saturating_sub(1);
        self.selected_tracks
            .last()
            .copied()
            .unwrap_or(0)
            .min(fallback)
    }

    pub fn track_is_selected(&self, lane: usize) -> bool {
        self.selected_tracks.contains(&lane)
    }

    pub fn selected_channels(&self) -> Vec<u16> {
        let mut channels = Vec::new();
        for lane in &self.selected_tracks {
            let Some(channel) = self.pinned.get(*lane).copied() else {
                continue;
            };
            if (1..=512).contains(&channel) && !channels.contains(&channel) {
                channels.push(channel);
            }
        }
        channels
    }

    pub fn set_track_level(&self, channel: u16, level: u8) {
        let selected = self.selected_channels();
        if selected.len() > 1 && selected.contains(&channel) {
            for channel in selected {
                self.levels.set(channel, level);
            }
        } else {
            self.levels.set(channel, level);
        }
    }

    pub fn set_transfer(&self, channel: u16, law: TransferLaw) {
        let selected = self.selected_channels();
        let targets = if selected.len() > 1 && selected.contains(&channel) {
            selected
        } else {
            vec![channel]
        };
        for channel in targets {
            let _ = self.curves.set(channel, law.clone());
        }
    }

    pub fn set_min_on(&self, channel: u16, min_on: u8) {
        let selected = self.selected_channels();
        let targets = if selected.len() > 1 && selected.contains(&channel) {
            selected
        } else {
            vec![channel]
        };
        for channel in targets {
            let _ = self.curves.set_min_on(channel, min_on);
        }
    }

    pub fn solo_channel(&self, channel: u16) {
        let selected = self.selected_channels();
        if selected.len() > 1 && selected.contains(&channel) {
            let soloed = self.solo.channels();
            let same = soloed.len() == selected.len()
                && selected.iter().all(|channel| soloed.contains(channel));
            if same {
                self.solo.clear();
            } else {
                self.solo.set_channels(&selected);
            }
        } else {
            self.solo.toggle(channel);
        }
    }

    pub fn selected_slots(&self) -> Vec<usize> {
        let mut slots = Vec::new();
        for lane in &self.selected_tracks {
            let Some(channel) = self.pinned.get(*lane).copied() else {
                continue;
            };
            if (1..=512).contains(&channel) {
                let slot = usize::from(channel - 1);
                if !slots.contains(&slot) {
                    slots.push(slot);
                }
            }
        }
        slots
    }

    pub fn focus_track(&mut self, lane: usize) {
        if self.track_is_selected(lane) {
            if let Some(index) = self.selected_tracks.iter().position(|track| *track == lane) {
                self.selected_tracks.remove(index);
            }
            self.selected_tracks.push(lane);
        } else {
            self.select_track(lane);
        }
    }

    pub fn select_track(&mut self, lane: usize) {
        if self.pinned.is_empty() {
            self.selected_tracks.clear();
            return;
        }
        let lane = lane.min(self.pinned.len() - 1);
        self.selected_tracks = vec![lane];
    }

    pub fn toggle_track(&mut self, lane: usize) {
        if self.pinned.is_empty() {
            return;
        }
        let lane = lane.min(self.pinned.len() - 1);
        if let Some(index) = self.selected_tracks.iter().position(|track| *track == lane) {
            if self.selected_tracks.len() > 1 {
                self.selected_tracks.remove(index);
            }
        } else {
            self.selected_tracks.push(lane);
        }
    }

    pub fn select_track_range(&mut self, lane: usize) {
        if self.pinned.is_empty() {
            return;
        }
        let lane = lane.min(self.pinned.len() - 1);
        let anchor = self.primary_track().min(self.pinned.len() - 1);
        let (start, end) = if anchor <= lane {
            (anchor, lane)
        } else {
            (lane, anchor)
        };
        let mut tracks = (start..=end)
            .filter(|index| *index != lane)
            .collect::<Vec<_>>();
        tracks.push(lane);
        self.selected_tracks = tracks;
    }

    pub fn hide_channel(&mut self, channel: u16) {
        let Some(index) = self.pinned.iter().position(|pinned| *pinned == channel) else {
            return;
        };
        self.pinned.remove(index);
        self.selected_tracks = self
            .selected_tracks
            .iter()
            .filter_map(|lane| {
                if *lane == index {
                    None
                } else if *lane > index {
                    Some(*lane - 1)
                } else {
                    Some(*lane)
                }
            })
            .collect();
        self.ensure_track_selection();
    }

    pub fn ensure_track_selection(&mut self) {
        let count = self.pinned.len();
        let mut kept = Vec::new();
        for lane in self.selected_tracks.drain(..) {
            if lane < count && !kept.contains(&lane) {
                kept.push(lane);
            }
        }
        self.selected_tracks = if kept.is_empty() && count > 0 {
            vec![0]
        } else {
            kept
        };
    }

    pub fn track_height(&self, channel: u16) -> f32 {
        self.track_heights
            .get(&channel)
            .copied()
            .unwrap_or(timeline::LANE_H)
            .clamp(timeline::MIN_LANE_H, timeline::MAX_LANE_H)
    }

    pub fn begin_track_resize(&mut self, lane: usize, origin_y: f32) {
        let Some(&channel) = self.pinned.get(lane) else {
            return;
        };
        let grouped = self.track_is_selected(lane) && self.selected_tracks.len() > 1;
        let channels = if grouped {
            self.selected_channels()
        } else {
            vec![channel]
        };
        let above = if grouped {
            self.selected_tracks
                .iter()
                .filter(|index| **index < lane)
                .count()
        } else {
            0
        };
        self.lane_resize = Some(LaneResize {
            origin_y,
            above,
            tracks: channels
                .into_iter()
                .map(|channel| (channel, self.track_height(channel)))
                .collect(),
        });
    }

    pub fn drag_track_resize(&mut self, y: f32) {
        let Some(resize) = self.lane_resize.clone() else {
            return;
        };
        let delta = (y - resize.origin_y) / (resize.above as f32 + 1.0);
        for (channel, start) in resize.tracks {
            self.track_heights.insert(
                channel,
                (start + delta).clamp(timeline::MIN_LANE_H, timeline::MAX_LANE_H),
            );
        }
    }

    pub fn end_track_resize(&mut self, reset: bool) {
        let Some(resize) = self.lane_resize.take() else {
            return;
        };
        if reset {
            for (channel, _) in resize.tracks {
                self.track_heights.remove(&channel);
            }
        }
    }
}

impl Default for NcodViewState {
    fn default() -> Self {
        Self {
            selected: vec![0],
            selected_keys: Vec::new(),
            key_clipboard: Vec::new(),
            playhead: 0,
            play_ms: 0,
            play_clock: None,
            selected_tracks: vec![0],
            channel_start: 1,
            channel_count: 8,
            use_active_only: true,
            pinned: vec![1, 2, 3, 4],
            track_heights: HashMap::new(),
            lane_resize: None,
            view_start_ms: 0.0,
            view_end_ms: 15_000.0,
            follow_playhead: true,
            frame_goto: 0,
            list_scroll_to: None,
            snap_ms: 100,
            show_keyframes: true,
            range_ms: None,
            gesture: None,
            inspector_drag: None,
            playback: PlaybackSettings::default(),
            master: 255,
            master_level: Arc::new(AtomicU8::new(255)),
            solo: Arc::new(SoloMask::new()),
            levels: Arc::new(TrackLevels::new()),
            curves: Arc::new(TrackCurves::new()),
            curve_editor: None,
            playing: false,
            play_gen: 0,
            playback_rx: None,
            playback_handle: None,
            playback_frames: None,
            live: None,
            output_tracks: Vec::new(),
            output_at_ms: None,
            output_levels: [0; 512],
            mcp_touched: HashSet::new(),
            mcp_until: None,
        }
    }
}

impl NcodViewState {
    pub fn reset_for_show(&mut self, show: &Show) {
        self.solo.clear();
        self.playhead = 0;
        self.play_ms = 0;
        self.play_clock = None;
        self.frame_goto = 0;
        self.selected_tracks = vec![0];
        self.selected = if show.frames.is_empty() {
            vec![]
        } else {
            vec![0]
        };
        self.selected_keys.clear();
        self.output_tracks.clear();
        self.output_at_ms = None;
        self.mcp_touched.clear();
        self.mcp_until = None;
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

    pub fn poll_playback(&mut self, show: &Show) {
        let events: Vec<PlaybackEvent> = match self.playback_rx.as_ref() {
            Some(rx) => rx.try_iter().collect(),
            None => return,
        };
        let frame_count = show.frames.len();
        let mut finished = false;
        for event in events {
            match event {
                PlaybackEvent::Frame(index, at_ms, gen) => {
                    if gen == self.play_gen {
                        self.playhead = index.min(frame_count.saturating_sub(1));
                        self.play_ms = at_ms.min(show.end_ms());
                        self.play_clock = Some((Instant::now(), self.play_ms));
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
            self.play_clock = None;
        }
    }

    pub fn stop_playback(&mut self) {
        if let Some(handle) = self.playback_handle.take() {
            handle.stop();
        }
        self.playing = false;
        self.playback_rx = None;
        self.playback_frames = None;
        self.play_clock = None;
    }

    pub fn mark_mcp_channels(&mut self, channels: impl IntoIterator<Item = u16>, replace: bool) {
        if replace {
            self.mcp_touched.clear();
        }
        for channel in channels {
            if !(1..=512).contains(&channel) {
                continue;
            }
            self.mcp_touched.insert(channel);
            if !self.pinned.contains(&channel) {
                self.pinned.push(channel);
            }
        }
        if !self.mcp_touched.is_empty() {
            self.mcp_until = Some(Instant::now() + MCP_FLASH);
        }
    }

    pub fn expire_mcp_highlight(&mut self) -> Option<Duration> {
        let until = self.mcp_until?;
        let now = Instant::now();
        if now >= until {
            self.mcp_touched.clear();
            self.mcp_until = None;
            return None;
        }
        Some(until.saturating_duration_since(now))
    }

    pub fn refresh_after_show_mutation(show: &Show, state: &mut NcodViewState) {
        let play_ms = state.play_ms.min(show.end_ms());
        state.output_tracks.clear();
        state.output_at_ms = None;
        if state.playing {
            state.push_playback_frames(show);
        }
        state.play_ms = play_ms;
        state.playhead = show.frame_index_at(play_ms);
        state.frame_goto = play_ms;
        if !show.frames.is_empty() {
            state.selected = vec![state.playhead];
        } else {
            state.selected = vec![];
        }
        state.list_scroll_to = Some(state.playhead);
        let end_ms = f64::from(show.end_ms().max(1));
        state.view_end_ms = state.view_end_ms.min(end_ms).max(20.0);
        if state.view_start_ms >= state.view_end_ms {
            state.view_start_ms = 0.0;
            state.view_end_ms = end_ms;
        }
    }

    pub fn apply_output(&mut self, output: &ShowOutput) -> Result<(), String> {
        self.master = output.master;
        self.master_level.store(output.master, Ordering::Relaxed);
        self.curves.restore(output)
    }

    pub fn output_snapshot(&self) -> ShowOutput {
        let mut output = self.curves.snapshot();
        output.master = self.master;
        output
    }

    pub fn reload_show(&mut self, show: &mut Show, incoming: Show, undo: &mut UndoStack) -> bool {
        show.frames = incoming.frames;
        let keep = !incoming.output.explicit;
        if !keep {
            show.output = incoming.output.clone();
            let _ = self.apply_output(&incoming.output);
        }
        Self::refresh_after_show_mutation(show, self);
        undo.clear();
        keep && (self.master != 255 || !self.curves.is_plain())
    }

    pub fn place_playhead(&mut self, show: &Show, index: usize) {
        let last = show.frames.len().saturating_sub(1);
        let index = if show.frames.is_empty() {
            0
        } else {
            index.min(last)
        };
        self.playhead = index;
        self.play_ms = show.frames.get(index).map(|frame| frame.at_ms).unwrap_or(0);
        self.list_scroll_to = Some(index);
        self.note_clock();
        if self.playing {
            if let Some(handle) = &self.playback_handle {
                self.play_gen = handle.seek(index);
            }
        }
    }

    pub fn place_time(&mut self, show: &Show, ms: u32) {
        self.play_ms = ms.min(show.end_ms());
        self.playhead = show.frame_index_at(self.play_ms);
        self.selected = vec![self.playhead];
        self.list_scroll_to = Some(self.playhead);
        self.note_clock();
        if self.playing {
            if let Some(handle) = &self.playback_handle {
                self.play_gen = handle.seek_time(self.play_ms);
            }
        }
    }

    pub(crate) fn arm_playback_clock(&mut self) {
        self.play_clock = Some((Instant::now(), self.play_ms));
    }

    fn note_clock(&mut self) {
        if self.playing {
            self.arm_playback_clock();
        }
    }

    fn advance_playback_clock(&mut self, show: &Show) {
        let Some((origin, origin_ms)) = self.play_clock else {
            return;
        };
        if !self.playing {
            self.play_clock = None;
            return;
        }
        let elapsed = u32::try_from(origin.elapsed().as_millis()).unwrap_or(u32::MAX);
        self.play_ms = origin_ms.saturating_add(elapsed).min(show.end_ms());
        self.playhead = show.frame_index_at(self.play_ms);
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
        let mut do_delete = false;
        let mut select_all = false;
        let mut do_copy = false;
        let mut do_paste = false;
        let mut do_duplicate = false;
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
                if input.key_pressed(egui::Key::A) {
                    select_all = true;
                }
                if !input.modifiers.shift && input.key_pressed(egui::Key::C) {
                    do_copy = true;
                }
                if !input.modifiers.shift && input.key_pressed(egui::Key::V) {
                    do_paste = true;
                }
                if !input.modifiers.shift && input.key_pressed(egui::Key::D) {
                    do_duplicate = true;
                }
            }
            if input.key_pressed(egui::Key::OpenBracket) {
                let ms = self.play_ms;
                let end = self
                    .range_ms
                    .map(|(_, end)| end)
                    .unwrap_or(ms.saturating_add(1_000));
                self.range_ms = Some((ms.min(end), ms.max(end)));
            }
            if input.key_pressed(egui::Key::CloseBracket) {
                let ms = self.play_ms;
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
            if input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace) {
                do_delete = true;
            }
            if !input.modifiers.alt && input.key_pressed(egui::Key::ArrowLeft) {
                self.place_playhead(show, self.playhead.saturating_sub(1));
                self.selected = vec![self.playhead];
            }
            if !input.modifiers.alt && input.key_pressed(egui::Key::ArrowRight) {
                let last = show.frames.len().saturating_sub(1);
                self.place_playhead(show, (self.playhead + 1).min(last));
                self.selected = vec![self.playhead];
            }
            if input.key_pressed(egui::Key::Home) {
                self.place_playhead(show, 0);
                self.selected = vec![0];
            }
            if input.key_pressed(egui::Key::End) {
                let last = show.frames.len().saturating_sub(1);
                self.place_playhead(show, last);
                self.selected = vec![last];
            }
        });
        if select_all {
            self.select_keyframes_on_selected_tracks(show);
        }
        if do_copy {
            self.copy_selected_keys(show);
        }
        if do_duplicate {
            return self.duplicate_selected_keys(show, undo);
        }
        if do_paste {
            return self.paste_keys(show, undo);
        }
        if toggle_play {
            if self.playing {
                self.stop_playback();
            } else {
                transport::toggle_play(show, self);
            }
        }
        if do_delete {
            return self.delete_selected_keys(show, undo);
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

    pub fn select_keyframes_on_selected_tracks(&mut self, show: &Show) {
        let slots = self.selected_slots();
        let mut keys = Vec::new();
        for (frame, item) in show.frames.iter().enumerate() {
            for &slot in &slots {
                if item.is_key(slot) {
                    keys.push((frame, slot));
                }
            }
        }
        self.selected_keys = keys;
    }

    pub fn can_paste_keys(&self) -> bool {
        !self.key_clipboard.is_empty()
    }

    pub fn copy_selected_keys(&mut self, show: &Show) -> bool {
        let Some(copied) = self.selection_copy(show) else {
            return false;
        };
        self.key_clipboard = copied.keys;
        true
    }

    pub fn paste_keys(&mut self, show: &mut Show, undo: &mut UndoStack) -> bool {
        let copied = self.key_clipboard.clone();
        self.place_copied(show, undo, &copied, self.play_ms)
    }

    pub fn duplicate_selected_keys(&mut self, show: &mut Show, undo: &mut UndoStack) -> bool {
        let Some(copied) = self.selection_copy(show) else {
            return false;
        };
        let shift = duplicate_shift(&copied.keys, self.snap_ms);
        let anchor = copied.origin_ms.saturating_add(shift);
        self.place_copied(show, undo, &copied.keys, anchor)
    }

    fn selection_copy(&self, show: &Show) -> Option<KeyCopy> {
        let mut seen = HashSet::new();
        let mut points = Vec::new();
        for &(frame, slot) in &self.selected_keys {
            let Some(current) = show.frames.get(frame) else {
                continue;
            };
            if slot >= 512 || !current.is_key(slot) || !seen.insert((frame, slot)) {
                continue;
            }
            points.push((current.at_ms, slot, current.slots[slot]));
        }
        if points.is_empty() {
            return None;
        }
        let origin_ms = points.iter().map(|point| point.0).min().unwrap_or(0);
        points.sort_by_key(|point| (point.0, point.1));
        let keys = points
            .into_iter()
            .map(|(at_ms, slot, level)| CopiedKey {
                offset_ms: at_ms.saturating_sub(origin_ms),
                slot,
                level,
            })
            .collect();
        Some(KeyCopy { origin_ms, keys })
    }

    fn place_copied(
        &mut self,
        show: &mut Show,
        undo: &mut UndoStack,
        copied: &[CopiedKey],
        anchor_ms: u32,
    ) -> bool {
        if self.gesture.is_some() || copied.is_empty() {
            return false;
        }
        let mut placed = std::collections::BTreeMap::<(u32, usize), u8>::new();
        for key in copied {
            if key.slot >= 512 {
                continue;
            }
            placed.insert(
                (anchor_ms.saturating_add(key.offset_ms), key.slot),
                key.level,
            );
        }
        if placed.is_empty() {
            return false;
        }
        let mut batch = Vec::new();
        let mut selection = Vec::new();
        let mut current_time = None;
        let mut index = 0;
        for ((at_ms, slot), level) in placed {
            if current_time != Some(at_ms) {
                index = ensure_frame(show, self, &mut batch, at_ms);
                current_time = Some(at_ms);
            }
            write_placed_key(show, &mut batch, index, slot, level);
            selection.push((index, slot));
        }
        self.selected_keys = selection;
        self.playhead = show.frame_index_at(self.play_ms);
        let changed = !batch.is_empty();
        undo.push(batch);
        changed
    }

    pub fn delete_selected_keys(&mut self, show: &mut Show, undo: &mut UndoStack) -> bool {
        if self.selected_keys.is_empty() {
            return false;
        }
        let mut by_slot: std::collections::HashMap<usize, HashSet<usize>> =
            std::collections::HashMap::new();
        for &(frame, slot) in &self.selected_keys {
            by_slot.entry(slot).or_default().insert(frame);
        }
        let mut batch = Vec::new();
        for (slot, frames) in &by_slot {
            let endpoints = channel_endpoints(show, *slot);
            for frame in frames {
                if endpoints.is_some_and(|(first, last)| *frame == first || *frame == last) {
                    continue;
                }
                let Some(current) = show.frames.get(*frame) else {
                    continue;
                };
                if !current.is_key(*slot) {
                    continue;
                }
                let level = current.slots[*slot];
                show.frames[*frame].clear_key(*slot);
                batch.push(UndoOp::ClearKey {
                    frame: *frame,
                    slot: *slot,
                    level,
                });
            }
        }
        self.selected_keys.clear();
        let changed = !batch.is_empty();
        undo.push(batch);
        changed
    }

    pub(crate) fn publish_output(&mut self, show: &Show, edited: bool) {
        if edited {
            self.output_tracks.clear();
            self.output_at_ms = None;
            if self.playing {
                self.push_playback_frames(show);
            }
        }
        let live = self.live.clone();
        if let Some(live) = &live {
            live.set_endpoint(&self.playback.target, self.playback.universe);
        }
        if self.playing {
            return;
        }
        if !self.fill_stopped_levels(show) {
            return;
        }
        if let Some(live) = &live {
            live.publish(&self.output_levels);
        }
    }

    fn push_playback_frames(&self, show: &Show) {
        let Some(frames) = &self.playback_frames else {
            return;
        };
        if let Ok(mut guard) = frames.write() {
            guard.revision = guard.revision.wrapping_add(1);
            guard.frames.clone_from(&show.frames);
        }
    }

    fn fill_stopped_levels(&mut self, show: &Show) -> bool {
        if self.output_tracks.is_empty() {
            self.output_tracks = crate::show::index_tracks(&show.frames);
            self.output_at_ms = None;
        }
        if self.output_at_ms == Some(self.play_ms) {
            return false;
        }
        self.output_levels = sample_tracks(&self.output_tracks, self.play_ms);
        self.output_at_ms = Some(self.play_ms);
        true
    }

    pub fn tool_hint() -> &'static str {
        "Drag up or down moves level. Shift+drag moves time. Shift-click a selected point scales the selection vertically. Shift adds. Hold ⌘ to brush: click places one point, drag paints. ⌘A selects every point. Delete removes points. ⌘C copies, ⌘V pastes at the playhead, ⌘D duplicates. Right-click a selected point for random or sine"
    }
}

fn sample_tracks(tracks: &[Vec<(u32, u8)>], at_ms: u32) -> [u8; 512] {
    let mut levels = [0u8; 512];
    for (slot, keys) in tracks.iter().enumerate().take(512) {
        levels[slot] = crate::show::lerp_keys(keys, at_ms);
    }
    levels
}

fn nudge_playhead(
    show: &mut Show,
    state: &mut NcodViewState,
    undo: &mut UndoStack,
    delta: i16,
) -> bool {
    let slots = state.selected_slots();
    if slots.is_empty() || show.frames.is_empty() {
        return false;
    }
    let at_ms = state.play_ms.min(show.end_ms());
    let targets: Vec<(usize, u8, u8)> = slots
        .into_iter()
        .filter_map(|slot| {
            let before = show.value_at(slot, at_ms);
            let after = (i16::from(before) + delta).clamp(0, 255) as u8;
            (before != after).then_some((slot, before, after))
        })
        .collect();
    if targets.is_empty() {
        return false;
    }
    let mut batch = Vec::new();
    let index = match show.frames.iter().position(|frame| frame.at_ms == at_ms) {
        Some(index) => index,
        None => {
            let index = show.frames.partition_point(|frame| frame.at_ms < at_ms);
            let frame = crate::show::Frame::blank(at_ms);
            show.frames.insert(index, frame.clone());
            shift_indexes_after_insert(state, index);
            batch.push(UndoOp::Insert { index, frame });
            index
        }
    };
    for (slot, before, after) in targets {
        let was_key = show.frames[index].is_key(slot);
        show.frames[index].set_key(slot, after);
        if was_key {
            batch.push(UndoOp::Slot {
                frame: index,
                slot,
                before,
                after,
            });
        } else {
            batch.push(UndoOp::AddKey {
                frame: index,
                slot,
                level: after,
            });
        }
    }
    state.playhead = index;
    state.play_ms = at_ms;
    undo.push(batch);
    true
}

fn duplicate_shift(keys: &[CopiedKey], snap_ms: u32) -> u32 {
    let max = keys.iter().map(|key| key.offset_ms).max().unwrap_or(0);
    let fallback = if snap_ms == 0 { 100 } else { snap_ms };
    if max == 0 {
        return fallback;
    }
    let mut offsets: Vec<u32> = keys.iter().map(|key| key.offset_ms).collect();
    offsets.sort_unstable();
    offsets.dedup();
    let step = offsets
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .min()
        .unwrap_or(fallback);
    max.saturating_add(step.max(1))
}

fn ensure_frame(
    show: &mut Show,
    state: &mut NcodViewState,
    batch: &mut Vec<UndoOp>,
    at_ms: u32,
) -> usize {
    if let Some(index) = show.frames.iter().position(|frame| frame.at_ms == at_ms) {
        return index;
    }
    let index = show.frames.partition_point(|frame| frame.at_ms < at_ms);
    let frame = crate::show::Frame::blank(at_ms);
    show.frames.insert(index, frame.clone());
    shift_indexes_after_insert(state, index);
    batch.push(UndoOp::Insert { index, frame });
    index
}

fn write_placed_key(
    show: &mut Show,
    batch: &mut Vec<UndoOp>,
    index: usize,
    slot: usize,
    level: u8,
) {
    let Some(frame) = show.frames.get_mut(index) else {
        return;
    };
    if frame.is_key(slot) {
        let before = frame.slots[slot];
        if before == level {
            return;
        }
        frame.set_key(slot, level);
        batch.push(UndoOp::Slot {
            frame: index,
            slot,
            before,
            after: level,
        });
    } else {
        frame.set_key(slot, level);
        batch.push(UndoOp::AddKey {
            frame: index,
            slot,
            level,
        });
    }
}

fn shift_indexes_after_insert(state: &mut NcodViewState, index: usize) {
    for (frame, _) in &mut state.selected_keys {
        if *frame >= index {
            *frame += 1;
        }
    }
    for frame in &mut state.selected {
        if *frame >= index {
            *frame += 1;
        }
    }
    if let Some((frame, _)) = &mut state.inspector_drag {
        if *frame >= index {
            *frame += 1;
        }
    }
}

fn channel_endpoints(show: &Show, slot: usize) -> Option<(usize, usize)> {
    let mut indexes = show
        .frames
        .iter()
        .enumerate()
        .filter(|(_, frame)| frame.is_key(slot))
        .map(|(index, _)| index);
    let first = indexes.next()?;
    Some((first, indexes.last().unwrap_or(first)))
}

pub fn layout(
    ctx: &Context,
    show: &mut Show,
    undo: &mut UndoStack,
    state: &mut NcodViewState,
) -> bool {
    let mut dirty = false;
    let master_before = state.master;
    let curves_before = state.curves.generation();
    state.poll_playback(show);
    state.advance_playback_clock(show);
    if state.handle_shortcuts(ctx, show, undo) {
        dirty = true;
    }

    egui::TopBottomPanel::top("transport")
        .frame(
            egui::Frame::default()
                .fill(theme::BG_PANEL)
                .inner_margin(8.0),
        )
        .show(ctx, |ui| {
            dirty |= transport::bar(ui, show, state, undo);
        });

    egui::SidePanel::left("channels")
        .default_width(188.0)
        .resizable(true)
        .frame(
            egui::Frame::default()
                .fill(theme::BG_PANEL)
                .inner_margin(10.0),
        )
        .show(ctx, |ui| {
            channels_panel(ui, show, state);
        });

    egui::SidePanel::right("inspector")
        .default_width(260.0)
        .resizable(true)
        .frame(
            egui::Frame::default()
                .fill(theme::BG_PANEL)
                .inner_margin(10.0),
        )
        .show(ctx, |ui| {
            dirty |= inspector_panel(ui, show, state, undo);
        });

    egui::CentralPanel::default()
        .frame(
            egui::Frame::default()
                .fill(theme::BG_DEEP)
                .inner_margin(6.0),
        )
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("Timeline")
                        .strong()
                        .color(Color32::from_gray(210)),
                );
                ui.add(egui::Label::new(theme::label_muted(NcodViewState::tool_hint())).truncate())
                    .on_hover_text(NcodViewState::tool_hint());
            });
            dirty |= timeline::ui(ui, show, state, undo);
        });

    state.master_level.store(state.master, Ordering::Relaxed);
    dirty |= state.master != master_before || state.curves.generation() != curves_before;
    state.publish_output(show, dirty);
    dirty
}

pub const ASYNC_PARSE_THRESHOLD: usize = ASYNC_PARSE_BYTES;
pub const ASYNC_SAVE_FRAMES: usize = LARGE_SHOW_FRAMES;

fn channels_panel(ui: &mut Ui, show: &Show, state: &mut NcodViewState) {
    ui.heading("Universe");
    ui.label(theme::label_muted(
        "Click a cell to show it. Shift-click solos it",
    ));
    if !state.solo.is_empty() {
        let names = state
            .solo
            .channels()
            .iter()
            .map(|channel| channel.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        ui.colored_label(theme::SOLO, format!("Solo {names}"));
        if ui.button("Clear solo").clicked() {
            state.solo.clear();
        }
    }
    if ui.button("Show channels that move").clicked() {
        state.pinned = show.active_channels();
        if state.pinned.is_empty() {
            state.pinned = vec![1];
        }
        state.selected_tracks = vec![0];
    }
    ui.horizontal(|ui| {
        ui.label("Ch");
        ui.add(egui::DragValue::new(&mut state.channel_start).range(1..=512));
        if ui.button("Show").clicked() {
            show_channel(state, state.channel_start, false);
        }
    });
    universe_grid(ui, show, state);
}

fn show_channel(state: &mut NcodViewState, channel: u16, extend: bool) {
    let (index, added) =
        if let Some(index) = state.pinned.iter().position(|pinned| *pinned == channel) {
            (index, false)
        } else {
            state.pinned.push(channel);
            (state.pinned.len() - 1, true)
        };
    if extend {
        if added || !state.track_is_selected(index) {
            state.selected_tracks.push(index);
        } else {
            state.toggle_track(index);
        }
    } else {
        state.select_track(index);
    }
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
        let track = state.pinned.iter().position(|pinned| *pinned == channel);
        let selected = track.is_some_and(|lane| state.track_is_selected(lane));
        let mcp = state.mcp_touched.contains(&channel);
        if mcp {
            color = theme::MCP_TRACK;
        } else if let Some(track) = track {
            color = theme::track_color(track);
            if value < 20 {
                color = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 90);
            }
        }
        let soloed = state.solo.contains(channel);
        if !state.solo.is_empty() && !soloed {
            color = egui::Color32::from_rgb(color.r() / 3, color.g() / 3, color.b() / 3);
        }
        painter.rect_filled(cell_rect, 1.0, color);
        if mcp {
            painter.rect_stroke(cell_rect, 1.0, egui::Stroke::new(1.5_f32, theme::MCP_TRACK));
        } else if selected {
            painter.rect_stroke(cell_rect, 1.0, egui::Stroke::new(1.5_f32, theme::ACCENT));
        }
        if soloed {
            painter.rect_stroke(
                if selected {
                    cell_rect.shrink(1.5)
                } else {
                    cell_rect
                },
                1.0,
                egui::Stroke::new(1.5_f32, theme::SOLO),
            );
        }
    }
    let response =
        if let Some(channel) = grid_channel_at(response.hover_pos(), rect, cell, columns, rows) {
            let label = format!("Channel {channel}");
            response.on_hover_ui_at_pointer(|ui| {
                ui.label(label);
            })
        } else {
            response
        };
    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            if let Some(channel) = grid_channel_at(Some(pos), rect, cell, columns, rows) {
                if ui.input(|input| input.modifiers.alt) {
                    state.hide_channel(channel);
                } else if ui.input(|input| input.modifiers.shift) {
                    state.solo_channel(channel);
                } else if ui.input(|input| input.modifiers.command) {
                    show_channel(state, channel, true);
                } else {
                    show_channel(state, channel, false);
                }
            }
        }
    }
    ui.label(theme::label_muted(
        "Alt-click hides. ⌘-click adds to the selection",
    ));
}

fn grid_channel_at(
    pos: Option<egui::Pos2>,
    rect: egui::Rect,
    cell: f32,
    columns: usize,
    rows: usize,
) -> Option<u16> {
    let pos = pos?;
    let column = ((pos.x - rect.left()) / cell).floor();
    let row = ((pos.y - rect.top()) / cell).floor();
    if column < 0.0 || row < 0.0 {
        return None;
    }
    let column = column as usize;
    let row = row as usize;
    if column >= columns || row >= rows {
        return None;
    }
    Some((row * columns + column + 1) as u16)
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
    if show.frames.get(index).is_none() {
        ui.label("No frame selected");
        return false;
    }

    ui.label(format!("Frame {index}"));
    let (min_ms, max_ms) = show.frame_time_bounds(index);
    ui.horizontal(|ui| {
        ui.label("Time");
        let mut at = show.frames[index].at_ms;
        if ui
            .add(
                egui::DragValue::new(&mut at)
                    .speed(1.0)
                    .range(min_ms..=max_ms)
                    .clamp_existing_to_range(false),
            )
            .changed()
        {
            let next = at.clamp(min_ms, max_ms);
            if next != show.frames[index].at_ms {
                show.frames[index].at_ms = next;
                dirty = true;
            }
        }
        ui.monospace(timecode::format_ms(show.frames[index].at_ms));
    });
    let frame = &mut show.frames[index];

    let slots = state.selected_slots();
    if let Some(&primary) = slots.last() {
        let track = state.primary_track();
        let channel = state.pinned.get(track).copied().unwrap_or(1);
        let track_color = if state.mcp_touched.contains(&channel) {
            theme::MCP_TRACK
        } else {
            theme::track_color(track)
        };
        ui.separator();
        let channels = state.selected_channels();
        let all_solo = channels.iter().all(|channel| state.solo.contains(*channel));
        ui.horizontal(|ui| {
            if slots.len() == 1 {
                ui.colored_label(track_color, format!("Channel {channel}"));
            } else {
                ui.label(format!("{} channels", slots.len()));
            }
            if ui.selectable_label(all_solo, "Solo").clicked() {
                state.solo_channel(channel);
            }
        });
        let mut value = frame.slots[primary];
        let response = ui.add(
            Slider::new(&mut value, 0..=255)
                .text("DMX")
                .show_value(true),
        );
        if response.drag_started() {
            let before = slots
                .iter()
                .map(|slot| (*slot, frame.slots[*slot]))
                .collect();
            state.inspector_drag = Some((index, before));
        }
        if response.changed() || response.dragged() {
            for slot in &slots {
                frame.set_key(*slot, value);
            }
            dirty = true;
        }
        if response.drag_stopped() {
            state.finish_inspector_drag(undo, show);
        }
        ui.label(theme::label_muted("0 is off, 255 is full"));
    }

    dirty
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::Frame;

    #[test]
    fn timeline_fits_between_side_panels() {
        let ctx = egui::Context::default();
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: vec![Frame::full(0, [0; 512]), Frame::full(600_000, [0; 512])],
        };
        let mut undo = UndoStack::default();
        let mut state = NcodViewState::default();
        state.reset_for_show(&show);
        state.pinned = vec![1, 2, 3];
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let mut plot = egui::Rect::NOTHING;
        let mut clip = egui::Rect::NOTHING;
        for _ in 0..3 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ctx| {
                    layout(ctx, &mut show, &mut undo, &mut state);
                    ctx.data(|data| {
                        plot = data.get_temp(egui::Id::new("timeline_plot")).unwrap();
                        clip = data.get_temp(egui::Id::new("timeline_clip")).unwrap();
                    });
                },
            );
        }
        assert!(plot.right() <= clip.right() + 0.5);
        assert!(clip.right() - plot.right() <= 24.0, "{plot:?} {clip:?}");
        assert!(plot.width() > 400.0, "{plot:?}");
        assert!(ctx.used_rect().right() <= screen.right() + 1.0);
    }

    #[test]
    fn solo_follows_every_selected_track() {
        let mut state = NcodViewState::default();
        state.pinned = vec![1, 2, 3, 4];
        state.selected_tracks = vec![0, 2, 3];
        state.solo.toggle(8);
        state.solo_channel(1);
        assert_eq!(state.solo.channels(), vec![1, 3, 4]);
        state.solo_channel(3);
        assert!(state.solo.is_empty());
        state.solo_channel(2);
        assert_eq!(state.solo.channels(), vec![2]);
    }

    #[test]
    fn track_level_follows_every_selected_track() {
        let mut state = NcodViewState::default();
        state.pinned = vec![1, 2, 3, 4];
        state.selected_tracks = vec![0, 2, 3];
        state.levels.set(1, 10);
        state.levels.set(3, 40);
        state.set_track_level(1, 80);
        assert_eq!(state.levels.get(1), 80);
        assert_eq!(state.levels.get(2), 255);
        assert_eq!(state.levels.get(3), 80);
        assert_eq!(state.levels.get(4), 80);
        state.set_track_level(2, 20);
        assert_eq!(state.levels.get(2), 20);
        assert_eq!(state.levels.get(1), 80);
    }

    #[test]
    fn transfer_follows_every_selected_track() {
        use crate::curve::TransferLaw;
        let mut state = NcodViewState::default();
        state.pinned = vec![1, 2, 3, 4];
        state.selected_tracks = vec![0, 2, 3];
        state.set_transfer(1, TransferLaw::Gamma { gamma: 2.0 });
        assert!(matches!(state.curves.law(1), TransferLaw::Gamma { .. }));
        assert!(state.curves.law(2).is_linear());
        assert!(matches!(state.curves.law(3), TransferLaw::Gamma { .. }));
        assert!(matches!(state.curves.law(4), TransferLaw::Gamma { .. }));
        state.set_transfer(2, TransferLaw::SCurve);
        assert!(matches!(state.curves.law(1), TransferLaw::Gamma { .. }));
        assert!(matches!(state.curves.law(2), TransferLaw::SCurve));
    }

    #[test]
    fn resize_moves_every_selected_track_and_double_click_resets() {
        let mut state = NcodViewState::default();
        state.pinned = vec![4, 8];
        state.track_heights.insert(8, 200.0);
        state.selected_tracks = vec![0, 1];
        state.begin_track_resize(0, 100.0);
        state.drag_track_resize(140.0);
        assert_eq!(state.track_height(4), timeline::LANE_H + 40.0);
        assert_eq!(state.track_height(8), 240.0);
        state.end_track_resize(true);
        assert_eq!(state.track_height(4), timeline::LANE_H);
        assert_eq!(state.track_height(8), timeline::LANE_H);
        state.selected_tracks = vec![0, 1];
        state.begin_track_resize(1, 0.0);
        state.drag_track_resize(40.0);
        assert_eq!(state.track_height(4), timeline::LANE_H + 20.0);
        assert_eq!(state.track_height(8), timeline::LANE_H + 20.0);
        state.end_track_resize(true);
        state.selected_tracks = vec![0];
        state.begin_track_resize(1, 0.0);
        state.drag_track_resize(30.0);
        assert_eq!(state.track_height(4), timeline::LANE_H);
        assert_eq!(state.track_height(8), timeline::LANE_H + 30.0);
    }

    #[test]
    fn select_all_takes_every_key_on_the_selected_tracks() {
        let show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: [0, 100, 200]
                .into_iter()
                .map(|at_ms| {
                    let mut frame = Frame::blank(at_ms);
                    frame.set_key(0, 10);
                    frame.set_key(2, 20);
                    if at_ms != 100 {
                        frame.set_key(1, 30);
                    }
                    frame
                })
                .collect(),
        };
        let mut state = NcodViewState::default();
        state.pinned = vec![1, 2, 3];
        state.selected_tracks = vec![0, 2];
        state.select_keyframes_on_selected_tracks(&show);
        assert_eq!(
            state.selected_keys,
            vec![(0, 0), (0, 2), (1, 0), (1, 2), (2, 0), (2, 2)]
        );
        state.selected_tracks = vec![1];
        state.select_keyframes_on_selected_tracks(&show);
        assert_eq!(state.selected_keys, vec![(0, 1), (2, 1)]);
    }

    #[test]
    fn delete_removes_middle_keys_and_keeps_the_ends() {
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: [0, 100, 200, 300]
                .into_iter()
                .map(|at_ms| {
                    let mut slots = [0u8; 512];
                    slots[0] = (at_ms / 100) as u8;
                    slots[1] = 7;
                    Frame::full(at_ms, slots)
                })
                .collect(),
        };
        let mut state = NcodViewState::default();
        let mut undo = UndoStack::default();
        state.selected_keys = vec![(0, 0), (1, 0), (2, 0), (3, 0)];
        assert!(state.delete_selected_keys(&mut show, &mut undo));
        assert!(show.frames[0].is_key(0));
        assert!(!show.frames[1].is_key(0));
        assert!(!show.frames[2].is_key(0));
        assert!(show.frames[3].is_key(0));
        assert_eq!(show.value_at(0, 100), 1);
        assert_eq!(show.value_at(0, 200), 2);
        assert!(show
            .frames
            .iter()
            .all(|frame| frame.is_key(1) && frame.slots[1] == 7));
        assert!(undo.undo(&mut show));
        assert!(show.frames[1].is_key(0));
        assert_eq!(show.frames[1].slots[0], 1);
        assert_eq!(show.frames[2].slots[0], 2);
    }

    #[test]
    fn manual_point_edit_drives_the_live_curve() {
        let mut parked = Frame::blank(500);
        parked.slots[0] = 200;
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: vec![
                {
                    let mut frame = Frame::blank(0);
                    frame.set_key(0, 255);
                    frame
                },
                parked,
                {
                    let mut frame = Frame::blank(1_000);
                    frame.set_key(0, 255);
                    frame
                },
            ],
        };
        let mut state = NcodViewState::default();
        state.play_ms = 500;
        state.playhead = 1;
        state.publish_output(&show, true);
        assert_eq!(state.output_levels[0], 255);
        assert_eq!(show.frames[1].slots[0], 200);
        state.play_ms = 0;
        state.publish_output(&show, false);
        assert_eq!(state.output_levels[0], 255);

        show.frames[0].set_key(0, 0);
        show.frames[2].set_key(0, 0);
        state.play_ms = 500;
        state.publish_output(&show, true);
        assert_eq!(state.output_levels[0], 0);
        assert_eq!(show.frames[1].slots[0], 200);

        state.playing = true;
        state.playback_frames = Some(Arc::new(std::sync::RwLock::new(
            crate::gui::playback::SharedShow {
                frames: Vec::new(),
                revision: 1,
            },
        )));
        state.publish_output(&show, true);
        let guard = state.playback_frames.as_ref().unwrap().read().unwrap();
        assert_eq!(guard.revision, 2);
        assert_eq!(guard.frames[0].slots[0], 0);
        assert_eq!(guard.frames[2].slots[0], 0);
        assert_eq!(crate::show::levels_at(&guard.frames, 500)[0], 0);
    }

    #[test]
    fn nudge_follows_the_curve_at_the_playhead() {
        let mut parked = Frame::blank(500);
        parked.slots[0] = 200;
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: vec![
                {
                    let mut frame = Frame::blank(0);
                    frame.set_key(0, 0);
                    frame
                },
                parked,
                {
                    let mut frame = Frame::blank(1_000);
                    frame.set_key(0, 100);
                    frame
                },
            ],
        };
        let mut state = NcodViewState::default();
        state.pinned = vec![1];
        state.selected_tracks = vec![0];
        state.play_ms = 500;
        state.playhead = 1;
        let mut undo = UndoStack::default();
        assert!(nudge_playhead(&mut show, &mut state, &mut undo, 1));
        assert!(show.frames[1].is_key(0));
        assert_eq!(show.frames[1].slots[0], 51);
        assert_eq!(show.value_at(0, 500), 51);
        assert!(undo.undo(&mut show));
        assert!(!show.frames[1].is_key(0));
        assert_eq!(show.value_at(0, 500), 50);

        state.play_ms = 250;
        state.selected_keys = vec![(2, 0)];
        assert!(nudge_playhead(&mut show, &mut state, &mut undo, 10));
        assert_eq!(show.frames[1].at_ms, 250);
        assert_eq!(show.value_at(0, 250), 35);
        assert_eq!(state.selected_keys, vec![(3, 0)]);
        assert_eq!(state.playhead, 1);
        assert!(undo.undo(&mut show));
        assert_eq!(show.frames.len(), 3);
        assert_eq!(show.value_at(0, 250), 25);

        state.play_ms = 0;
        state.playhead = 0;
        assert!(nudge_playhead(&mut show, &mut state, &mut undo, 1));
        assert_eq!(show.frames[0].slots[0], 1);
        assert!(show.frames[0].is_key(0));
        assert!(undo.undo(&mut show));
        assert_eq!(show.frames[0].slots[0], 0);
        assert!(show.frames[0].is_key(0));
    }

    fn keyed(at_ms: u32, points: &[(usize, u8)]) -> Frame {
        let mut frame = Frame::blank(at_ms);
        for &(slot, level) in points {
            frame.set_key(slot, level);
        }
        frame
    }

    #[test]
    fn paste_lands_the_first_key_on_the_playhead_and_keeps_spacing() {
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: vec![
                keyed(0, &[(0, 10), (1, 7)]),
                keyed(200, &[(0, 40)]),
                keyed(1_000, &[(0, 0), (1, 80)]),
            ],
        };
        let mut state = NcodViewState::default();
        let mut undo = UndoStack::default();
        state.selected_keys = vec![(0, 0), (1, 0)];
        state.play_ms = 1_000;
        state.playhead = 2;
        assert!(state.copy_selected_keys(&show));
        assert!(state.paste_keys(&mut show, &mut undo));
        assert_eq!(show.frames[2].at_ms, 1_000);
        assert_eq!(show.frames[2].slots[0], 10);
        assert_eq!(show.frames[2].slots[1], 80);
        assert_eq!(show.frames[3].at_ms, 1_200);
        assert_eq!(show.frames[3].slots[0], 40);
        assert!(!show.frames[3].is_key(1));
        assert_eq!(show.frames[0].slots[0], 10);
        assert_eq!(state.selected_keys, vec![(2, 0), (3, 0)]);
        assert_eq!(state.playhead, 2);
        assert!(undo.undo(&mut show));
        assert_eq!(show.frames.len(), 3);
        assert_eq!(show.frames[2].slots[0], 0);
        assert!(undo.redo(&mut show));
        assert_eq!(show.frames[3].slots[0], 40);
    }

    #[test]
    fn paste_overwrites_only_the_destination_channel() {
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: vec![keyed(0, &[(0, 99)]), keyed(500, &[(0, 1), (1, 20)])],
        };
        let mut state = NcodViewState::default();
        let mut undo = UndoStack::default();
        state.selected_keys = vec![(0, 0)];
        state.play_ms = 500;
        assert!(state.copy_selected_keys(&show));
        assert!(state.paste_keys(&mut show, &mut undo));
        assert_eq!(show.frames.len(), 2);
        assert_eq!(show.frames[1].slots[0], 99);
        assert_eq!(show.frames[1].slots[1], 20);
        assert!(undo.undo(&mut show));
        assert_eq!(show.frames[1].slots[0], 1);
    }

    #[test]
    fn duplicate_repeats_the_rhythm_and_selects_the_copy() {
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: vec![
                keyed(0, &[(0, 1)]),
                keyed(500, &[(0, 2)]),
                keyed(1_000, &[(0, 3)]),
            ],
        };
        let mut state = NcodViewState::default();
        state.snap_ms = 100;
        let mut undo = UndoStack::default();
        state.selected_keys = vec![(0, 0), (1, 0), (2, 0)];
        assert!(state.duplicate_selected_keys(&mut show, &mut undo));
        assert_eq!(
            show.channel_keys(0),
            vec![
                (0, 1),
                (500, 2),
                (1_000, 3),
                (1_500, 1),
                (2_000, 2),
                (2_500, 3),
            ]
        );
        assert_eq!(state.selected_keys, vec![(3, 0), (4, 0), (5, 0)]);
        assert!(state.duplicate_selected_keys(&mut show, &mut undo));
        assert_eq!(show.channel_keys(0).last().copied(), Some((4_000, 3)));
        assert!(undo.undo(&mut show));
        assert_eq!(show.channel_keys(0).last().copied(), Some((2_500, 3)));
        assert!(undo.undo(&mut show));
        assert_eq!(show.frames.len(), 3);
    }

    #[test]
    fn duplicate_single_key_steps_by_the_snap() {
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: vec![keyed(200, &[(0, 15), (2, 9)]), keyed(900, &[(0, 1)])],
        };
        let mut state = NcodViewState::default();
        state.snap_ms = 100;
        let mut undo = UndoStack::default();
        state.selected_keys = vec![(0, 0)];
        assert!(state.duplicate_selected_keys(&mut show, &mut undo));
        assert_eq!(show.frames[1].at_ms, 300);
        assert_eq!(show.frames[1].slots[0], 15);
        assert!(!show.frames[1].is_key(2));
        assert_eq!(show.frames[0].slots[2], 9);
        assert_eq!(state.selected_keys, vec![(1, 0)]);
        state.snap_ms = 0;
        assert!(state.duplicate_selected_keys(&mut show, &mut undo));
        assert_eq!(show.frames[2].at_ms, 400);
        assert_eq!(show.frames[2].slots[0], 15);
    }

    #[test]
    fn paste_without_a_copy_does_nothing() {
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: vec![keyed(0, &[(0, 4)])],
        };
        let mut state = NcodViewState::default();
        let mut undo = UndoStack::default();
        state.play_ms = 100;
        assert!(!state.paste_keys(&mut show, &mut undo));
        assert!(!state.copy_selected_keys(&show));
        assert_eq!(show.frames.len(), 1);
        assert!(!undo.can_undo());
    }
}
