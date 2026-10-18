//! Язык интерфейса: английский и русский. Строки — прямо в коде, парой
//! `tr!("English", "Русский")` (или `trf!` — с форматированием); язык выбирается
//! в View → Language и хранится в `Settings::language`.
//!
//! Текущий язык — глобальный: его читают и фоновые потоки (экспорт HTML). Тесты язык
//! не переключают — проверяют чистые функции, иначе мешали бы параллельным тестам.

use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Language {
    #[default]
    English,
    Russian,
}

impl Language {
    pub const ALL: [Self; 2] = [Self::English, Self::Russian];

    /// Название на самом языке — так его узнают при любом текущем языке интерфейса.
    pub fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Russian => "Русский",
        }
    }

    /// Язык интерфейса Windows: русский — по-русски, остальные — по-английски.
    pub fn system() -> Self {
        #[cfg(windows)]
        {
            // SAFETY: функция без аргументов, только читает настройку пользователя.
            let lang_id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
            Self::from_lang_id(lang_id)
        }
        #[cfg(not(windows))]
        Self::English
    }

    /// По LANGID Windows: младшие 10 бит — основной язык (`LANG_RUSSIAN` = 0x19).
    fn from_lang_id(lang_id: u16) -> Self {
        const LANG_RUSSIAN: u16 = 0x19;
        if lang_id & 0x3ff == LANG_RUSSIAN {
            Self::Russian
        } else {
            Self::English
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn language() -> Language {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Language::Russian,
        _ => Language::English,
    }
}

pub fn set_language(language: Language) {
    CURRENT.store(language as u8, Ordering::Relaxed);
}

/// Строка на текущем языке: `tr!("Save", "Сохранить")`.
macro_rules! tr {
    ($en:expr, $ru:expr $(,)?) => {
        match $crate::i18n::language() {
            $crate::i18n::Language::English => $en,
            $crate::i18n::Language::Russian => $ru,
        }
    };
}

/// `format!` на текущем языке: `trf!("Opened {}", "Открыт {}", path.display())`.
macro_rules! trf {
    ($en:literal, $ru:literal $(, $arg:expr)* $(,)?) => {
        match $crate::i18n::language() {
            $crate::i18n::Language::English => format!($en $(, $arg)*),
            $crate::i18n::Language::Russian => format!($ru $(, $arg)*),
        }
    };
}

/// Сообщение для строки статуса: `msg!("Saved {}", "Сохранён {}", path.display())`.
macro_rules! msg {
    ($en:literal, $ru:literal $(, $arg:expr)* $(,)?) => {{
        let english = format!($en $(, $arg)*);
        let text = match $crate::i18n::language() {
            $crate::i18n::Language::English => english.clone(),
            $crate::i18n::Language::Russian => format!($ru $(, $arg)*),
        };
        $crate::i18n::Message { english, text }
    }};
}

pub(crate) use {msg, tr, trf};

/// Сообщение пользователю: `text` — на языке интерфейса, `english` — для журнала
/// (журнал всегда по-английски).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub english: String,
    pub text: String,
}

/// Русская форма слова для числа `n`: `[минута, минуты, минут]`.
pub fn plural_ru(n: u64, forms: [&'static str; 3]) -> &'static str {
    let (last, last_two) = (n % 10, n % 100);
    if last == 1 && last_two != 11 {
        forms[0]
    } else if (2..=4).contains(&last) && !(12..=14).contains(&last_two) {
        forms[1]
    } else {
        forms[2]
    }
}

/// «`n` слов» на текущем языке: `count(3, ["minute", "minutes"], ["минута", "минуты", "минут"])`.
pub fn count(n: u64, en: [&'static str; 2], ru: [&'static str; 3]) -> String {
    count_in(language(), n, en, ru)
}

/// «`n` слов» на языке `language`.
pub fn count_in(
    language: Language,
    n: u64,
    en: [&'static str; 2],
    ru: [&'static str; 3],
) -> String {
    let word = match language {
        Language::English => en[usize::from(n != 1)],
        Language::Russian => plural_ru(n, ru),
    };
    format!("{n} {word}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn russian_plural_forms() {
        let forms = ["минута", "минуты", "минут"];
        let cases = [
            (0, "минут"),
            (1, "минута"),
            (2, "минуты"),
            (4, "минуты"),
            (5, "минут"),
            (11, "минут"),
            (12, "минут"),
            (14, "минут"),
            (21, "минута"),
            (22, "минуты"),
            (25, "минут"),
            (101, "минута"),
            (111, "минут"),
            (112, "минут"),
        ];
        for (n, expected) in cases {
            assert_eq!(plural_ru(n, forms), expected, "{n}");
        }
    }

    #[test]
    fn counts_in_both_languages() {
        let en = ["image not found", "images not found"];
        let ru = [
            "картинка не найдена",
            "картинки не найдены",
            "картинок не найдено",
        ];
        assert_eq!(count_in(Language::English, 1, en, ru), "1 image not found");
        assert_eq!(count_in(Language::English, 3, en, ru), "3 images not found");
        assert_eq!(
            count_in(Language::Russian, 3, en, ru),
            "3 картинки не найдены"
        );
        assert_eq!(
            count_in(Language::Russian, 5, en, ru),
            "5 картинок не найдено"
        );
    }

    #[test]
    fn language_from_windows_lang_id() {
        // ru-RU (0x0419), ru-UA (0x0819), en-US (0x0409), uk-UA (0x0422).
        assert_eq!(Language::from_lang_id(0x0419), Language::Russian);
        assert_eq!(Language::from_lang_id(0x0819), Language::Russian);
        assert_eq!(Language::from_lang_id(0x0409), Language::English);
        assert_eq!(Language::from_lang_id(0x0422), Language::English);
    }

    #[test]
    fn language_is_stored_by_name() {
        let json = serde_json::to_string(&Language::Russian).unwrap();
        assert_eq!(json, "\"Russian\"");
        // Порядок вариантов — значения в `CURRENT`.
        assert_eq!(Language::English as u8, 0);
        assert_eq!(Language::Russian as u8, 1);
    }
}
