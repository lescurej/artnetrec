mod cof_view;
mod ncod_view;
mod playback;
mod theme;
mod timecode;
mod timeline;
mod transport;
mod undo;

use cof_view::CofViewState;
use ncod_view::{NcodViewState, ASYNC_PARSE_THRESHOLD, ASYNC_SAVE_FRAMES};
use crate::{devices, show, showfile};
use std::path::PathBuf;
use std::sync::mpsc;

enum IoJob {
    Save {
        path: PathBuf,
        adopt: bool,
        rx: mpsc::Receiver<Result<(), String>>,
    },
    Open {
        path: PathBuf,
        rx: mpsc::Receiver<Result<show::Show, String>>,
    },
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
    pending_open: bool,
    pending_save_as: bool,
    pending_export: Option<&'static devices::ShowFormat>,
    io_job: Option<IoJob>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        Self {
            path: None,
            document: None,
            dirty: false,
            error: None,
            busy: None,
            pending_open: false,
            pending_save_as: false,
            pending_export: None,
            io_job: None,
        }
    }

    pub fn open_path(&mut self, path: PathBuf) {
        if self.io_job.is_some() {
            self.error = Some("wait for the current file operation".into());
            return;
        }
        self.error = None;
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
                    self.path = Some(path);
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

        if bytes.len() >= ASYNC_PARSE_THRESHOLD {
            self.busy = Some(format!("Loading {}…", path.display()));
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let result = showfile::parse_show(&bytes);
                let _ = tx.send(result);
            });
            self.io_job = Some(IoJob::Open { path, rx });
        } else {
            match showfile::parse_show(&bytes) {
                Ok(show) => self.finish_open_show(path, show),
                Err(err) => self.error = Some(err),
            }
        }
    }

    fn finish_open_show(&mut self, path: PathBuf, show: show::Show) {
        let mut view = NcodViewState::default();
        view.reset_for_show(&show);
        self.path = Some(path);
        self.document = Some(LoadedFile::Show {
            show,
            view,
            undo: undo::UndoStack::default(),
        });
        self.dirty = false;
        self.busy = None;
    }

    fn save_to(&mut self, path: PathBuf) -> bool {
        if self.io_job.is_some() {
            self.error = Some("wait for the current file operation".into());
            return false;
        }
        self.error = None;
        let Some(document) = &self.document else {
            return false;
        };
        match document {
            LoadedFile::Cof { config, .. } => match devices::tr512::cof::write(&path, config) {
                Ok(()) => {
                    self.path = Some(path);
                    self.dirty = false;
                    true
                }
                Err(err) => {
                    self.error = Some(err);
                    false
                }
            },
            LoadedFile::Show { show, undo: _, .. } => self.write_show(path, show.clone(), true),
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
                    if adopt {
                        self.path = Some(path);
                        self.dirty = false;
                    }
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
                            if adopt {
                                self.path = Some(path);
                                self.dirty = false;
                            }
                        }
                        Err(err) => self.error = Some(err),
                    }
                } else {
                    self.io_job = Some(IoJob::Save { path, adopt, rx });
                }
            }
            IoJob::Open { path, rx } => {
                if let Ok(result) = rx.try_recv() {
                    self.busy = None;
                    match result {
                        Ok(show) => self.finish_open_show(path, show),
                        Err(err) => self.error = Some(err),
                    }
                } else {
                    self.io_job = Some(IoJob::Open { path, rx });
                }
            }
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
            Some(LoadedFile::Cof { .. }) => {
                dialog.add_filter("Showtec TR-512 configuration", devices::tr512::DEVICE.config_extensions)
            }
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

    fn menu_bar(&mut self, ctx: &egui::Context) {
        let io_busy = self.io_job.is_some();
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                let edit_dirty = std::cell::Cell::new(false);
                ui.menu_button("Edit", |ui| {
                    if let Some(LoadedFile::Show { show, undo, .. }) = &mut self.document {
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
                    }
                });
                if edit_dirty.get() {
                    self.dirty = true;
                }
                ui.menu_button("File", |ui| {
                    if ui.add_enabled(!io_busy, egui::Button::new("Open…")).clicked() {
                        self.pending_open = true;
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(self.document.is_some() && !io_busy, egui::Button::new("Save"))
                        .clicked()
                    {
                        if let Some(path) = self.path.clone() {
                            self.save_to(path);
                        } else {
                            self.pending_save_as = true;
                        }
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(self.document.is_some() && !io_busy, egui::Button::new("Save As…"))
                        .clicked()
                    {
                        self.pending_save_as = true;
                        ui.close_menu();
                    }
                    let can_export = matches!(self.document, Some(LoadedFile::Show { .. })) && !io_busy;
                    for format in devices::show_formats() {
                        if ui
                            .add_enabled(can_export, egui::Button::new(format!("Export {}…", format.label)))
                            .clicked()
                        {
                            self.pending_export = Some(format);
                            ui.close_menu();
                        }
                    }
                    if ui.add_enabled(!io_busy, egui::Button::new("Close")).clicked() {
                        if let Some(LoadedFile::Show { view, .. }) = &mut self.document {
                            view.stop_playback();
                        }
                        self.document = None;
                        self.path = None;
                        self.dirty = false;
                        ui.close_menu();
                    }
                });
            });
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_io();
        if self.io_job.is_some() {
            ctx.request_repaint();
        }

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
                    for known in devices::DEVICES.iter().filter(|device| device.status == devices::Status::Supported) {
                        let code = known.code.map(|code| format!(" {code}")).unwrap_or_default();
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