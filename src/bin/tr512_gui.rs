#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ncod_artnet::gui::App;

fn main() -> eframe::Result<()> {
    let open = std::env::args().nth(1).map(std::path::PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1400.0, 900.0]),
        ..Default::default()
    };
    eframe::run_native(
        "DMX Recorder",
        options,
        Box::new(move |cc| {
            let mut app = App::new(cc);
            if let Some(path) = open.clone() {
                app.open_path(path);
            }
            Ok(Box::new(app))
        }),
    )
}
