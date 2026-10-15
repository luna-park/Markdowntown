#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod diagram;
mod document;
mod preview;
mod segment;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("mdview")
            .with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };

    eframe::run_native(
        "mdview",
        options,
        Box::new(|cc| Ok(Box::new(app::MdViewApp::new(cc)))),
    )
}
