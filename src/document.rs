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

pub struct Document {
    pub text: String,
    /// Уникален для каждого созданного/загруженного документа.
    id: u64,
    path: Option<PathBuf>,
    format: FileFormat,
    dirty: bool,
    /// Растёт при каждом изменении текста — ключ для кэшей превью.
    revision: u64,
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
        }
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
        })
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
            .unwrap_or_else(|| "untitled".to_owned())
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
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
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
