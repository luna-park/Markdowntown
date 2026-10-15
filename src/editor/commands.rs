//! Команды редактора над текстом: форматирование выделения, вставка ссылки,
//! продолжение списков. Чистые функции над `&str` и байтовыми диапазонами.

use std::ops::Range;

/// Правка текста: заменить `range` на `replacement`, затем выделить `selection`
/// (байтовые позиции в тексте *после* правки).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub replacement: String,
    pub selection: Range<usize>,
}

impl Edit {
    pub fn apply(&self, text: &mut String) {
        text.replace_range(self.range.clone(), &self.replacement);
    }
}

/// Обернуть выделение маркером (`**`, `*`, `` ` ``) или снять обёртку, если она уже есть
/// — внутри выделения или сразу вокруг него. Без выделения вставляет пару маркеров
/// и ставит курсор между ними.
pub fn toggle_wrap(text: &str, selection: Range<usize>, marker: &str) -> Edit {
    let m = marker.len();
    let selected = &text[selection.clone()];

    // Маркеры внутри выделения: «**текст**» выделено целиком.
    if selected.len() >= 2 * m && selected.starts_with(marker) && selected.ends_with(marker) {
        let inner = &selected[m..selected.len() - m];
        return Edit {
            range: selection.clone(),
            replacement: inner.to_owned(),
            selection: selection.start..selection.start + inner.len(),
        };
    }
    // Маркеры вокруг выделения: выделено только «текст» в «**текст**».
    // `get`, а не срез: перед выделением может стоять многобайтовый символ.
    let before = selection
        .start
        .checked_sub(m)
        .and_then(|s| text.get(s..selection.start));
    let after = text.get(selection.end..selection.end + m);
    if before == Some(marker)
        && after == Some(marker)
        && !is_longer_marker(text, &selection, marker)
    {
        return Edit {
            range: selection.start - m..selection.end + m,
            replacement: selected.to_owned(),
            selection: selection.start - m..selection.end - m,
        };
    }

    Edit {
        range: selection.clone(),
        replacement: format!("{marker}{selected}{marker}"),
        selection: selection.start + m..selection.end + m,
    }
}

/// `*` вокруг выделения, которое на самом деле обёрнуто в `**`: снимать курсив нельзя.
fn is_longer_marker(text: &str, selection: &Range<usize>, marker: &str) -> bool {
    let m = marker.len();
    let ch = marker.as_bytes()[0];
    let bytes = text.as_bytes();
    marker.bytes().all(|b| b == ch)
        && (selection.start > m && bytes[selection.start - m - 1] == ch
            || bytes.get(selection.end + m) == Some(&ch))
}

/// Вставить ссылку: выделенный текст становится текстом ссылки, `url` выделяется
/// для замены. Если выделено похожее на адрес — оно становится адресом.
pub fn insert_link(text: &str, selection: Range<usize>) -> Edit {
    let selected = &text[selection.clone()];
    let start = selection.start;
    if looks_like_url(selected) {
        let replacement = format!("[text]({selected})");
        return Edit {
            range: selection,
            replacement,
            selection: start + 1..start + 5,
        };
    }
    let label = if selected.is_empty() {
        "text"
    } else {
        selected
    };
    let replacement = format!("[{label}](url)");
    let url_start = start + 1 + label.len() + 2;
    Edit {
        range: selection,
        replacement,
        selection: url_start..url_start + 3,
    }
}

fn looks_like_url(s: &str) -> bool {
    !s.contains(char::is_whitespace)
        && (s.starts_with("http://") || s.starts_with("https://") || s.starts_with("www."))
}

/// Enter в конце пункта списка (или цитаты): продолжить разметку на новой строке.
/// На пустом пункте — убрать маркер и закончить список. `None` — обычный перенос строки.
pub fn continue_list(text: &str, cursor: usize) -> Option<Edit> {
    let line_start = text[..cursor].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[cursor..].find('\n').map_or(text.len(), |i| cursor + i);
    // Курсор в середине строки — разрываем её как обычно, разметку не трогаем.
    if !text[cursor..line_end].trim().is_empty() {
        return None;
    }
    let line = &text[line_start..cursor];
    let prefix = list_prefix(line)?;

    if line[prefix.len..].trim().is_empty() {
        // Пустой пункт: снять маркер, оставив отступ.
        let indent = &line[..prefix.indent];
        return Some(Edit {
            range: line_start..cursor,
            replacement: indent.to_owned(),
            selection: line_start + indent.len()..line_start + indent.len(),
        });
    }

    let insert = format!("\n{}", prefix.next);
    let pos = cursor + insert.len();
    Some(Edit {
        range: cursor..cursor,
        replacement: insert,
        selection: pos..pos,
    })
}

struct ListPrefix {
    /// Длина разметки в начале строки (отступ + маркер + пробел).
    len: usize,
    /// Длина отступа.
    indent: usize,
    /// Разметка для следующей строки.
    next: String,
}

