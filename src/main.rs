#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod alerts;
mod app;
mod data_dir;
mod diagram;
mod document;
mod editor;
mod export;
#[cfg(windows)]
mod file_assoc;
mod fonts;
mod gpu;
mod links;
mod logging;
mod math;
mod outline;
mod preview;
mod recovery;
mod scroll_sync;
mod segment;
mod settings;
mod single_instance;
mod style;
mod tab;
mod viewer;

use std::path::{Path, PathBuf};

#[cfg(test)]
mod assets_tests;
#[cfg(test)]
mod perf_tests;

/// Языки блоков кода (как в ```` ```lang ````), подсветка которых готовится при запуске.
/// Остальные компилируются при первой встрече, один раз за запуск.
const WARM_UP_LANGUAGES: &[&str] = &[
    "rust", "python", "bash", "sh", "bat", "cmd", "js", "json", "yaml", "c", "cpp", "cs", "java",
    "go", "html", "xml", "css", "sql", "lua", "diff", "ini", "md",
];

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
    // Файлы из командной строки (Проводник передаёт один). Абсолютные — их может
    // открыть уже запущенный экземпляр с другой текущей папкой.
    let files: Vec<PathBuf> = std::env::args_os()
        .skip(1)
        .map(|arg| std::path::absolute(&arg).unwrap_or_else(|_| arg.into()))
        .collect();
    // До журнала: процесс, передавший файлы запущенному окну, не должен затирать его журнал.
    let instance_key = data_dir.clone().or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_owned))
    });
    let inbox = match instance_key {
        Some(key) => match single_instance::start(&key, &files) {
            single_instance::Startup::Forwarded => return Ok(()),
            single_instance::Startup::Primary(inbox) => inbox,
        },
        None => single_instance::Inbox::none(),
    };
    let log_file = logging::init(data_dir.as_deref());
    // Пока создаются окно и графика — подсветка частых языков блоков кода (иначе первый
    // блок каждого языка задерживает кадр на ~30 мс).
    egui_commonmark::warm_up_highlighting(WARM_UP_LANGUAGES.iter().map(|&s| s.to_owned()));
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
        Box::new(move |cc| {
            Ok(Box::new(app::MarkdowntownApp::new(
                cc, data_dir, &files, inbox,
            )))
        }),
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
