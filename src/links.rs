//! Ссылки из превью: `egui_commonmark` открывает любую ссылку командой egui `OpenUrl`
//! (то есть в браузере). Локальные ссылки перехватываем: Markdown-файл — во вкладке,
//! `#якорь` — переход к заголовку, прочие локальные файлы — программой по умолчанию
//! (безопасные типы) или показом в Проводнике. Внешние (`https:`, `mailto:`) — как было.

use std::path::{Path, PathBuf};

/// Куда ведёт ссылка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Внешний адрес — оставить браузеру.
    External,
    /// Заголовок в текущем документе (`#раздел`).
    Anchor(String),
    /// Markdown-файл, возможно с якорем (`other.md#раздел`).
    Markdown {
        path: PathBuf,
        anchor: Option<String>,
    },
    /// Другой существующий локальный файл или папка.
    LocalFile(PathBuf),
    /// Локальный путь, которого нет.
    Missing(PathBuf),
}

const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown", "mdown", "mkd"];

/// Типы, которые можно открыть программой по умолчанию по клику: документы и картинки.
/// Остальное (`.exe`, `.bat`, `.lnk`…) только показываем в Проводнике — клик по ссылке
/// в документе не должен ничего запускать.
const SAFE_TO_OPEN: &[&str] = &[
    "pdf", "txt", "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "html", "htm", "csv",
    "json", "docx", "xlsx", "pptx", "odt",
];

/// `base` — папка текущего документа (у безымянного — `None`).
pub fn classify(url: &str, base: Option<&Path>) -> Target {
    let url = url.trim();
    if let Some(anchor) = url.strip_prefix('#') {
        return Target::Anchor(crate::export::percent_decode(anchor));
    }
    let local = if let Some(rest) = url.strip_prefix("file://") {
        // file:///C:/dir/x.md → C:/dir/x.md; file://server/share/x.md → \\server\share\x.md
        match rest.strip_prefix('/') {
            Some(path) => path.to_owned(),
            None => format!(r"\\{rest}"),
        }
    } else if has_scheme(url) {
        return Target::External;
    } else {
        url.to_owned()
    };

    // Якорь и параметры — не часть пути.
    let (path_part, anchor) = match local.split_once('#') {
        Some((p, a)) => (p, Some(crate::export::percent_decode(a))),
        None => (local.as_str(), None),
    };
    let path_part = path_part.split('?').next().unwrap_or_default();
    let path = PathBuf::from(crate::export::percent_decode(path_part));
    let path = if path.is_absolute() {
        path
    } else {
        match base {
            Some(base) => base.join(path),
            // У безымянного документа нет папки — относительную ссылку не разрешить.
            None => return Target::Missing(path),
        }
    };
    let path = normalize(&path);

    if !path.exists() {
        return Target::Missing(path);
    }
    if is_markdown(&path) && path.is_file() {
        Target::Markdown { path, anchor }
    } else {
        Target::LocalFile(path)
    }
}

/// `https:`, `mailto:` и т.п. Буква диска (`C:`) — не схема.
fn has_scheme(url: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    scheme.len() > 1
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| MARKDOWN_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

pub fn safe_to_open(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SAFE_TO_OPEN.contains(&e.to_ascii_lowercase().as_str()))
}

