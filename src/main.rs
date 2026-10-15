#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod data_dir;
mod diagram;
mod document;
mod editor;
mod export;
mod fonts;
mod gpu;
mod logging;
mod preview;
mod recovery;
mod scroll_sync;
mod segment;
mod settings;
mod style;
mod viewer;

#[cfg(test)]
mod assets_tests;
#[cfg(test)]
mod perf_tests;

/// Иконка окна (заголовок, Alt+Tab). Иконка самого exe — в ресурсах (`build.rs`).
fn window_icon() -> egui::IconData {
    let image = image::load_from_memory(include_bytes!("../assets/icon-256.png"))
        .expect("embedded icon is a valid PNG")
        .into_rgba8();
    egui::IconData {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    }
}

fn main() -> eframe::Result {
    let data_dir = data_dir::resolve();
    let log_file = logging::init(data_dir.as_deref());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(app::APP_TITLE)
            .with_icon(window_icon())
            .with_inner_size([1200.0, 800.0])
            // Не меньше суммы минимальных ширин редактора и превью (см. app.rs).
            .with_min_inner_size([520.0, 360.0]),
        // Настройки и положение окна — в каталоге данных (portable: рядом с exe).
        // Несмотря на документацию eframe, это путь к файлу, а не к папке.
        persistence_path: data_dir.as_ref().map(|dir| dir.join("app.ron")),
        // Свой выбор графического адаптера: DX12 и встроенная видеокарта в приоритете.
        wgpu_options: gpu::wgpu_options(),
        ..Default::default()
    };

    let result = eframe::run_native(
        "Markdowntown",
        options,
        Box::new(move |cc| Ok(Box::new(app::MarkdowntownApp::new(cc, data_dir)))),
    );
    if let Err(e) = &result {
        // У release-сборки нет консоли: без окна с сообщением запуск просто «не происходит».
        log::error!("cannot start: {e}");
        let details = log_file.map_or_else(String::new, |p| {
            format!("\n\nDetails are in the log file:\n{}", p.display())
        });
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title(app::APP_TITLE)
            .set_description(format!("Markdowntown could not start.\n\n{e}{details}"))
            .show();
    }
    result
}
