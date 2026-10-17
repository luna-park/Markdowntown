//! Настройки, сохраняемые между запусками (через persistence eframe).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Ключ в хранилище eframe.
pub const STORAGE_KEY: &str = "settings";
/// Сколько недавних файлов помнить.
const MAX_RECENT: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ViewMode {
    #[default]
    Both,
    Editor,
    Preview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    pub fn preference(self) -> egui::ThemePreference {
        match self {
            Self::System => egui::ThemePreference::System,
            Self::Light => egui::ThemePreference::Light,
            Self::Dark => egui::ThemePreference::Dark,
        }
    }
}

/// Поля с `#[serde(default)]`: старые файлы настроек без новых полей читаются без ошибок.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub editor_font_size: f32,
    /// Редактор: `None` — встроенный моноширинный шрифт.
    pub editor_font: Option<String>,
    /// Масштаб интерфейса (Ctrl+Plus / Ctrl+Minus).
    pub zoom: f32,
    pub sync_scroll: bool,
    pub editor_ratio: f32,
    pub view_mode: ViewMode,
    /// Панель оглавления (View → Outline).
    pub show_outline: bool,
    /// Превью — колонкой ограниченной ширины по центру (View → Readable width).
    pub readable_width: bool,
    pub theme: Theme,
    /// Недавние файлы, свежие первыми.
    pub recent_files: Vec<PathBuf>,
    /// Файлы, открытые во вкладках при закрытии, — открываются при следующем запуске.
    /// Безымянные вкладки сюда не попадают.
    pub session: Vec<PathBuf>,
    /// Активная вкладка сеанса (индекс в `session`).
    pub session_active: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            editor_font_size: crate::style::EDITOR_FONT_SIZE,
            editor_font: None,
            zoom: 1.0,
            sync_scroll: true,
            editor_ratio: 0.5,
            view_mode: ViewMode::default(),
            show_outline: false,
            readable_width: true,
            theme: Theme::default(),
            recent_files: Vec::new(),
            session: Vec::new(),
            session_active: 0,
        }
    }
}

impl Settings {
    /// Добавляет файл в начало списка недавних (без дублей, не больше `MAX_RECENT`).
    pub fn add_recent(&mut self, path: &Path) {
        self.recent_files.retain(|p| !same_path(p, path));
        self.recent_files.insert(0, path.to_owned());
        self.recent_files.truncate(MAX_RECENT);
    }

    pub fn remove_recent(&mut self, path: &Path) {
        self.recent_files.retain(|p| !same_path(p, path));
    }
}

/// Пути на Windows сравниваются без учёта регистра и вида разделителей.
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_files_are_deduplicated_and_capped() {
        let mut s = Settings::default();
        for i in 0..15 {
            s.add_recent(Path::new(&format!("C:\\docs\\{i}.md")));
        }
        assert_eq!(s.recent_files.len(), MAX_RECENT);
        assert_eq!(s.recent_files[0], Path::new("C:\\docs\\14.md"));

        s.add_recent(Path::new("c:/docs/10.md"));
        assert_eq!(s.recent_files.len(), MAX_RECENT);
        assert_eq!(s.recent_files[0], Path::new("c:/docs/10.md"));
        assert_eq!(
            s.recent_files
                .iter()
                .filter(|p| same_path(p, Path::new("C:\\docs\\10.md")))
                .count(),
            1
        );

        s.remove_recent(Path::new("C:\\DOCS\\14.md"));
        assert!(!s.recent_files.iter().any(|p| p.ends_with("14.md")));
    }

    #[test]
    fn old_settings_without_new_fields_load() {
        let s: Settings = serde_json::from_str(r#"{ "editor_font_size": 18.0 }"#).unwrap();
        assert_eq!(s.editor_font_size, 18.0);
        assert!(s.sync_scroll);
        assert!(s.readable_width);
        assert_eq!(s.view_mode, ViewMode::Both);
    }
}