/// Убирает `.` и `..` без обращения к диску (`canonicalize` дал бы `\\?\`-пути).
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Начало заголовка с якорем `anchor` (как у GitHub и в HTML-экспорте: `#my-section`,
/// повторы — `#my-section-1`).
pub fn find_anchor(text: &str, anchor: &str) -> Option<usize> {
    let wanted = anchor.to_lowercase();
    let mut used = std::collections::HashMap::<String, usize>::new();
    for heading in crate::outline::headings(text) {
        let mut slug = crate::export::slugify(&heading.text);
        let count = used.entry(slug.clone()).or_insert(0);
        if *count > 0 {
            slug = format!("{slug}-{count}");
        }
        *count += 1;
        if slug == wanted {
            return Some(heading.byte);
        }
    }
    find_footnote(text, anchor)
}

/// Куда ведёт ссылка — для строки статуса при наведении: то же, что сделает клик
/// (`classify`). `text` — текущий документ (заголовки и сноски для якорей). `true` —
/// ссылка никуда не ведёт.
pub fn describe(url: &str, base: Option<&Path>, text: &str) -> (String, bool) {
    match classify(url, base) {
        Target::External => (url.trim().to_owned(), false),
        Target::Anchor(anchor) => describe_anchor(text, &anchor),
        Target::Markdown { path, anchor } => {
            let mut s = format!("Open in a tab: {}", path.display());
            if let Some(anchor) = anchor {
                s.push('#');
                s.push_str(&anchor);
            }
            (s, false)
        }
        Target::LocalFile(path) => {
            // Как `open_local`: что сделает клик.
            let action = if path.is_dir() {
                "Open the folder"
            } else if safe_to_open(&path) {
                "Open with the default app"
            } else {
                "Show in Explorer"
            };
            (format!("{action}: {}", path.display()), false)
        }
        Target::Missing(path) => (format!("Not found: {}", path.display()), true),
    }
}

/// Якорь текущего документа — в том же порядке, что `find_anchor`: заголовок, затем сноска.
fn describe_anchor(text: &str, anchor: &str) -> (String, bool) {
    let Some(byte) = find_anchor(text, anchor) else {
        return (format!("Not found: #{anchor}"), true);
    };
    if let Some(heading) = crate::outline::headings(text)
        .into_iter()
        .find(|h| h.byte == byte)
    {
        return (format!("Go to “{}”", heading.text), false);
    }
    if anchor.starts_with("fnref-") {
        return ("Back to the reference".to_owned(), false);
    }
    let number = crate::segment::split_all(text)
        .footnotes
        .iter()
        .position(|f| f.definition == byte)
        .map_or(0, |i| i + 1);
    (format!("Footnote {number}"), false)
}

/// Якоря сносок, как в HTML-экспорте и у GitHub: `fn-<метка>` — определение сноски,
/// `fnref-<метка>` — первая ссылка на неё.
fn find_footnote(text: &str, anchor: &str) -> Option<usize> {
    let (label, definition) = match anchor.strip_prefix("fnref-") {
        Some(label) => (label, false),
        None => (anchor.strip_prefix("fn-")?, true),
    };
    let footnote = crate::segment::split_all(text)
        .footnotes
        .into_iter()
        .find(|f| f.label == label)?;
    Some(if definition {
        footnote.definition
    } else {
        footnote.first_ref
    })
}

/// Открыть локальный файл программой по умолчанию (безопасные типы) или показать
/// в Проводнике. Возвращает сообщение для строки статуса.
pub fn open_local(path: &Path) -> String {
    let result = if safe_to_open(path) || path.is_dir() {
        open_with_shell(path).map(|()| format!("Opened {}", path.display()))
    } else {
        reveal_in_explorer(path).map(|()| format!("Shown in Explorer: {}", path.display()))
    };
    result.unwrap_or_else(|e| format!("Cannot open {}: {e}", path.display()))
}

#[cfg(windows)]
fn open_with_shell(path: &Path) -> std::io::Result<()> {
    shell_open(path.as_os_str())
}

/// Открыть файл, папку или адрес (`ms-settings:…`) обработчиком Windows по умолчанию —
/// как двойной клик в Проводнике. Через `ShellExecuteW`, а не `explorer.exe <аргумент>`:
/// Проводник принимает не всякий адрес и молча открывает вместо него «Документы».
#[cfg(windows)]
pub fn shell_open(target: &std::ffi::OsStr) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let target: Vec<u16> = target.encode_wide().chain(Some(0)).collect();
    let verb: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
    // SAFETY: строки завершены нулём и живут до конца вызова; окно-владелец не нужно.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // По документации успех — значение больше 32, иначе — код ошибки.
    let code = result as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("ShellExecute error {code}")))
    }
}

#[cfg(windows)]
fn reveal_in_explorer(path: &Path) -> std::io::Result<()> {
    let mut arg = std::ffi::OsString::from("/select,");
    arg.push(path);
    std::process::Command::new("explorer.exe")
        .arg(arg)
        .spawn()
        .map(|_| ())
}

#[cfg(not(windows))]
fn open_with_shell(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(|_| ())
}

