mod cof_view;
mod curve_editor;
mod fader;
mod file_watch;
mod mcp_api;
mod mcp_help;
mod ncod_view;
mod playback;
mod theme;
mod timecode;
mod timeline;
mod transport;
mod undo;

use crate::{
    behavior::BehaviorCatalog,
    devices,
    mcp::{spawn as spawn_mcp_server, McpServerHandle},
    session::{self, ApiJob},
    show, showfile,
};
use cof_view::CofViewState;
use crossbeam_channel::Receiver;
use mcp_api::{handle_api, load_sidecar, save_sidecar_if_possible};
use ncod_view::{NcodViewState, ASYNC_PARSE_THRESHOLD, ASYNC_SAVE_FRAMES};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

enum CuecoreOpen {
    NotCuecore,
    Prompt {
        tracks: Vec<devices::cuecore2::xml::TrackChoice>,
    },
    Error(String),
}

fn cuecore_open(bytes: &[u8]) -> CuecoreOpen {
    if !devices::cuecore2::xml::is_file(bytes) {
        return CuecoreOpen::NotCuecore;
    }
    match devices::cuecore2::xml::list_tracks(bytes) {
        Ok(tracks) if !tracks.is_empty() => CuecoreOpen::Prompt { tracks },
        Ok(_) => CuecoreOpen::Error("backup has no track".into()),
        Err(err) => CuecoreOpen::Error(err),
    }
}

fn track_duration(frames: u32, fps: u32) -> String {
    let ms = u64::from(frames.saturating_sub(1)) * 1_000 / u64::from(fps.max(1));
    let tenths = (ms % 1_000) / 100;
    let seconds = ms / 1_000;
    let minutes = seconds / 60;
    let rest = seconds % 60;
    if minutes > 0 {
        format!("{minutes}:{rest:02}.{tenths}")
    } else {
        format!("{rest}.{tenths}s")
    }
}

fn is_jsonl(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
}

fn drop_curve_sidecar(path: &Path) {
    if showfile::format_for_path(path).is_some() {
        return;
    }
    let sidecar = crate::curve::sidecar_path(path);
    if sidecar.is_file() {
        let _ = std::fs::remove_file(sidecar);
    }
}

const APP_ID: &str = "DMX Recorder";
const SAVE_SHORTCUT: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::S);
const SAVE_AS_SHORTCUT: egui::KeyboardShortcut = egui::KeyboardShortcut::new(
    egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
    egui::Key::S,
);

enum IoJob {
    Save {
        path: PathBuf,
        adopt: bool,
        rx: mpsc::Receiver<Result<(), String>>,
    },
    Open {
        path: PathBuf,
        track: Option<u32>,
        rx: mpsc::Receiver<Result<show::Show, String>>,
    },
    ReloadShow {
        rx: mpsc::Receiver<Result<show::Show, String>>,
    },
}

struct CuecoreImport {
    path: PathBuf,
    bytes: Vec<u8>,
    tracks: Vec<devices::cuecore2::xml::TrackChoice>,
}

pub enum LoadedFile {
    Show {
        show: show::Show,
        view: NcodViewState,
        undo: undo::UndoStack,
    },
    Cof {
        config: devices::tr512::cof::Config,
        view: CofViewState,
    },
}

