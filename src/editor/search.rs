//! Поиск и замена в тексте документа.

use std::ops::Range;

/// Все непересекающиеся вхождения `query` (байтовые диапазоны, по порядку).
/// Без учёта регистра сравнение посимвольное через `to_lowercase`, поэтому
/// работает и для кириллицы.
pub fn find_all(text: &str, query: &str, case_sensitive: bool) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    if case_sensitive {
        return text
            .match_indices(query)
            .map(|(i, m)| i..i + m.len())
            .collect();
    }

    let needle: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut matches = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if let Some(end) = match_at(text, &chars, i, &needle) {
            matches.push(chars[i].0..end);
            // Следующий поиск — после совпадения, без пересечений.
            i = chars.partition_point(|(pos, _)| *pos < end);
        } else {
            i += 1;
        }
    }
    matches
}

/// Совпадает ли `needle` (уже в нижнем регистре) с текстом начиная с символа `start`.
/// Возвращает байтовый конец совпадения.
fn match_at(text: &str, chars: &[(usize, char)], start: usize, needle: &[char]) -> Option<usize> {
    let mut n = 0;
    let mut i = start;
    while n < needle.len() {
        let (_, c) = *chars.get(i)?;
        for lower in c.to_lowercase() {
            if needle.get(n) != Some(&lower) {
                return None;
            }
            n += 1;
        }
        i += 1;
    }
    Some(chars.get(i).map_or(text.len(), |(pos, _)| *pos))
}

/// Заменяет все `matches` (по возрастанию, без пересечений) на `replacement`.
pub fn replace_all(text: &str, matches: &[Range<usize>], replacement: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for m in matches {
        out.push_str(&text[pos..m.start]);
        out.push_str(replacement);
        pos = m.end;
    }
    out.push_str(&text[pos..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_case_sensitive() {
        assert_eq!(find_all("aXa xa", "a", true), [0..1, 2..3, 5..6]);
        assert_eq!(find_all("aaaa", "aa", true), [0..2, 2..4]);
        assert!(find_all("abc", "", true).is_empty());
    }

    #[test]
    fn finds_case_insensitive_cyrillic() {
        let text = "Привет, привет, ПРИВЕТ";
        let found = find_all(text, "привет", false);
        assert_eq!(found.len(), 3);
        for r in &found {
            assert_eq!(text[r.clone()].to_lowercase(), "привет");
        }
        assert_eq!(find_all(text, "привет", true).len(), 1);
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)] // один диапазон, а не диапазон чисел
    fn match_at_end_of_text() {
        assert_eq!(find_all("abC", "c", false), [2..3]);
    }

    #[test]
    fn replaces_all() {
        let text = "one two one";
        let m = find_all(text, "one", true);
        assert_eq!(replace_all(text, &m, "1"), "1 two 1");
        assert_eq!(replace_all(text, &[], "x"), text);
    }
}
