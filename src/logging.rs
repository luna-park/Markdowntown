//! Журнал: `<каталог данных>\markdowntown.log` (перезаписывается при каждом запуске).
//!
//! У release-сборки нет консоли, поэтому журнал — единственный способ узнать, какой
//! графический адаптер выбран и что пошло не так. Уровень по умолчанию — `info` для
//! приложения и графики, `warn` для остального; переопределяется `RUST_LOG`.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const LOG_FILE: &str = "markdowntown.log";
// Шумные библиотеки (CSS в SVG диаграмм, пустые пути, системные шрифты, загрузчик
// Vulkan) — только ошибки.
const DEFAULT_FILTER: &str = concat!(
    "warn,markdowntown=info,eframe=info,egui_wgpu=info,",
    "simplecss=error,tiny_skia=error,usvg=error,fontdb=error,wgpu_hal=error"
);

/// Пишет в файл и (в debug-сборке) дублирует в stderr.
struct Tee(Mutex<File>);

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if cfg!(debug_assertions) {
            let _ = std::io::stderr().write_all(buf);
        }
        self.0.lock().expect("log file lock").write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().expect("log file lock").flush()
    }
}

/// Настраивает журнал и запись паник в него. Возвращает путь к файлу журнала.
pub fn init(data_dir: Option<&Path>) -> Option<PathBuf> {
    let mut builder = env_logger::Builder::new();
    builder.parse_filters(&std::env::var("RUST_LOG").unwrap_or_else(|_| DEFAULT_FILTER.to_owned()));
    let path = data_dir.map(|dir| dir.join(LOG_FILE));
    if let Some(file) = path.as_ref().and_then(|p| File::create(p).ok()) {
        builder.target(env_logger::Target::Pipe(Box::new(Tee(Mutex::new(file)))));
    }
    let _ = builder.try_init();

    // Паника в release без консоли иначе пропадает бесследно.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        default_hook(info);
    }));

    log::info!(
        "Markdowntown {} starting; data folder: {}",
        env!("CARGO_PKG_VERSION"),
        data_dir.map_or_else(|| "<none>".to_owned(), |d| d.display().to_string())
    );
    path
}