pub struct App {
    pub path: Option<PathBuf>,
    pub document: Option<LoadedFile>,
    pub dirty: bool,
    pub error: Option<String>,
    pub busy: Option<String>,
    pub behavior_catalog: BehaviorCatalog,
    pub mcp_port: u16,
    mcp_write_batch: bool,
    mcp_jobs: Receiver<ApiJob>,
    _mcp_server: Option<McpServerHandle>,
    mcp_help_open: bool,
    pending_open: bool,
    pending_save_as: bool,
    pending_export: Option<&'static devices::ShowFormat>,
    cuecore_import: Option<CuecoreImport>,
    cuecore_track: Option<u32>,
    io_job: Option<IoJob>,
    file_watch: Option<file_watch::Handle>,
    reload_debounce: Option<Instant>,
    reload_after_io: bool,
    ignore_external_reload_until: Option<Instant>,
    stream: Option<playback::Stream>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        let default_port = std::env::var("ARTNETREC_MCP_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(9474);
        let (mut facade, mcp_jobs) = session::pair();
        let ctx = cc.egui_ctx.clone();
        facade.set_wake(move || ctx.request_repaint());
        let (mcp_port, mcp_server) = match spawn_mcp_server(facade, default_port) {
            Ok((port, handle)) => (port, Some(handle)),
            Err(err) => {
                eprintln!("mcp: {err}");
                (default_port, None)
            }
        };
        let mut app = Self {
            path: None,
            document: None,
            dirty: false,
            error: None,
            busy: None,
            behavior_catalog: BehaviorCatalog::new(),
            mcp_port,
            mcp_write_batch: false,
            mcp_jobs,
            _mcp_server: mcp_server,
            mcp_help_open: false,
            pending_open: false,
            pending_save_as: false,
            pending_export: None,
            cuecore_import: None,
            cuecore_track: None,
            io_job: None,
            file_watch: None,
            reload_debounce: None,
            reload_after_io: false,
            ignore_external_reload_until: None,
            stream: match playback::Stream::start() {
                Ok(stream) => Some(stream),
                Err(err) => {
                    eprintln!("art-net: {err}");
                    None
                }
            },
        };
        if let Some(path) = read_last_opened() {
            app.open_path(path);
        }
        app
    }

    fn poll_mcp(&mut self) {
        self.mcp_write_batch = false;
        while let Ok(job) = self.mcp_jobs.try_recv() {
            let result = handle_api(self, job.request);
            let _ = job.respond.send(result);
        }
    }

    fn poll_mcp_flash(&mut self, ctx: &egui::Context) {
        let Some(LoadedFile::Show { view, .. }) = &mut self.document else {
            return;
        };
        if let Some(left) = view.expire_mcp_highlight() {
            ctx.request_repaint_after(left);
        }
    }

    fn adopt_path(&mut self, path: PathBuf) {
        write_last_opened(Some(&path));
        self.set_file_watch(&path);
        self.path = Some(path);
    }

    fn set_file_watch(&mut self, path: &PathBuf) {
        self.file_watch = file_watch::watch(path.clone()).ok();
        self.reload_debounce = None;
    }

    fn note_own_write(&mut self) {
        self.ignore_external_reload_until = Some(Instant::now() + Duration::from_secs(1));
        self.reload_debounce = None;
    }

    fn poll_file_reload(&mut self) {
        let Some(watch) = &self.file_watch else {
            return;
        };
        let loaded = watch.path().to_path_buf();
        let touched = watch
            .drain()
            .into_iter()
            .any(|path| file_watch::targets_file(&path, &loaded));
        if touched {
            self.reload_debounce = Some(Instant::now());
        }
        let Some(since) = self.reload_debounce else {
            return;
        };
        if since.elapsed() < file_watch::DEBOUNCE {
            return;
        }
        self.reload_debounce = None;
        self.reload_from_disk();
    }

