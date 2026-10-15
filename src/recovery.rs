//! Черновики для восстановления после сбоя.
//!
//! Пока в документе есть несохранённые правки, его текст периодически пишется в
//! `<каталог данных>\drafts\`. Сам файл документа не трогаем. При штатном
//! сохранении или закрытии черновик удаляется; если он остался — прошлый запуск
//! завершился аварийно, и при старте предлагается восстановить текст.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const DRAFTS_DIR: &str = "drafts";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    /// Файл документа; `None` — безымянный документ.
    pub path: Option<PathBuf>,
    pub text: String,
    /// Время записи черновика, секунды Unix.
    pub saved_at: u64,
    /// Ключ безымянного документа (у каждой вкладки свой). `None` — черновик старой
    /// версии с общим файлом `untitled.json`.
    #[serde(default)]
    pub untitled: Option<u64>,
}

impl Draft {
    pub fn new(path: Option<PathBuf>, untitled: Option<u64>, text: String) -> Self {
        let saved_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Self {
            path,
            text,
            saved_at,
            untitled,
        }
    }

    pub fn age_description(&self) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let minutes = now.saturating_sub(self.saved_at) / 60;
        match minutes {
            0 => "less than a minute ago".to_owned(),
            1 => "1 minute ago".to_owned(),
            m if m < 120 => format!("{m} minutes ago"),
            m if m < 48 * 60 => format!("{} hours ago", m / 60),
            m => format!("{} days ago", m / 60 / 24),
        }
    }
}

/// Хранилище черновиков в каталоге данных.
pub struct Drafts {
    dir: PathBuf,
}

impl Drafts {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join(DRAFTS_DIR),
        }
    }

    /// Файл черновика для документа: имя — хэш пути; у безымянного — по его ключу.
    fn file_for(&self, path: Option<&Path>, untitled: Option<u64>) -> PathBuf {
        let name = match (path, untitled) {
            (Some(p), _) => {
                use std::hash::{DefaultHasher, Hash, Hasher};
                let mut h = DefaultHasher::new();
                p.to_string_lossy().to_lowercase().hash(&mut h);
                format!("{:016x}.json", h.finish())
            }
            (None, Some(key)) => format!("untitled-{key:016x}.json"),
            (None, None) => "untitled.json".to_owned(),
        };
        self.dir.join(name)
    }

    pub fn save(&self, draft: &Draft) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let json = serde_json::to_vec(draft).map_err(std::io::Error::other)?;
        let file = self.file_for(draft.path.as_deref(), draft.untitled);
        // Через временный файл: сбой во время записи не портит прошлый черновик.
        let tmp = file.with_extension("tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &file)
    }

    /// `untitled` важен только для безымянного документа (`path` = `None`).
    pub fn remove(&self, path: Option<&Path>, untitled: Option<u64>) {
        let _ = std::fs::remove_file(self.file_for(path, untitled));
    }

    /// Все оставшиеся черновики, свежие первыми. Повреждённые файлы пропускаются.
    pub fn list(&self) -> Vec<Draft> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut drafts: Vec<Draft> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| std::fs::read(e.path()).ok())
            .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
            .collect();
        drafts.sort_by_key(|d: &Draft| std::cmp::Reverse(d.saved_at));
        drafts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_drafts(name: &str) -> (PathBuf, Drafts) {
        let dir = std::env::temp_dir().join(format!("markdowntown-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let drafts = Drafts::new(&dir);
        (dir, drafts)
    }

    #[test]
    fn save_list_remove() {
        let (dir, drafts) = temp_drafts("drafts");
        assert!(drafts.list().is_empty());

        let path = PathBuf::from(r"C:\docs\Заметки.md");
        let mut older = Draft::new(Some(path.clone()), None, "текст".into());
        older.saved_at -= 100;
        drafts.save(&older).unwrap();
        drafts
            .save(&Draft::new(None, Some(1), "first".into()))
            .unwrap();
        drafts
            .save(&Draft::new(None, Some(2), "second".into()))
            .unwrap();

        let list = drafts.list();
        assert_eq!(list.len(), 3, "untitled documents keep separate drafts");
        assert_eq!(list[2].text, "текст", "fresh first");

        // Повторное сохранение того же документа перезаписывает черновик.
        drafts
            .save(&Draft::new(Some(path.clone()), Some(7), "новый".into()))
            .unwrap();
        assert_eq!(drafts.list().len(), 3);

        drafts.remove(Some(&path), Some(7));
        drafts.remove(None, Some(1));
        drafts.remove(None, Some(2));
        assert!(drafts.list().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn old_untitled_draft_is_read_and_removed() {
        let (dir, drafts) = temp_drafts("legacy");
        std::fs::create_dir_all(dir.join(DRAFTS_DIR)).unwrap();
        std::fs::write(
            dir.join(DRAFTS_DIR).join("untitled.json"),
            br#"{"path":null,"text":"old","saved_at":1}"#,
        )
        .unwrap();
        let list = drafts.list();
        assert_eq!((list.len(), list[0].untitled), (1, None));
        drafts.remove(None, list[0].untitled);
        assert!(drafts.list().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn corrupted_draft_is_skipped() {
        let (dir, drafts) = temp_drafts("corrupt");
        std::fs::create_dir_all(dir.join(DRAFTS_DIR)).unwrap();
        std::fs::write(dir.join(DRAFTS_DIR).join("bad.json"), b"{not json").unwrap();
        drafts
            .save(&Draft::new(None, Some(1), "ok".into()))
            .unwrap();
        assert_eq!(drafts.list().len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
