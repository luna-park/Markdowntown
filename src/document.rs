//! Документ: текст, путь, признак изменений и формат файла на диске.
//!
//! Внутри текст всегда хранится с окончаниями `\n` и без BOM. Исходный формат
//! (BOM, CRLF/LF) запоминается при чтении и восстанавливается при записи.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const BOM: &[u8] = b"\xEF\xBB\xBF";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn label(self) -> &'static str {
        match self {
            Self::Lf => "LF",
            Self::CrLf => "CRLF",
        }
    }
}

/// Формат файла на диске.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileFormat {
    pub bom: bool,
    pub line_ending: LineEnding,
}

#[derive(Debug)]
pub enum LoadError {
    Io(io::Error),
    /// Файл не в UTF-8; `offset` — позиция первого некорректного байта.
    NotUtf8 {
        offset: usize,
    },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::NotUtf8 { offset } => write!(
                f,
                "file is not valid UTF-8 (invalid byte at offset {offset}); \
                 only UTF-8 files are supported"
            ),
        }
    }
}

impl From<io::Error> for LoadError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Отпечаток файла на диске: по нему замечаем изменения другими программами.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiskStamp {
    modified: std::time::SystemTime,
    len: u64,
}

impl DiskStamp {
    fn read(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            modified: meta.modified().ok()?,
            len: meta.len(),
        })
    }
}

/// Что случилось с файлом на диске с момента последнего чтения или записи.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskChange {
    Modified,
    Removed,
}

pub struct Document {
    pub text: String,
    /// Уникален для каждого созданного/загруженного документа.
    id: u64,
    path: Option<PathBuf>,
    format: FileFormat,
    dirty: bool,
    /// Растёт при каждом изменении текста — ключ для кэшей превью.
    revision: u64,
    /// Состояние файла при последнем чтении/записи (или принятое пользователем).
    disk: Option<DiskStamp>,
}

impl Document {
    pub fn new_untitled(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            id: next_id(),
            path: None,
            format: FileFormat::default(),
            dirty: false,
            revision: 0,
            disk: None,
        }
    }

    /// Документ из черновика восстановления: текст не совпадает с файлом, поэтому
    /// сразу «изменён». Формат (BOM, EOL) берётся из файла, если он ещё существует.
    pub fn recovered(path: Option<PathBuf>, text: String) -> Self {
        let on_disk = path.as_deref().and_then(|p| Self::load(p).ok());
        let mut doc = Self::new_untitled(text);
        if let Some(disk) = on_disk {
            doc.format = disk.format;
            doc.disk = disk.disk;
        }
        doc.path = path;
        doc.dirty = true;
        doc
    }

    pub fn load(path: &Path) -> Result<Self, LoadError> {
        let bytes = std::fs::read(path)?;
        let (text, format) = decode(&bytes)?;
        Ok(Self {
            text,
            id: next_id(),
            path: Some(path.to_owned()),
            format,
            dirty: false,
            revision: 0,
            disk: DiskStamp::read(path),
        })
    }

    /// Перечитывает файл с диска, отбрасывая несохранённые правки.
    pub fn reload(&mut self) -> Result<(), LoadError> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| io::Error::other("document has no path"))?;
        let fresh = Self::load(&path)?;
        self.text = fresh.text;
        self.format = fresh.format;
        self.disk = fresh.disk;
        self.dirty = false;
        self.revision += 1;
        Ok(())
    }

    /// Изменился ли файл на диске с последнего чтения/записи. Дешёвый вызов
    /// (только метаданные) — можно опрашивать раз в секунду.
    pub fn disk_change(&self) -> Option<DiskChange> {
        let path = self.path.as_deref()?;
        let known = self.disk?;
        match DiskStamp::read(path) {
            None => Some(DiskChange::Removed),
            Some(now) if now != known => Some(DiskChange::Modified),
            Some(_) => None,
        }
    }

    /// Пользователь решил оставить свою версию: больше не сообщать об этом изменении
    /// файла (документ остаётся изменённым).
    pub fn accept_disk_state(&mut self) {
        if let Some(path) = &self.path {
            self.disk = DiskStamp::read(path);
        }
        self.dirty = true;
    }

    /// Сохраняет в текущий путь. Документ без пути сохранить нельзя — используйте [`Self::save_as`].
    pub fn save(&mut self) -> io::Result<()> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| io::Error::other("document has no path"))?;
        self.save_as(&path)
    }

    pub fn save_as(&mut self, path: &Path) -> io::Result<()> {
        write_atomic(path, &encode(&self.text, self.format))?;
        self.path = Some(path.to_owned());
        self.dirty = false;
        self.disk = DiskStamp::read(path);
        Ok(())
    }

    /// Вызывать после любого изменения `text`.
    pub fn mark_edited(&mut self) {
        self.dirty = true;
        self.revision += 1;
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn format(&self) -> FileFormat {
        self.format
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Ключ для кэшей: меняется при любой правке и при замене документа.
    pub fn version(&self) -> (u64, u64) {
        (self.id, self.revision)
    }

    /// Каталог файла — база для относительных путей к картинкам.
    pub fn base_dir(&self) -> Option<&Path> {
        self.path.as_deref().and_then(Path::parent)
    }

    pub fn display_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| crate::i18n::tr!("untitled", "без имени").to_owned())
    }
}

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Байты файла → текст с `\n` + исходный формат.
pub fn decode(bytes: &[u8]) -> Result<(String, FileFormat), LoadError> {
    let (bom, body) = match bytes.strip_prefix(BOM) {
        Some(rest) => (true, rest),
        None => (false, bytes),
    };
    let text = std::str::from_utf8(body).map_err(|e| LoadError::NotUtf8 {
        offset: e.valid_up_to() + if bom { BOM.len() } else { 0 },
    })?;

    // Смешанные окончания: побеждает большинство.
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    let line_ending = if crlf > lf {
        LineEnding::CrLf
    } else {
        LineEnding::Lf
    };

    let text = if crlf > 0 {
        text.replace("\r\n", "\n")
    } else {
        text.to_owned()
    };
    Ok((text, FileFormat { bom, line_ending }))
}

