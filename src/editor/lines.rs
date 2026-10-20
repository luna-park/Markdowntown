//! Команды над строками: отступ (Tab / Shift+Tab), перемещение, дублирование и удаление
//! строк, заголовки, флажки задач. Чистые функции над `&str` и байтовыми диапазонами;
//! действуют на все строки, которых касается выделение.

use std::ops::Range;

use super::commands::{Edit, ListPrefix, PrefixKind, list_prefix};

/// Отступ строк не из списка (Tab на нескольких строках), пробелов.
const INDENT: usize = 4;

/// Строки, которых касается выделение: от начала первой до конца последней (без `\n`).
/// Выделение, которое кончается в начале строки, эту строку не захватывает — как в VS Code.
fn line_block(text: &str, sel: &Range<usize>) -> Range<usize> {
    let end = if sel.end > sel.start && text[..sel.end].ends_with('\n') {
        sel.end - 1
    } else {
        sel.end
    };
    line_start(text, sel.start)..line_end(text, end)
}

fn line_start(text: &str, pos: usize) -> usize {
    text[..pos].rfind('\n').map_or(0, |i| i + 1)
}

fn line_end(text: &str, pos: usize) -> usize {
    text[pos..].find('\n').map_or(text.len(), |i| pos + i)
}

/// Выделение захватывает перенос строки — Tab и Shift+Tab сдвигают строки целиком.
pub fn is_multiline(text: &str, sel: &Range<usize>) -> bool {
    text[sel.clone()].contains('\n')
}

/// Пункт маркированного или нумерованного списка (не цитата).
fn list_item(line: &str) -> Option<ListPrefix> {
    list_prefix(line).filter(|p| p.kind != PrefixKind::Quote)
}

/// Строки над строкой, начинающейся с `start`, снизу вверх — до пустой строки.
fn lines_above(text: &str, start: usize) -> impl Iterator<Item = &str> {
    text[..start.saturating_sub(1)]
        .rsplit('\n')
        .take_while(|line| !line.trim().is_empty())
}

/// Замена в строке: диапазон внутри строки и чем его заменить.
type LineChange = (Range<usize>, String);

/// Правит каждую строку блока: `change` по тексту строки говорит, что в ней заменить.
/// Выделение сдвигается вместе с текстом. `None` — ни одна строка не изменилась.
fn edit_lines(
    text: &str,
    sel: Range<usize>,
    mut change: impl FnMut(&str) -> Option<LineChange>,
) -> Option<Edit> {
    let block = line_block(text, &sel);
    let mut replacement = String::with_capacity(block.len() + 16);
    // (начало заменённого куска в тексте, его старая длина, новая длина)
    let mut changes = Vec::new();
    let mut pos = block.start;
    for (i, line) in text[block.clone()].split('\n').enumerate() {
        if i > 0 {
            replacement.push('\n');
        }
        match change(line) {
            Some((range, new)) => {
                replacement.push_str(&line[..range.start]);
                replacement.push_str(&new);
                replacement.push_str(&line[range.end..]);
                changes.push((pos + range.start, range.len(), new.len()));
            }
            None => replacement.push_str(line),
        }
        pos += line.len() + 1;
    }
    if changes.is_empty() {
        return None;
    }
    let selection = shift_position(sel.start, &changes)..shift_position(sel.end, &changes);
    Some(Edit {
        range: block,
        replacement,
        selection,
    })
}

/// Позиция `p` после замен `changes` (по возрастанию): вставка в точке `p` сдвигает её,
/// а позиция внутри заменённого куска остаётся не дальше его нового конца.
fn shift_position(p: usize, changes: &[(usize, usize, usize)]) -> usize {
    let mut shift = 0isize;
    for &(start, old, new) in changes {
        if p >= start + old {
            shift += new as isize - old as isize;
        } else {
            if p > start {
                return start.saturating_add_signed(shift) + (p - start).min(new);
            }
            break;
        }
    }
    p.saturating_add_signed(shift)
}

