//! Счётчики для строки статуса и переход к строке: слова, время чтения, позиция курсора,
//! начало строки по номеру. Чистые функции над `&str` и байтовыми позициями.

/// Скорость чтения для оценки времени, слов в минуту — обычная оценка для текста
/// с экрана; точность здесь не важна.
const WORDS_PER_MINUTE: usize = 200;

/// Число слов. Слово — буквы и цифры подряд; дефис и апостроф между ними слово не
/// разрывают («кто-то», «don't»). Разметка Markdown словами не считается.
pub fn word_count(text: &str) -> usize {
    let mut count = 0;
    let mut in_word = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_alphanumeric() {
            if !in_word {
                count += 1;
                in_word = true;
            }
        } else {
            let joins = matches!(c, '-' | '\'' | '’')
                && in_word
                && chars.peek().is_some_and(|n| n.is_alphanumeric());
            in_word = joins;
        }
    }
    count
}

/// Минуты чтения `words` слов, с округлением вверх.
pub fn reading_minutes(words: usize) -> usize {
    words.div_ceil(WORDS_PER_MINUTE)
}

/// Строка и столбец позиции `byte`, с 1; столбец — в символах.
pub fn line_col(text: &str, byte: usize) -> (usize, usize) {
    let before = &text[..text.floor_char_boundary(byte)];
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let line = before.bytes().filter(|&b| b == b'\n').count() + 1;
    (line, before[line_start..].chars().count() + 1)
}

/// Число строк (пустой текст — одна строка, как в номерах строк редактора).
pub fn line_count(text: &str) -> usize {
    text.bytes().filter(|&b| b == b'\n').count() + 1
}

/// Начало строки `line` (с 1); номер за концом текста — последняя строка.
pub fn line_start(text: &str, line: usize) -> usize {
    if line <= 1 {
        return 0;
    }
    text.match_indices('\n')
        .nth(line - 2)
        .map_or_else(|| text.rfind('\n').map_or(0, |i| i + 1), |(i, _)| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_words_not_markup() {
        assert_eq!(word_count(""), 0);
        assert_eq!(word_count("# Title\n\n**bold** and `code`"), 4);
        assert_eq!(word_count("- [x] done\n- item 2"), 4);
        assert_eq!(word_count("кто-то don't пол-2"), 3);
        assert_eq!(word_count("a - b -- c"), 3);
        assert_eq!(word_count("ёжик, Ёлка!"), 2);
    }

    #[test]
    fn reading_time_rounds_up() {
        assert_eq!(reading_minutes(0), 0);
        assert_eq!(reading_minutes(1), 1);
        assert_eq!(reading_minutes(200), 1);
        assert_eq!(reading_minutes(201), 2);
    }

    #[test]
    fn line_and_column() {
        let text = "ab\nвгд\n";
        assert_eq!(line_col(text, 0), (1, 1));
        assert_eq!(line_col(text, 2), (1, 3));
        assert_eq!(line_col(text, 3), (2, 1));
        // Столбец — в символах, а не байтах.
        assert_eq!(line_col(text, 7), (2, 3));
        assert_eq!(line_col(text, text.len()), (3, 1));
        // Байт внутри символа — к его началу.
        assert_eq!(line_col(text, 4), (2, 1));
    }

    #[test]
    fn start_of_line_by_number() {
        let text = "a\nbb\nccc";
        assert_eq!(line_count(text), 3);
        assert_eq!(line_start(text, 0), 0);
        assert_eq!(line_start(text, 1), 0);
        assert_eq!(line_start(text, 2), 2);
        assert_eq!(line_start(text, 3), 5);
        assert_eq!(line_start(text, 99), 5);
        assert_eq!(line_start("a\n", 2), 2);
        assert_eq!(line_start("", 5), 0);
    }
}