/// Текст с `\n` → байты файла в заданном формате.
pub fn encode(text: &str, format: FileFormat) -> Vec<u8> {
    // CRLF мог попасть в текст через вставку из буфера обмена.
    let normalized;
    let text = if text.contains('\r') {
        normalized = text.replace("\r\n", "\n");
        normalized.as_str()
    } else {
        text
    };
    let mut out = Vec::with_capacity(text.len() + BOM.len());
    if format.bom {
        out.extend_from_slice(BOM);
    }
    match format.line_ending {
        LineEnding::Lf => out.extend_from_slice(text.as_bytes()),
        LineEnding::CrLf => out.extend_from_slice(text.replace('\n', "\r\n").as_bytes()),
    }
    out
}

/// Пишет во временный файл рядом и переименовывает: при сбое старый файл не портится.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".markdowntown-tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(bytes: &[u8]) {
        let (text, format) = decode(bytes).unwrap();
        assert!(!text.contains('\r'));
        assert_eq!(encode(&text, format), bytes);
    }

    #[test]
    fn roundtrip_formats() {
        roundtrip(b"# a\nb\n");
        roundtrip(b"# a\r\nb\r\n");
        roundtrip(b"\xEF\xBB\xBF# a\r\nb\r\n");
        roundtrip("\u{FEFF}привет\n".as_bytes());
        roundtrip(b"");
        roundtrip(b"no newline at end");
    }

    #[test]
    fn detects_format() {
        let (_, f) = decode(b"\xEF\xBB\xBFa\r\nb\r\n").unwrap();
        assert_eq!(
            f,
            FileFormat {
                bom: true,
                line_ending: LineEnding::CrLf
            }
        );
        let (_, f) = decode(b"a\nb\n").unwrap();
        assert_eq!(f, FileFormat::default());
    }

    #[test]
    fn mixed_line_endings_use_majority() {
        let (text, f) = decode(b"a\r\nb\r\nc\n").unwrap();
        assert_eq!(text, "a\nb\nc\n");
        assert_eq!(f.line_ending, LineEnding::CrLf);
    }

    #[test]
    fn pasted_crlf_is_not_doubled() {
        let crlf = FileFormat {
            bom: false,
            line_ending: LineEnding::CrLf,
        };
        assert_eq!(encode("a\r\nb\n", crlf), b"a\r\nb\r\n");
        assert_eq!(encode("a\r\nb\n", FileFormat::default()), b"a\nb\n");
    }

    #[test]
    fn rejects_non_utf8() {
        // "Привет" в CP1251.
        let err = decode(b"ok \xCF\xF0\xE8").unwrap_err();
        assert!(matches!(err, LoadError::NotUtf8 { offset: 3 }));
        let err = decode(b"\xEF\xBB\xBFok \xFF").unwrap_err();
        assert!(matches!(err, LoadError::NotUtf8 { offset: 6 }));
    }

    #[test]
    fn detects_external_changes_and_reloads() {
        let dir = std::env::temp_dir().join(format!("markdowntown-disk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        std::fs::write(&path, "one\n").unwrap();

        let mut doc = Document::load(&path).unwrap();
        assert_eq!(doc.disk_change(), None);

        // Другая длина — изменение видно даже при грубом разрешении времени.
        std::fs::write(&path, "two two\n").unwrap();
        assert_eq!(doc.disk_change(), Some(DiskChange::Modified));

        let version = doc.version();
        doc.reload().unwrap();
        assert_eq!(doc.text, "two two\n");
        assert!(!doc.is_dirty());
        assert_ne!(doc.version(), version, "preview must re-render");
        assert_eq!(doc.disk_change(), None);

        // Своё сохранение — не внешнее изменение.
        doc.text.push_str("three\n");
        doc.mark_edited();
        doc.save().unwrap();
        assert_eq!(doc.disk_change(), None);

        std::fs::write(&path, "external\n").unwrap();
        doc.accept_disk_state();
        assert_eq!(doc.disk_change(), None);
        assert!(doc.is_dirty());

        std::fs::remove_file(&path).unwrap();
        assert_eq!(doc.disk_change(), Some(DiskChange::Removed));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn recovered_document_is_dirty_and_keeps_file_format() {
        let dir = std::env::temp_dir().join(format!("markdowntown-rec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        std::fs::write(&path, b"a\r\nb\r\n").unwrap();

        let doc = Document::recovered(Some(path.clone()), "draft\n".into());
        assert!(doc.is_dirty());
        assert_eq!(doc.text, "draft\n");
        assert_eq!(doc.format().line_ending, LineEnding::CrLf);
        assert_eq!(doc.path(), Some(path.as_path()));

        let untitled = Document::recovered(None, "x".into());
        assert!(untitled.is_dirty() && untitled.path().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_and_load_preserve_format() {
        let dir = std::env::temp_dir().join(format!("markdowntown-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        let original = b"\xEF\xBB\xBF# T\r\n\r\ntext\r\n";
        std::fs::write(&path, original).unwrap();

        let mut doc = Document::load(&path).unwrap();
        assert_eq!(doc.text, "# T\n\ntext\n");
        doc.text.push_str("more\n");
        doc.mark_edited();
        assert!(doc.is_dirty());
        doc.save().unwrap();
        assert!(!doc.is_dirty());

        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"\xEF\xBB\xBF# T\r\n\r\ntext\r\nmore\r\n"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