/// Tab: пункт списка сдвигается под содержимое предыдущего пункта того же уровня (так
/// он становится вложенным), выделенные строки — на `INDENT` пробелов. `None` — строка
/// не из списка без выделенного переноса: обычная табуляция.
pub fn indent(text: &str, sel: Range<usize>) -> Option<Edit> {
    let block = line_block(text, &sel);
    let first = &text[block.start..line_end(text, block.start)];
    let width = match list_item(first) {
        Some(item) => {
            // Ближайший пункт выше с тем же отступом — соседний; с меньшим — родитель,
            // соседа нет.
            let sibling = lines_above(text, block.start)
                .filter_map(list_item)
                .find(|p| p.indent <= item.indent)
                .filter(|p| p.indent == item.indent);
            sibling.map_or(item.marker_end, |p| p.marker_end) - item.indent
        }
        None if is_multiline(text, &sel) => INDENT,
        None => return None,
    };
    let pad = " ".repeat(width);
    edit_lines(text, sel, |line| {
        (!line.trim().is_empty()).then(|| (0..0, pad.clone()))
    })
}

/// Shift+Tab: пункт списка — на уровень родителя, выделенные строки — влево на `INDENT`
/// пробелов (или одну табуляцию). `None` — двигать нечего или строка не из списка без
/// выделенного переноса.
pub fn outdent(text: &str, sel: Range<usize>) -> Option<Edit> {
    let block = line_block(text, &sel);
    let first = &text[block.start..line_end(text, block.start)];
    let width = match list_item(first) {
        Some(item) if item.indent > 0 => {
            let parent = lines_above(text, block.start)
                .filter_map(list_item)
                .find(|p| p.indent < item.indent);
            item.indent - parent.map_or(0, |p| p.indent)
        }
        Some(_) => return None,
        None if is_multiline(text, &sel) => INDENT,
        None => return None,
    };
    edit_lines(text, sel, |line| {
        let n = if line.starts_with('\t') {
            1
        } else {
            line.bytes().take(width).take_while(|&b| b == b' ').count()
        };
        (n > 0).then(|| (0..n, String::new()))
    })
}

/// Alt+↑ / Alt+↓: поменять строки выделения местами с соседней строкой.
/// `None` — двигать некуда.
pub fn move_lines(text: &str, sel: Range<usize>, up: bool) -> Option<Edit> {
    let block = line_block(text, &sel);
    let lines = &text[block.clone()];
    let (range, replacement, shift) = if up {
        if block.start == 0 {
            return None;
        }
        let above_start = line_start(text, block.start - 1);
        let above = &text[above_start..block.start - 1];
        let shift = -(above.len() as isize + 1);
        (above_start..block.end, format!("{lines}\n{above}"), shift)
    } else {
        if block.end == text.len() {
            return None;
        }
        let below_end = line_end(text, block.end + 1);
        let below = &text[block.end + 1..below_end];
        let shift = below.len() as isize + 1;
        (block.start..below_end, format!("{below}\n{lines}"), shift)
    };
    // Длина текста не меняется; конец выделения в начале следующей строки (которая
    // не двигается) может оказаться за концом текста.
    let moved = |p: usize| p.saturating_add_signed(shift).min(text.len());
    Some(Edit {
        range,
        replacement,
        selection: moved(sel.start)..moved(sel.end),
    })
}

/// Ctrl+Shift+D: повторить строки выделения под ними; выделение — на копии.
pub fn duplicate_lines(text: &str, sel: Range<usize>) -> Edit {
    let block = line_block(text, &sel);
    let lines = &text[block.clone()];
    let shift = lines.len() + 1;
    Edit {
        range: block.end..block.end,
        replacement: format!("\n{lines}"),
        selection: sel.start + shift..sel.end + shift,
    }
}

/// Ctrl+Shift+K: удалить строки выделения; курсор — в начало строки, вставшей на их место
/// (у последней строки документа — в начало предыдущей).
pub fn delete_lines(text: &str, sel: Range<usize>) -> Edit {
    let block = line_block(text, &sel);
    let (range, cursor) = if block.end < text.len() {
        (block.start..block.end + 1, block.start)
    } else if block.start > 0 {
        let above = line_start(text, block.start - 1);
        (block.start - 1..block.end, above)
    } else {
        (block, 0)
    };
    Edit {
        range,
        replacement: String::new(),
        selection: cursor..cursor,
    }
}

/// Ctrl+1…6: строки — заголовками уровня `level`; если первая строка уже заголовок
/// этого уровня — обычным текстом.
pub fn set_heading(text: &str, sel: Range<usize>, level: usize) -> Option<Edit> {
    let block = line_block(text, &sel);
    let first = &text[block.start..line_end(text, block.start)];
    let off = heading_prefix(first).is_some_and(|(_, l)| l == level);
    let marker = if off {
        String::new()
    } else {
        format!("{} ", "#".repeat(level))
    };
    edit_lines(text, sel, |line| {
        if line.trim().is_empty() {
            return None;
        }
        let len = heading_prefix(line).map_or(0, |(len, _)| len);
        (line[..len] != marker).then(|| (0..len, marker.clone()))
    })
}

