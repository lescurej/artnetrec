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
use crate::{cof, ncod};
use std::path::PathBuf;
use std::sync::mpsc;

enum IoJob {
    Save {
        path: PathBuf,
        rx: mpsc::Receiver<Result<(), String>>,
    },
    Open {
        path: PathBuf,
        rx: mpsc::Receiver<Result<ncod::Show, String>>,
    },
}

pub enum LoadedFile {
    Ncod {
        show: ncod::Show,
        view: NcodViewState,
        undo: undo::UndoStack,
    },
    Cof {
        config: cof::Config,
        view: CofViewState,
    },
}

pub struct Tr512App {
    pub path: Option<PathBuf>,
    pub document: Option<LoadedFile>,
    pub dirty: bool,
    pub error: Option<String>,
    pub busy: Option<String>,
    pending_open: bool,
    pending_save_as: bool,
    io_job: Option<IoJob>,
}

impl Tr512App {
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
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_uppercase();
        if extension == "COF" || bytes.starts_with(b"[Config]") {
            match cof::parse(&bytes) {
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
                let result = ncod::parse(&bytes);
                let _ = tx.send(result);
            });
            self.io_job = Some(IoJob::Open { path, rx });
        } else {
            match ncod::parse(&bytes) {
                Ok(show) => self.finish_open_ncod(path, show),
                Err(err) => self.error = Some(err),
            }
        }
    }

    fn finish_open_ncod(&mut self, path: PathBuf, show: ncod::Show) {
        let mut view = NcodViewState::default();
        view.reset_for_show(&show);
        self.path = Some(path);
        self.document = Some(LoadedFile::Ncod {
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
            LoadedFile::Cof { config, .. } => match cof::write(&path, config) {
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
            LoadedFile::Ncod { show, undo: _, .. } => {
                if show.frames.len() >= ASYNC_SAVE_FRAMES {
                    self.busy = Some(format!("Saving {}…", path.display()));
                    let show = show.clone();
                    let save_path = path.clone();
                    let (tx, rx) = mpsc::channel();
                    std::thread::spawn(move || {
                        let result = ncod::write(&save_path, &show);
                        let _ = tx.send(result);
                    });
                    self.io_job = Some(IoJob::Save { path, rx });
                    true
                } else {
                    match ncod::write(&path, show) {
                        Ok(()) => {
                            self.path = Some(path);
                            self.dirty = false;
                            true
                        }
                        Err(err) => {
                            self.error = Some(err);
                            false
                        }
                    }
                }
            }
        }
    }

    fn poll_io(&mut self) {
        let Some(job) = self.io_job.take() else {
            return;
        };
        match job {
            IoJob::Save { path, rx } => {
                if let Ok(result) = rx.try_recv() {
                    self.busy = None;
                    match result {
                        Ok(()) => {
                            self.path = Some(path);
                            self.dirty = false;
                        }
                        Err(err) => self.error = Some(err),
                    }
                } else {
                    self.io_job = Some(IoJob::Save { path, rx });
                }
            }
            IoJob::Open { path, rx } => {
                if let Ok(result) = rx.try_recv() {
                    self.busy = None;
                    match result {
                        Ok(show) => self.finish_open_ncod(path, show),
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
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("TR-512", &["NCOD", "ncod", "COF", "cof"])
            .pick_file()
        {
            self.open_path(path);
        }
    }

    fn run_save_as_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new();
        dialog = match &self.document {
            Some(LoadedFile::Cof { .. }) => dialog.add_filter("COF", &["cof", "COF"]),
            Some(LoadedFile::Ncod { .. }) | None => dialog.add_filter("NCOD", &["ncod", "NCOD"]),
        };
        if let Some(path) = dialog.save_file() {
            let _ = self.save_to(path);
        }
    }

    fn menu_bar(&mut self, ctx: &egui::Context) {
        let io_busy = self.io_job.is_some();
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                let edit_dirty = std::cell::Cell::new(false);
                ui.menu_button("Edit", |ui| {
                    if let Some(LoadedFile::Ncod { show, undo, .. }) = &mut self.document {
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
                    if ui.add_enabled(!io_busy, egui::Button::new("Close")).clicked() {
                        if let Some(LoadedFile::Ncod { view, .. }) = &mut self.document {
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

impl eframe::App for Tr512App {
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
                    ui.heading("TR-512 Studio");
                    ui.label("Open a .NCOD show or .COF configuration.");
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
            LoadedFile::Ncod { show, view, undo } => ncod_view::layout(ctx, show, undo, view),
        };
        if became_dirty {
            self.mark_dirty();
        }

        if let Some(LoadedFile::Ncod { view, .. }) = &self.document {
            if view.playing {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }
    }
}