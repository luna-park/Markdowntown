//! Шрифты: системные шрифты Windows как запасные для символов, которых нет во
//! встроенных шрифтах egui, и выбор шрифта редактора.
//!
//! Файлы берутся из `%WINDIR%\Fonts` (есть на любой Windows 10/11, в exe не вшиваются).
//! Отсутствующий файл молча пропускается — остаются встроенные шрифты egui.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily};

/// Семейство шрифта редактора.
const EDITOR_FAMILY: &str = "editor";

/// Семейство жирного шрифта заголовков превью (его ищет патч `egui_commonmark`).
const HEADING_FAMILY: &str = "heading";

/// Жирное начертание того же Ubuntu, что у основного текста egui (Ubuntu Light);
/// вшито в exe — в Windows его нет. Лицензия — `licenses/ubuntu-font/`.
const HEADING_FONT: &[u8] = include_bytes!("../assets/fonts/Ubuntu-Bold.ttf");

/// Запасные шрифты для символов и эмодзи (↑ ↓ ✓ → и т.п.).
const FALLBACK_FILES: &[(&str, &str)] = &[
    ("Segoe UI Symbol", "seguisym.ttf"),
    ("Segoe UI Emoji", "seguiemj.ttf"),
];

/// Моноширинные шрифты, которые можно выбрать для редактора (название, файл).
const EDITOR_FONT_FILES: &[(&str, &str)] = &[
    ("Consolas", "consola.ttf"),
    ("Cascadia Mono", "CascadiaMono.ttf"),
    ("Cascadia Code", "CascadiaCode.ttf"),
    ("Lucida Console", "lucon.ttf"),
    ("Courier New", "cour.ttf"),
];

/// Название встроенного моноширинного шрифта egui.
pub const BUILTIN_EDITOR_FONT: &str = "Hack";

pub fn editor_family() -> FontFamily {
    FontFamily::Name(EDITOR_FAMILY.into())
}

fn fonts_dir() -> Option<PathBuf> {
    std::env::var_os("WINDIR")
        .or_else(|| std::env::var_os("SystemRoot"))
        .map(|dir| PathBuf::from(dir).join("Fonts"))
}

/// Шрифты редактора, файлы которых есть в системе.
pub fn available_editor_fonts() -> Vec<&'static str> {
    let Some(dir) = fonts_dir() else {
        return Vec::new();
    };
    EDITOR_FONT_FILES
        .iter()
        .filter(|(_, file)| dir.join(file).is_file())
        .map(|(name, _)| *name)
        .collect()
}

fn load(name: &str, file: &str) -> Option<(String, Arc<FontData>)> {
    let path = fonts_dir()?.join(file);
    match std::fs::read(&path) {
        Ok(bytes) => Some((name.to_owned(), Arc::new(FontData::from_owned(bytes)))),
        Err(e) => {
            log::info!("font {name} not available ({}): {e}", path.display());
            None
        }
    }
}

/// Устанавливает шрифты в контекст. `editor_font` — название из
/// [`available_editor_fonts`]; `None` или недоступный — встроенный моноширинный.
/// Вызывать при старте и при смене шрифта редактора.
pub fn install(ctx: &egui::Context, editor_font: Option<&str>) {
    let mut defs = FontDefinitions::default();

    let fallbacks: Vec<String> = FALLBACK_FILES
        .iter()
        .filter_map(|(name, file)| load(name, file))
        .map(|(name, data)| {
            defs.font_data.insert(name.clone(), data);
            name
        })
        .collect();
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        defs.families
            .entry(family)
            .or_default()
            .extend(fallbacks.iter().cloned());
    }

    // Редактор: выбранный шрифт, затем встроенный моноширинный (в нём есть кириллица)
    // и запасные.
    let mut editor = Vec::new();
    if let Some((name, data)) = editor_font
        .and_then(|font| EDITOR_FONT_FILES.iter().find(|(n, _)| *n == font))
        .and_then(|(name, file)| load(name, file))
    {
        defs.font_data.insert(name.clone(), data);
        editor.push(name);
    }
    editor.extend(defs.families[&FontFamily::Monospace].iter().cloned());
    defs.families.insert(editor_family(), editor);

    // Заголовки: Ubuntu Bold, недостающие символы — из обычных пропорциональных.
    defs.font_data.insert(
        HEADING_FAMILY.to_owned(),
        Arc::new(FontData::from_static(HEADING_FONT)),
    );
    let mut heading = vec![HEADING_FAMILY.to_owned()];
    heading.extend(defs.families[&FontFamily::Proportional].iter().cloned());
    defs.families
        .insert(FontFamily::Name(HEADING_FAMILY.into()), heading);

    ctx.set_fonts(defs);
}