    fn reload_from_disk(&mut self) {
        if self
            .ignore_external_reload_until
            .is_some_and(|until| Instant::now() < until)
        {
            return;
        }
        if self.document.is_none() || self.path.is_none() {
            return;
        }
        if self.io_job.is_some() {
            self.reload_after_io = true;
            return;
        }
        let path = self.path.clone().unwrap();
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.error = Some(format!("read {}: {err}", path.display()));
                return;
            }
        };
        let extension = path.extension().and_then(|ext| ext.to_str());
        if let Some(parsed) = devices::tr512::open_config(extension, &bytes) {
            let Some(LoadedFile::Cof { config, .. }) = &mut self.document else {
                return;
            };
            match parsed {
                Ok(next) => {
                    *config = next;
                    self.dirty = false;
                }
                Err(err) => self.error = Some(err),
            }
            return;
        }

        let cuecore_track = self
            .cuecore_track
            .filter(|_| devices::cuecore2::xml::is_file(&bytes));
        if cuecore_track.is_none() {
            self.cuecore_track = None;
        }
        if bytes.len() >= ASYNC_PARSE_THRESHOLD {
            let path_label = path.display().to_string();
            self.busy = Some(format!("Reloading {path_label}…"));
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let result = match cuecore_track {
                    Some(index) => devices::cuecore2::xml::parse_index(&bytes, index),
                    None => showfile::parse_show(&bytes),
                };
                let _ = tx.send(result);
            });
            self.io_job = Some(IoJob::ReloadShow { rx });
            return;
        }
        let parsed = match cuecore_track {
            Some(index) => devices::cuecore2::xml::parse_index(&bytes, index),
            None => showfile::parse_show(&bytes),
        };
        let Some(LoadedFile::Show { show, view, undo }) = &mut self.document else {
            return;
        };
        match parsed {
            Ok(next) => {
                self.dirty = view.reload_show(show, next, undo);
            }
            Err(err) => self.error = Some(err),
        }
    }

    pub fn open_path(&mut self, path: PathBuf) {
        if self.io_job.is_some() {
            self.error = Some("wait for the current file operation".into());
            return;
        }
        self.error = None;
        self.cuecore_import = None;
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.error = Some(format!("read {}: {err}", path.display()));
                return;
            }
        };
        let extension = path.extension().and_then(|ext| ext.to_str());
        if let Some(parsed) = devices::tr512::open_config(extension, &bytes) {
            match parsed {
                Ok(config) => {
                    self.cuecore_track = None;
                    self.adopt_path(path);
                    self.document = Some(LoadedFile::Cof {
                        config,
                        view: CofViewState::default(),
                    });
                    self.dirty = false;
                }
                Err(err) => self.error = Some(err),
            }
            return;
        }

        match cuecore_open(&bytes) {
            CuecoreOpen::Prompt { tracks } => {
                self.cuecore_import = Some(CuecoreImport {
                    path,
                    bytes,
                    tracks,
                });
                return;
            }
            CuecoreOpen::Error(err) => {
                self.error = Some(err);
                return;
            }
            CuecoreOpen::NotCuecore => {}
        }

        if bytes.len() >= ASYNC_PARSE_THRESHOLD {
            self.busy = Some(format!("Loading {}…", path.display()));
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let result = showfile::parse_show(&bytes);
                let _ = tx.send(result);
            });
            self.io_job = Some(IoJob::Open {
                path,
                track: None,
                rx,
            });
        } else {
            match showfile::parse_show(&bytes) {
                Ok(show) => {
                    self.cuecore_track = None;
                    self.finish_open_show(path, show);
                }
                Err(err) => self.error = Some(err),
            }
        }
    }

    fn show_track_picker(&mut self, ctx: &egui::Context) {
        let Some(import) = &self.cuecore_import else {
            return;
        };
        let mut chosen = None;
        let mut open = true;
        let mut cancel = false;
        let path_label = import.path.display().to_string();
        let tracks = import.tracks.clone();
        egui::Window::new("Import CueCore2 track")
            .collapsible(false)
            .resizable(true)
            .default_width(460.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .order(egui::Order::Foreground)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Choose the recording to import.");
                ui.label(egui::RichText::new(path_label).small().weak());
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        for track in &tracks {
                            let name = if track.label.is_empty() {
                                format!("Track {}", track.index)
                            } else {
                                track.label.clone()
                            };
                            let button = egui::Button::new(format!(
                                "{name}    {} frames · {} · {} fps",
                                track.frames,
                                track_duration(track.frames, track.sample_rate),
                                track.sample_rate,
                            ))
                            .min_size(egui::vec2(ui.available_width(), 0.0));
                            if ui.add(button).clicked() {
                                chosen = Some(track.index);
                            }
                        }
                    });
                ui.add_space(6.0);
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        if cancel || !open {
            self.cuecore_import = None;
            return;
        }
        let Some(index) = chosen else {
            return;
        };
        let Some(import) = self.cuecore_import.take() else {
            return;
        };
        match devices::cuecore2::xml::parse_index(&import.bytes, index) {
            Ok(show) => {
                self.cuecore_track = Some(index);
                self.finish_open_show(import.path, show);
            }
            Err(err) => {
                self.cuecore_import = Some(import);
                self.error = Some(err);
            }
        }
    }

    fn finish_open_show(&mut self, path: PathBuf, show: show::Show) {
        let mut view = NcodViewState::default();
        view.reset_for_show(&show);
        let mut migrated = false;
        if show.output.explicit {
            if let Err(err) = view.apply_output(&show.output) {
                self.error = Some(err);
            }
        } else if is_jsonl(&path) && crate::curve::sidecar_path(&path).is_file() {
            match view.curves.load_sidecar(&path) {
                Ok(()) => migrated = true,
                Err(err) => self.error = Some(err),
            }
        }
        if let Some(stream) = &self.stream {
            let live = stream.live();
            live.bind(
                std::sync::Arc::clone(&view.master_level),
                std::sync::Arc::clone(&view.solo),
                std::sync::Arc::clone(&view.levels),
                std::sync::Arc::clone(&view.curves),
            );
            view.live = Some(live);
            view.publish_output(&show, true);
        }
        self.behavior_catalog = load_sidecar(&path);
        self.adopt_path(path);
        self.document = Some(LoadedFile::Show {
            show,
            view,
            undo: undo::UndoStack::default(),
        });
        self.dirty = migrated;
        self.busy = None;
    }

    fn save_to(&mut self, path: PathBuf) -> bool {
        if self.io_job.is_some() {
            self.error = Some("wait for the current file operation".into());
            return false;
        }
        self.error = None;
        let show = if let Some(LoadedFile::Show { show, view, .. }) = &mut self.document {
            show.output = view.output_snapshot();
            Some(show.clone())
        } else {
            None
        };
        if let Some(show) = show {
            return self.write_show(path, show, true);
        }
        let Some(LoadedFile::Cof { config, .. }) = &self.document else {
            return false;
        };
        match devices::tr512::cof::write(&path, config) {
            Ok(()) => {
                self.adopt_path(path);
                self.dirty = false;
                self.note_own_write();
                true
            }
            Err(err) => {
                self.error = Some(err);
                false
            }
        }
    }

    fn write_show(&mut self, path: PathBuf, show: show::Show, adopt: bool) -> bool {
        if show.frames.len() >= ASYNC_SAVE_FRAMES {
            self.busy = Some(format!("Saving {}…", path.display()));
            let save_path = path.clone();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let result = showfile::write_show(&save_path, &show);
                let _ = tx.send(result);
            });
            self.io_job = Some(IoJob::Save { path, adopt, rx });
            true
        } else {
            match showfile::write_show(&path, &show) {
                Ok(()) => {
                    drop_curve_sidecar(&path);
                    if adopt {
                        self.adopt_path(path);
                        self.dirty = false;
                    }
                    let _ = save_sidecar_if_possible(self);
                    self.note_own_write();
                    true
                }
                Err(err) => {
                    self.error = Some(err);
                    false
                }
            }
        }
    }

    fn poll_io(&mut self) {
        let Some(job) = self.io_job.take() else {
            return;
        };
        match job {
            IoJob::Save { path, adopt, rx } => {
                if let Ok(result) = rx.try_recv() {
                    self.busy = None;
                    match result {
                        Ok(()) => {
                            drop_curve_sidecar(&path);
                            if adopt {
                                self.adopt_path(path);
                                self.dirty = false;
                            }
                            let _ = save_sidecar_if_possible(self);
                            self.note_own_write();
                        }
                        Err(err) => self.error = Some(err),
                    }
                    self.maybe_reload_after_io();
                } else {
                    self.io_job = Some(IoJob::Save { path, adopt, rx });
                }
            }
            IoJob::Open { path, track, rx } => {
                if let Ok(result) = rx.try_recv() {
                    self.busy = None;
                    match result {
                        Ok(show) => {
                            self.cuecore_track = track;
                            self.finish_open_show(path, show);
                        }
                        Err(err) => self.error = Some(err),
                    }
                    self.maybe_reload_after_io();
                } else {
                    self.io_job = Some(IoJob::Open { path, track, rx });
                }
            }
            IoJob::ReloadShow { rx } => {
                if let Ok(result) = rx.try_recv() {
                    self.busy = None;
                    match result {
                        Ok(next) => {
                            if let Some(LoadedFile::Show { show, view, undo }) = &mut self.document
                            {
                                self.dirty = view.reload_show(show, next, undo);
                            }
                        }
                        Err(err) => self.error = Some(err),
                    }
                    self.maybe_reload_after_io();
                } else {
                    self.io_job = Some(IoJob::ReloadShow { rx });
                }
            }
        }
    }

    fn maybe_reload_after_io(&mut self) {
        if self.reload_after_io {
            self.reload_after_io = false;
            self.reload_from_disk();
        }
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn run_open_dialog(&mut self) {
        if self.io_job.is_some() {
            return;
        }
        let mut dialog = rfd::FileDialog::new().add_filter("DMX Recorder", &["jsonl", "JSONL"]);
        let filters = devices::open_filters();
        for (name, extensions) in &filters {
            dialog = dialog.add_filter(*name, extensions);
        }
        if let Some(path) = dialog.pick_file() {
            self.open_path(path);
        }
    }

    fn run_save_as_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new();
        dialog = match &self.document {
            Some(LoadedFile::Cof { .. }) => dialog.add_filter(
                "Showtec TR-512 configuration",
                devices::tr512::DEVICE.config_extensions,
            ),
            Some(LoadedFile::Show { .. }) | None => {
                let mut dialog = dialog.add_filter("DMX Recorder", &["jsonl", "JSONL"]);
                for format in devices::show_formats() {
                    dialog = dialog.add_filter(format.label, format.extensions);
                }
                dialog
            }
        };
        if let Some(path) = dialog.save_file() {
            let path = if matches!(self.document, Some(LoadedFile::Cof { .. })) {
                path
            } else {
                showfile::with_extension(path)
            };
            let _ = self.save_to(path);
        }
    }

    fn run_export_dialog(&mut self, format: &'static devices::ShowFormat) {
        if self.io_job.is_some() {
            return;
        }
        let Some(LoadedFile::Show { show, .. }) = &self.document else {
            return;
        };
        let show = show.clone();
        if let Some(path) = rfd::FileDialog::new()
            .add_filter(format.label, format.extensions)
            .save_file()
        {
            let path = match path.extension().and_then(|ext| ext.to_str()) {
                Some(ext)
                    if format
                        .extensions
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(ext)) =>
                {
                    path
                }
                _ => path.with_extension(format.extension),
            };
            let _ = self.write_show(path, show, false);
        }
    }

    fn save(&mut self) {
        if self.document.is_none() || self.io_job.is_some() {
            return;
        }
        if let Some(path) = self.path.clone() {
            self.save_to(path);
        } else {
            self.pending_save_as = true;
        }
    }

    fn save_as(&mut self) {
        if self.document.is_some() && self.io_job.is_none() {
            self.pending_save_as = true;
        }
    }

    fn handle_file_shortcuts(&mut self, ctx: &egui::Context) {
        let (save_as, save) = ctx.input_mut(|input| {
            (
                take_chord(input, SAVE_AS_SHORTCUT),
                take_chord(input, SAVE_SHORTCUT),
            )
        });
        if save_as {
            self.save_as();
        } else if save {
            self.save();
        }
    }

    fn menu_bar(&mut self, ctx: &egui::Context) {
        let io_busy = self.io_job.is_some();
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui
                        .add_enabled(!io_busy, egui::Button::new("Open…"))
                        .clicked()
                    {
                        self.pending_open = true;
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            self.document.is_some() && !io_busy,
                            egui::Button::new("Save")
                                .shortcut_text(ctx.format_shortcut(&SAVE_SHORTCUT)),
                        )
                        .clicked()
                    {
                        self.save();
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            self.document.is_some() && !io_busy,
                            egui::Button::new("Save As…")
                                .shortcut_text(ctx.format_shortcut(&SAVE_AS_SHORTCUT)),
                        )
                        .clicked()
                    {
                        self.save_as();
                        ui.close_menu();
                    }
                    let can_export =
                        matches!(self.document, Some(LoadedFile::Show { .. })) && !io_busy;
                    for format in devices::show_formats() {
                        if ui
                            .add_enabled(
                                can_export,
                                egui::Button::new(format!("Export {}…", format.label)),
                            )
                            .clicked()
                        {
                            self.pending_export = Some(format);
                            ui.close_menu();
                        }
                    }
                    if ui
                        .add_enabled(!io_busy, egui::Button::new("Close"))
                        .clicked()
                    {
                        if let Some(LoadedFile::Show { view, .. }) = &mut self.document {
                            view.stop_playback();
                        }
                        self.document = None;
                        self.path = None;
                        self.behavior_catalog = BehaviorCatalog::new();
                        write_last_opened(None);
                        self.dirty = false;
                        self.file_watch = None;
                        self.reload_debounce = None;
                        ui.close_menu();
                    }
                });
                let edit_dirty = std::cell::Cell::new(false);
                ui.menu_button("Edit", |ui| {
                    if let Some(LoadedFile::Show { show, view, undo }) = &mut self.document {
                        if ui
                            .add_enabled(undo.can_undo(), egui::Button::new("Undo"))
                            .clicked()
                        {
                            if undo.undo(show) {
                                edit_dirty.set(true);
                            }
                            ui.close_menu();
                        }
                        if ui
                            .add_enabled(undo.can_redo(), egui::Button::new("Redo"))
                            .clicked()
                        {
                            if undo.redo(show) {
                                edit_dirty.set(true);
                            }
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(
                                !view.selected_keys.is_empty(),
                                egui::Button::new("Copy Keyframes")
                                    .shortcut_text(ctx.format_shortcut(&ncod_view::COPY_SHORTCUT)),
                            )
                            .clicked()
                        {
                            view.copy_selected_keys(show);
                            ui.close_menu();
                        }
                        if ui
                            .add_enabled(
                                view.can_paste_keys(),
                                egui::Button::new("Paste Keyframes")
                                    .shortcut_text(ctx.format_shortcut(&ncod_view::PASTE_SHORTCUT)),
                            )
                            .clicked()
                        {
                            if view.paste_keys(show, undo) {
                                edit_dirty.set(true);
                            }
                            ui.close_menu();
                        }
                        if ui
                            .add_enabled(
                                !view.selected_keys.is_empty(),
                                egui::Button::new("Duplicate Keyframes").shortcut_text(
                                    ctx.format_shortcut(&ncod_view::DUPLICATE_SHORTCUT),
                                ),
                            )
                            .clicked()
                        {
                            if view.duplicate_selected_keys(show, undo) {
                                edit_dirty.set(true);
                            }
                            ui.close_menu();
                        }
                    }
                });
                if edit_dirty.get() {
                    self.dirty = true;
                }
                ui.menu_button("Help", |ui| {
                    if ui.button("MCP connection…").clicked() {
                        self.mcp_help_open = true;
                        ui.close_menu();
                    }
                });
            });
        });
    }
}

