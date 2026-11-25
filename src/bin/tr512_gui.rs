#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ncod_artnet::gui::App;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1400.0, 900.0]),
        ..Default::default()
    };
    eframe::run_native(
        "DMX Recorder",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}