fn list_prefix(line: &str) -> Option<ListPrefix> {
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let rest = &line[indent..];
    let ws = &line[..indent];

    // Цитата: «> », в том числе вложенная «> > ».
    if rest.starts_with('>') {
        let quote_len = rest
            .char_indices()
            .take_while(|(_, c)| *c == '>' || *c == ' ')
            .map(|(i, c)| i + c.len_utf8())
            .last()
            .unwrap_or(1);
        let mut marker = rest[..quote_len].to_owned();
        if !marker.ends_with(' ') {
            marker.push(' ');
        }
        return Some(ListPrefix {
            len: indent + quote_len,
            indent,
            next: format!("{ws}{marker}"),
        });
    }

    // Маркированный список, в том числе task list.
    for bullet in ["- ", "* ", "+ "] {
        if let Some(after) = rest.strip_prefix(bullet) {
            for task in ["[ ] ", "[x] ", "[X] "] {
                if after.starts_with(task) {
                    return Some(ListPrefix {
                        len: indent + bullet.len() + task.len(),
                        indent,
                        next: format!("{ws}{bullet}[ ] "),
                    });
                }
            }
            return Some(ListPrefix {
                len: indent + bullet.len(),
                indent,
                next: format!("{ws}{bullet}"),
            });
        }
    }

    // Нумерованный: «1. » или «1) ».
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        let delim = rest.as_bytes().get(digits).copied();
        if matches!(delim, Some(b'.') | Some(b')'))
            && rest.as_bytes().get(digits + 1) == Some(&b' ')
        {
            let n: u64 = rest[..digits].parse().ok()?;
            let delim = delim? as char;
            return Some(ListPrefix {
                len: indent + digits + 2,
                indent,
                next: format!("{ws}{}{delim} ", n + 1),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(text: &str, edit: &Edit) -> (String, String) {
        let mut t = text.to_owned();
        edit.apply(&mut t);
        let selected = t[edit.selection.clone()].to_owned();
        (t, selected)
    }

    #[test]
    fn wrap_selection() {
        let e = toggle_wrap("a word b", 2..6, "**");
        assert_eq!(
            apply("a word b", &e),
            ("a **word** b".into(), "word".into())
        );
    }

    #[test]
    fn wrap_empty_selection_puts_cursor_inside() {
        let e = toggle_wrap("ab", 1..1, "`");
        assert_eq!(apply("ab", &e), ("a``b".into(), "".into()));
        assert_eq!(e.selection, 2..2);
    }

    #[test]
    fn unwrap_when_markers_selected() {
        let e = toggle_wrap("a **word** b", 2..10, "**");
        assert_eq!(
            apply("a **word** b", &e),
            ("a word b".into(), "word".into())
        );
    }

    #[test]
    fn unwrap_when_markers_around_selection() {
        let e = toggle_wrap("a **word** b", 4..8, "**");
        assert_eq!(
            apply("a **word** b", &e),
            ("a word b".into(), "word".into())
        );
    }

    #[test]
    fn italic_inside_bold_is_not_unwrapped() {
        // Выделено «word» в «**word**»: Ctrl+I должен добавить курсив, а не снять «*».
        let e = toggle_wrap("**word**", 2..6, "*");
        assert_eq!(apply("**word**", &e).0, "***word***");
    }

    #[test]
    fn wrap_cyrillic() {
        let text = "это слово";
        let start = text.find("слово").unwrap();
        let e = toggle_wrap(text, start..text.len(), "*");
        assert_eq!(apply(text, &e), ("это *слово*".into(), "слово".into()));
    }

    #[test]
    fn wrap_after_multibyte_char() {
        // Перед выделением «ж» (2 байта), маркер «*» (1 байт): проверка «маркер слева»
        // не должна резать символ.
        let text = "жслово";
        let e = toggle_wrap(text, 2..text.len(), "*");
        assert_eq!(apply(text, &e).0, "ж*слово*");
        let e = toggle_wrap(text, 2..2, "**");
        assert_eq!(apply(text, &e).0, "ж****слово");
    }

    #[test]
    fn link_from_text_selects_url() {
        let e = insert_link("see docs", 4..8);
        assert_eq!(
            apply("see docs", &e),
            ("see [docs](url)".into(), "url".into())
        );
    }

    #[test]
    fn link_from_url_selects_text() {
        let text = "https://a.b";
        let e = insert_link(text, 0..text.len());
        assert_eq!(
            apply(text, &e),
            ("[text](https://a.b)".into(), "text".into())
        );
    }

    #[test]
    fn link_without_selection() {
        let e = insert_link("", 0..0);
        assert_eq!(apply("", &e), ("[text](url)".into(), "url".into()));
    }

    fn enter(text: &str) -> Option<String> {
        continue_list(text, text.len()).map(|e| apply(text, &e).0)
    }

    #[test]
    fn continues_bullets_and_tasks() {
        assert_eq!(enter("- item").as_deref(), Some("- item\n- "));
        assert_eq!(enter("  * nested").as_deref(), Some("  * nested\n  * "));
        assert_eq!(enter("- [x] done").as_deref(), Some("- [x] done\n- [ ] "));
    }

    #[test]
    fn continues_numbers() {
        assert_eq!(enter("1. one").as_deref(), Some("1. one\n2. "));
        assert_eq!(enter("9) nine").as_deref(), Some("9) nine\n10) "));
    }

    #[test]
    fn continues_quotes() {
        assert_eq!(enter("> quote").as_deref(), Some("> quote\n> "));
        assert_eq!(enter("> > deep").as_deref(), Some("> > deep\n> > "));
    }

    #[test]
    fn empty_item_ends_list() {
        assert_eq!(enter("- a\n- ").as_deref(), Some("- a\n"));
        assert_eq!(enter("  - ").as_deref(), Some("  "));
        assert_eq!(enter("1. ").as_deref(), Some(""));
    }

    #[test]
    fn plain_lines_are_untouched() {
        assert_eq!(enter("text"), None);
        assert_eq!(enter("-not a list"), None);
        assert_eq!(enter(""), None);
        // По CommonMark «2024. » — пункт нумерованного списка (до 9 цифр).
        assert_eq!(enter("2024. year").as_deref(), Some("2024. year\n2025. "));
    }

    #[test]
    fn middle_of_line_is_untouched() {
        assert_eq!(continue_list("- item text", 4), None);
        // Хвост из пробелов не мешает.
        let text = "- item  ";
        assert!(continue_list(text, 6).is_some());
    }
}