fn take_chord(input: &mut egui::InputState, shortcut: egui::KeyboardShortcut) -> bool {
    let mut pressed = false;
    input.events.retain(|event| match event {
        egui::Event::Key {
            key,
            modifiers,
            pressed: true,
            repeat,
            ..
        } if *key == shortcut.logical_key && modifiers.matches_logically(shortcut.modifiers) => {
            pressed |= !repeat;
            false
        }
        _ => true,
    });
    pressed
}

fn last_opened_file() -> Option<PathBuf> {
    eframe::storage_dir(APP_ID).map(|dir| dir.join("last_opened_path"))
}

fn write_last_opened(path: Option<&Path>) {
    let Some(file) = last_opened_file() else {
        return;
    };
    match path {
        Some(path) => {
            if let Some(parent) = file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(file, path.to_string_lossy().as_bytes());
        }
        None => {
            let _ = std::fs::remove_file(file);
        }
    }
}

fn read_last_opened() -> Option<PathBuf> {
    let file = last_opened_file()?;
    let text = std::fs::read_to_string(&file).ok()?;
    let path = PathBuf::from(text.trim());
    if path.is_file() {
        Some(path)
    } else {
        let _ = std::fs::remove_file(file);
        None
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_mcp();
        self.poll_mcp_flash(ctx);
        self.poll_io();
        self.poll_file_reload();
        if self.io_job.is_some() || self.reload_debounce.is_some() || !self.mcp_jobs.is_empty() {
            ctx.request_repaint();
        }
        if self._mcp_server.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }

        self.handle_file_shortcuts(ctx);

        if self.pending_open {
            self.pending_open = false;
            self.run_open_dialog();
        }
        if self.pending_save_as {
            self.pending_save_as = false;
            self.run_save_as_dialog();
        }
        if let Some(format) = self.pending_export.take() {
            self.run_export_dialog(format);
        }

        self.menu_bar(ctx);
        self.show_track_picker(ctx);
        mcp_help::window(
            ctx,
            &mut self.mcp_help_open,
            self.mcp_port,
            self._mcp_server.is_some(),
        );

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let path = self
                    .path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(no file)".into());
                ui.label(path);
                if self.dirty {
                    ui.label(egui::RichText::new("modified").color(egui::Color32::YELLOW));
                }
                if let Some(busy) = &self.busy {
                    ui.label(egui::RichText::new(busy).color(egui::Color32::LIGHT_BLUE));
                }
                if let Some(err) = &self.error {
                    ui.label(egui::RichText::new(err).color(egui::Color32::RED));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button(if self._mcp_server.is_some() {
                            format!("MCP :{}", self.mcp_port)
                        } else {
                            "MCP off".into()
                        })
                        .clicked()
                    {
                        self.mcp_help_open = true;
                    }
                });
            });
        });

        if self.busy.is_some() && self.document.is_none() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.spinner();
                if let Some(busy) = &self.busy {
                    ui.label(busy);
                }
            });
            return;
        }

        if self.document.is_none() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.heading("DMX Recorder");
                    ui.label("Edit and play a show for a standalone DMX player.");
                    ui.add_space(16.0);
                    for known in devices::DEVICES
                        .iter()
                        .filter(|device| device.status == devices::Status::Supported)
                    {
                        let code = known
                            .code
                            .map(|code| format!(" {code}"))
                            .unwrap_or_default();
                        ui.label(format!("{}{code}", known.name));
                        ui.label(egui::RichText::new(known.detail).small());
                        ui.add_space(8.0);
                    }
                    ui.label("Open a .jsonl show, or import a recorder that has a codec.");
                    if ui
                        .add_enabled(self.io_job.is_none(), egui::Button::new("Open…"))
                        .clicked()
                    {
                        self.pending_open = true;
                    }
                });
            });
            return;
        }

        let became_dirty = match self.document.as_mut().unwrap() {
            LoadedFile::Cof { config, view } => {
                let mut dirty = false;
                egui::CentralPanel::default().show(ctx, |ui| {
                    let before = config.clone();
                    cof_view::ui(ui, config, view);
                    dirty = config != &before;
                });
                dirty
            }
            LoadedFile::Show { show, view, undo } => ncod_view::layout(ctx, show, undo, view),
        };
        if became_dirty {
            self.mark_dirty();
        }

        if let Some(LoadedFile::Show { view, .. }) = &self.document {
            if view.playing {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }
    }
}
