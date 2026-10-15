//! Каталог данных приложения (настройки, черновики восстановления).
//!
//! Portable-режим: `data\` рядом с exe. Если туда нельзя писать (exe в Program Files,
//! на защищённом носителе) — `%APPDATA%\Markdowntown`.

use std::path::{Path, PathBuf};

const PORTABLE_DIR: &str = "data";
const APP_DIR: &str = "Markdowntown";

/// Находит (и создаёт) каталог данных. `None` — писать некуда; приложение работает
/// без сохранения настроек и черновиков.
pub fn resolve() -> Option<PathBuf> {
    let portable = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(PORTABLE_DIR)));
    let roaming = std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join(APP_DIR));
    [portable, roaming]
        .into_iter()
        .flatten()
        .find(|dir| is_writable_dir(dir))
}

/// Создаёт каталог при необходимости и проверяет запись пробным файлом.
fn is_writable_dir(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".write-test");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writable_dir_is_created() {
        let dir = std::env::temp_dir()
            .join(format!("markdowntown-data-{}", std::process::id()))
            .join("nested");
        assert!(is_writable_dir(&dir));
        assert!(dir.is_dir());
        assert!(!dir.join(".write-test").exists());
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn file_in_place_of_dir_is_not_writable() {
        let file = std::env::temp_dir().join(format!("markdowntown-file-{}", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        assert!(!is_writable_dir(&file));
        std::fs::remove_file(&file).unwrap();
    }
}
