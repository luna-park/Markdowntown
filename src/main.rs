#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod diagram;
mod document;
mod preview;
mod segment;
mod viewer;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Markdowntown")
            .with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Markdowntown",
        options,
        Box::new(|cc| Ok(Box::new(app::MarkdowntownApp::new(cc)))),
    )
}