#[cfg(not(windows))]
fn reveal_in_explorer(path: &Path) -> std::io::Result<()> {
    open_with_shell(path.parent().unwrap_or(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mdt-links-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        dir
    }

    #[test]
    fn external_and_anchor() {
        assert_eq!(classify("https://example.com/a.md", None), Target::External);
        assert_eq!(classify("mailto:me@example.com", None), Target::External);
        assert_eq!(
            classify("#my-section", None),
            Target::Anchor("my-section".into())
        );
        assert_eq!(
            classify("#%D1%80%D0%B0%D0%B7%D0%B4%D0%B5%D0%BB", None),
            Target::Anchor("раздел".into())
        );
    }

    #[test]
    fn relative_markdown_and_files() {
        let dir = temp_dir("rel");
        std::fs::write(dir.join("other note.md"), "# Other").unwrap();
        std::fs::write(dir.join("sub").join("deep.MARKDOWN"), "").unwrap();
        std::fs::write(dir.join("doc.pdf"), "").unwrap();
        let base = dir.join("sub");

        assert_eq!(
            classify("../other%20note.md#part-2", Some(&base)),
            Target::Markdown {
                path: dir.join("other note.md"),
                anchor: Some("part-2".into())
            }
        );
        assert_eq!(
            classify("./deep.MARKDOWN", Some(&base)),
            Target::Markdown {
                path: base.join("deep.MARKDOWN"),
                anchor: None
            }
        );
        assert_eq!(
            classify("../doc.pdf", Some(&base)),
            Target::LocalFile(dir.join("doc.pdf"))
        );
        assert_eq!(
            classify("missing.md", Some(&base)),
            Target::Missing(base.join("missing.md"))
        );
        // Безымянный документ: относительную ссылку не разрешить.
        assert!(matches!(classify("x.md", None), Target::Missing(_)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn absolute_and_file_urls() {
        let dir = temp_dir("abs");
        let file = dir.join("a.md");
        std::fs::write(&file, "").unwrap();
        let expected = Target::Markdown {
            path: file.clone(),
            anchor: None,
        };
        assert_eq!(classify(&file.display().to_string(), None), expected);
        let url = format!("file:///{}", file.display().to_string().replace('\\', "/"));
        assert_eq!(
            classify(&url, None),
            Target::Markdown {
                path: PathBuf::from(file.display().to_string().replace('\\', "/")),
                anchor: None
            }
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn drive_letter_is_not_a_scheme() {
        assert!(!has_scheme(r"C:\docs\a.md"));
        assert!(has_scheme("https://x"));
        assert!(has_scheme("vscode:open"));
    }

    #[test]
    fn executables_are_not_opened() {
        assert!(safe_to_open(Path::new("a.PDF")));
        assert!(!safe_to_open(Path::new("setup.exe")));
        assert!(!safe_to_open(Path::new("run.bat")));
        assert!(!safe_to_open(Path::new("noext")));
    }

    #[test]
    fn anchors_match_github_slugs() {
        let text = "# Intro\n\n## Part `two`!\n\n## Same\n\n## Same\n";
        assert_eq!(find_anchor(text, "intro"), Some(0));
        assert_eq!(find_anchor(text, "part-two"), Some(9));
        assert_eq!(find_anchor(text, "same-1"), text.rfind("## Same"));
        assert_eq!(find_anchor(text, "Intro"), Some(0), "case-insensitive");
        assert_eq!(find_anchor(text, "nope"), None);
    }

    #[test]
    fn footnote_anchors() {
        let text = "Text[^n] and again[^n].\n\n[^n]: The note.\n";
        assert_eq!(find_anchor(text, "fn-n"), text.find("[^n]:"));
        assert_eq!(find_anchor(text, "fnref-n"), text.find("[^n]"));
        assert_eq!(find_anchor(text, "fn-missing"), None);
    }

    #[test]
    fn describes_where_links_go() {
        let dir = temp_dir("describe");
        std::fs::write(dir.join("other.md"), "# x").unwrap();
        std::fs::write(dir.join("pic.png"), b"PNG").unwrap();
        std::fs::write(dir.join("tool.exe"), b"MZ").unwrap();
        let text = "# Intro\n\nRef[^a] and[^b].\n\n[^b]: B.\n\n[^a]: A.\n";
        let d = |url: &str| describe(url, Some(&dir), text);

        assert_eq!(
            d("https://example.com/x"),
            ("https://example.com/x".into(), false)
        );
        assert_eq!(d("#intro"), ("Go to “Intro”".into(), false));
        assert_eq!(d("#nope"), ("Not found: #nope".into(), true));
        // Номер сноски — по порядку первой ссылки, как в превью.
        assert_eq!(d("#fn-a"), ("Footnote 1".into(), false));
        assert_eq!(d("#fn-b"), ("Footnote 2".into(), false));
        assert_eq!(d("#fnref-a"), ("Back to the reference".into(), false));

        let (s, missing) = d("other.md#part");
        assert!(
            s.starts_with("Open in a tab: ") && s.ends_with("other.md#part"),
            "{s}"
        );
        assert!(!missing);
        assert!(d("pic.png").0.starts_with("Open with the default app: "));
        assert!(d("tool.exe").0.starts_with("Show in Explorer: "));
        assert!(d("sub").0.starts_with("Open the folder: "));
        let (s, missing) = d("gone.md");
        assert!(s.starts_with("Not found: ") && missing, "{s}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
