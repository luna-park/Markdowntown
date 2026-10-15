#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod data_dir;
mod diagram;
mod document;
mod editor;
mod fonts;
mod preview;
mod recovery;
mod scroll_sync;
mod segment;
mod settings;
mod style;
mod viewer;

#[cfg(test)]
mod perf_tests;

fn main() -> eframe::Result {
    let data_dir = data_dir::resolve();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Markdowntown")
            .with_inner_size([1200.0, 800.0])
            // Не меньше суммы минимальных ширин редактора и превью (см. app.rs).
            .with_min_inner_size([520.0, 360.0]),
        // Настройки и положение окна — в каталоге данных (portable: рядом с exe).
        // Несмотря на документацию eframe, это путь к файлу, а не к папке.
        persistence_path: data_dir.as_ref().map(|dir| dir.join("app.ron")),
        ..Default::default()
    };

    eframe::run_native(
        "Markdowntown",
        options,
        Box::new(move |cc| Ok(Box::new(app::MarkdowntownApp::new(cc, data_dir)))),
    )
}