/// Разметка заголовка ATX в начале строки: (длина вместе с пробелом после `#`, уровень).
fn heading_prefix(line: &str) -> Option<(usize, usize)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let level = rest.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    match rest.as_bytes().get(level) {
        None => Some((indent + level, level)),
        Some(b' ' | b'\t') => Some((indent + level + 1, level)),
        _ => None,
    }
}

/// Ctrl+Enter: отметить задачи или снять отметку — как сделать с первым пунктом
/// выделения; пункт списка без флажка становится задачей.
pub fn toggle_task(text: &str, sel: Range<usize>) -> Option<Edit> {
    #[derive(Clone, Copy)]
    enum Mode {
        Check,
        Uncheck,
        Add,
    }
    let block = line_block(text, &sel);
    let mode = text[block]
        .split('\n')
        .find_map(|line| list_item(line).map(|item| (line, item)))
        .map(|(line, item)| match item.task {
            None => Mode::Add,
            Some(p) if line.as_bytes()[p + 1] == b' ' => Mode::Check,
            Some(_) => Mode::Uncheck,
        })?;
    edit_lines(text, sel, |line| {
        let item = list_item(line)?;
        match (mode, item.task) {
            (Mode::Add, None) => Some((item.marker_end..item.marker_end, "[ ] ".into())),
            (Mode::Check, Some(p)) => Some((p..p + 3, "[x]".into())),
            (Mode::Uncheck, Some(p)) => Some((p..p + 3, "[ ]".into())),
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Текст с выделением: `|` — курсор, `«…»` — выделение.
    fn parse(s: &str) -> (String, Range<usize>) {
        if let Some(i) = s.find('|') {
            return (s.replacen('|', "", 1), i..i);
        }
        let start = s.find('«').expect("selection start");
        let text = s.replacen('«', "", 1);
        let end = text.find('»').expect("selection end");
        (text.replacen('»', "", 1), start..end)
    }

    fn show(text: &str, sel: Range<usize>) -> String {
        if sel.is_empty() {
            format!("{}|{}", &text[..sel.start], &text[sel.start..])
        } else {
            let (a, b) = (sel.start, sel.end);
            format!("{}«{}»{}", &text[..a], &text[a..b], &text[b..])
        }
    }

    fn run(command: impl Fn(&str, Range<usize>) -> Option<Edit>, s: &str) -> Option<String> {
        let (text, sel) = parse(s);
        command(&text, sel).map(|edit| {
            let mut result = text.clone();
            edit.apply(&mut result);
            show(&result, edit.selection)
        })
    }

    #[test]
    fn tab_nests_list_item_under_previous_sibling() {
        assert_eq!(run(indent, "- a\n- |b").as_deref(), Some("- a\n  - |b"));
        assert_eq!(
            run(indent, "1. a\n2. |b").as_deref(),
            Some("1. a\n   2. |b")
        );
        // Под нумерованным пунктом — на ширину его маркера, иначе не вложится.
        assert_eq!(run(indent, "1. a\n- |b").as_deref(), Some("1. a\n   - |b"));
        assert_eq!(
            run(indent, "- a\n  - b\n  - |c").as_deref(),
            Some("- a\n  - b\n    - |c")
        );
        // Продолжение пункта между ними не мешает найти соседа.
        assert_eq!(
            run(indent, "10. a\n    more\n11. |b").as_deref(),
            Some("10. a\n    more\n    11. |b")
        );
        // Соседа нет — на ширину своего маркера.
        assert_eq!(run(indent, "- |a").as_deref(), Some("  - |a"));
        assert_eq!(run(indent, "- [ ] |a").as_deref(), Some("  - [ ] |a"));
    }

    #[test]
    fn tab_on_plain_line_is_left_to_text_edit() {
        assert_eq!(run(indent, "te|xt"), None);
        assert_eq!(run(indent, "> quo|te"), None);
        assert_eq!(run(outdent, "te|xt"), None);
    }

    #[test]
    fn tab_indents_selected_lines_except_blank() {
        assert_eq!(run(indent, "«a\n\nb»").as_deref(), Some("    «a\n\n    b»"));
        // Выделение до начала следующей строки её не захватывает.
        assert_eq!(run(indent, "«a\n»b").as_deref(), Some("    «a\n»b"));
    }

    #[test]
    fn shift_tab_moves_list_item_to_parent_level() {
        assert_eq!(run(outdent, "- a\n  - |b").as_deref(), Some("- a\n- |b"));
        assert_eq!(run(outdent, "1. a\n   - |b").as_deref(), Some("1. a\n- |b"));
        assert_eq!(
            run(outdent, "- a\n  - b\n    - |c").as_deref(),
            Some("- a\n  - b\n  - |c")
        );
        assert_eq!(run(outdent, "- |a"), None);
    }

    #[test]
    fn shift_tab_outdents_selected_lines() {
        assert_eq!(run(outdent, "«    a\n  b»").as_deref(), Some("«a\nb»"));
        assert_eq!(run(outdent, "«\ta\n      b»").as_deref(), Some("«a\n  b»"));
        assert_eq!(run(outdent, "«a\nb»"), None);
    }

    #[test]
    fn move_lines_up_and_down() {
        let up = |t: &str, s| move_lines(t, s, true);
        let down = |t: &str, s| move_lines(t, s, false);
        assert_eq!(run(up, "a\nb|\nc").as_deref(), Some("b|\na\nc"));
        assert_eq!(run(down, "a\nb|\nc").as_deref(), Some("a\nc\nb|"));
        assert_eq!(run(down, "«a\nb»\nc").as_deref(), Some("c\n«a\nb»"));
        assert_eq!(run(up, "a\n«b\nc»").as_deref(), Some("«b\nc»\na"));
        assert_eq!(run(up, "a|\nb"), None);
        assert_eq!(run(down, "a\nb|"), None);
        // Конец выделения в начале следующей строки: двигается только «a».
        assert_eq!(run(down, "«a\n»b").as_deref(), Some("b\n«a»"));
        assert_eq!(run(down, "абв|\nгд").as_deref(), Some("гд\nабв|"));
    }

    #[test]
    fn duplicate_puts_selection_on_copy() {
        assert_eq!(
            run(|t, s| Some(duplicate_lines(t, s)), "a|\nb").as_deref(),
            Some("a\na|\nb")
        );
        assert_eq!(
            run(|t, s| Some(duplicate_lines(t, s)), "x\n«a\nb»").as_deref(),
            Some("x\na\nb\n«a\nb»")
        );
    }

    #[test]
    fn delete_removes_whole_lines() {
        let delete = |t: &str, s| Some(delete_lines(t, s));
        assert_eq!(run(delete, "a\nb|\nc").as_deref(), Some("a\n|c"));
        assert_eq!(run(delete, "a\nb|").as_deref(), Some("|a"));
        assert_eq!(run(delete, "a|").as_deref(), Some("|"));
        assert_eq!(run(delete, "«a\nb»\nc").as_deref(), Some("|c"));
    }

    #[test]
    fn headings_set_change_and_toggle_off() {
        let h = |level| move |t: &str, s| set_heading(t, s, level);
        assert_eq!(run(h(2), "Ti|tle").as_deref(), Some("## Ti|tle"));
        assert_eq!(run(h(3), "## Ti|tle").as_deref(), Some("### Ti|tle"));
        assert_eq!(run(h(2), "## Ti|tle").as_deref(), Some("Ti|tle"));
        assert_eq!(run(h(1), "|#tag").as_deref(), Some("# |#tag"));
        assert_eq!(run(h(1), "«a\n\n## b»").as_deref(), Some("# «a\n\n# b»"));
        assert_eq!(run(h(1), "|").as_deref(), None);
    }

    #[test]
    fn ctrl_enter_toggles_tasks() {
        assert_eq!(run(toggle_task, "- [ ] a|").as_deref(), Some("- [x] a|"));
        assert_eq!(run(toggle_task, "- [X] a|").as_deref(), Some("- [ ] a|"));
        assert_eq!(run(toggle_task, "1. [ ] a|").as_deref(), Some("1. [x] a|"));
        assert_eq!(run(toggle_task, "- a|").as_deref(), Some("- [ ] a|"));
        assert_eq!(run(toggle_task, "  2. a|").as_deref(), Some("  2. [ ] a|"));
        assert_eq!(run(toggle_task, "te|xt"), None);
        // Несколько строк — как у первой задачи.
        assert_eq!(
            run(toggle_task, "«- [ ] a\n- [x] b\n- c»").as_deref(),
            Some("«- [x] a\n- [x] b\n- c»")
        );
    }
}
