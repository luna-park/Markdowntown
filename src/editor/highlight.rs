//! Подсветка синтаксиса Markdown в редакторе: текст → `LayoutJob` с секциями.
//!
//! Каждый байт размечается флагами по событиям `pulldown-cmark` (в каком элементе
//! он находится и является ли содержимым или разметкой), затем соседние байты
//! с одинаковым стилем склеиваются в секции.

use std::ops::Range;

use egui::text::{LayoutJob, LayoutSection, TextFormat};
use egui::{Color32, FontId, Stroke};
use pulldown_cmark::{Event, Options, Parser, Tag};

const CONTENT: u16 = 1 << 0;
const HEADING: u16 = 1 << 1;
const STRONG: u16 = 1 << 2;
const EMPH: u16 = 1 << 3;
const STRIKE: u16 = 1 << 4;
const CODE: u16 = 1 << 5;
const CODE_BLOCK: u16 = 1 << 6;
const LINK: u16 = 1 << 7;
const QUOTE: u16 = 1 << 8;
const ITEM: u16 = 1 << 9;
const TABLE: u16 = 1 << 10;
const TASK: u16 = 1 << 11;
const HTML: u16 = 1 << 12;
/// Элементы, у которых символы вне содержимого — разметка.
const MARKUP_CONTAINERS: u16 = HEADING | STRONG | EMPH | STRIKE | LINK | QUOTE | ITEM | TABLE;

/// Цвета подсветки.
#[derive(Clone, Copy, PartialEq)]
pub struct Palette {
    pub text: Color32,
    pub markup: Color32,
    pub heading: Color32,
    pub strong: Color32,
    pub code: Color32,
    pub code_bg: Color32,
    pub link: Color32,
    pub quote: Color32,
    pub list_marker: Color32,
    pub match_bg: Color32,
    pub current_match_bg: Color32,
}

impl Palette {
    pub fn from_visuals(v: &egui::Visuals) -> Self {
        if v.dark_mode {
            Self {
                text: crate::style::reading_text_color(true),
                markup: Color32::from_rgb(0x80, 0x80, 0x80),
                heading: Color32::from_rgb(0x6c, 0xb6, 0xff),
                strong: v.strong_text_color(),
                code: Color32::from_rgb(0xe0, 0x9c, 0x6c),
                code_bg: Color32::from_rgb(0x26, 0x26, 0x26),
                link: Color32::from_rgb(0x4e, 0xc9, 0xb0),
                quote: Color32::from_rgb(0x8f, 0xb5, 0x73),
                list_marker: Color32::from_rgb(0xd7, 0xba, 0x7d),
                match_bg: Color32::from_rgba_unmultiplied(0xff, 0xd7, 0x00, 0x40),
                current_match_bg: Color32::from_rgba_unmultiplied(0xff, 0x8c, 0x00, 0x90),
            }
        } else {
            Self {
                text: crate::style::reading_text_color(false),
                markup: Color32::from_rgb(0x8a, 0x8a, 0x8a),
                heading: Color32::from_rgb(0x05, 0x50, 0xae),
                strong: v.strong_text_color(),
                code: Color32::from_rgb(0xa3, 0x40, 0x15),
                code_bg: Color32::from_rgb(0xee, 0xee, 0xee),
                link: Color32::from_rgb(0x0a, 0x7a, 0x6a),
                quote: Color32::from_rgb(0x3f, 0x7a, 0x2a),
                list_marker: Color32::from_rgb(0x95, 0x6b, 0x00),
                match_bg: Color32::from_rgba_unmultiplied(0xff, 0xd7, 0x00, 0x70),
                current_match_bg: Color32::from_rgba_unmultiplied(0xff, 0x8c, 0x00, 0xb0),
            }
        }
    }
}

fn parser_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_DEFINITION_LIST
}

/// Флаги для каждого байта текста.
fn classify(text: &str) -> Vec<u16> {
    let mut flags = vec![0u16; text.len()];
    let mut mark = |range: Range<usize>, flag: u16| {
        for f in &mut flags[range] {
            *f |= flag;
        }
    };
    for (event, range) in Parser::new_ext(text, parser_options()).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                let flag = match tag {
                    Tag::Heading { .. } => HEADING,
                    Tag::Strong => STRONG,
                    Tag::Emphasis => EMPH,
                    Tag::Strikethrough => STRIKE,
                    Tag::CodeBlock(_) => CODE_BLOCK,
                    Tag::Link { .. } | Tag::Image { .. } => LINK,
                    Tag::BlockQuote(_) => QUOTE,
                    Tag::Item => ITEM,
                    Tag::Table(_) => TABLE,
                    Tag::HtmlBlock => HTML,
                    _ => 0,
                };
                if flag != 0 {
                    mark(range, flag);
                }
            }
            Event::Text(_) => mark(range, CONTENT),
            Event::Code(_) => mark(range, CONTENT | CODE),
            Event::TaskListMarker(_) => mark(range, TASK),
            Event::Html(_) | Event::InlineHtml(_) => mark(range, HTML),
            _ => {}
        }
    }
    flags
}

/// Стиль байта: индекс в небольшом наборе форматов, чтобы склеивать секции.
#[derive(Clone, Copy, PartialEq, Eq)]
struct StyleKey {
    flags: u16,
    /// 0 — нет, 1 — совпадение поиска, 2 — текущее совпадение.
    search: u8,
}

fn format_for(key: StyleKey, font: &FontId, p: &Palette) -> TextFormat {
    let f = key.flags;
    let content = f & CONTENT != 0;
    let mut fmt = TextFormat {
        font_id: font.clone(),
        color: p.text,
        ..Default::default()
    };

    if f & CODE_BLOCK != 0 {
        fmt.color = if content { p.code } else { p.markup };
        fmt.background = p.code_bg;
    } else if f & CODE != 0 {
        fmt.color = p.code;
        fmt.background = p.code_bg;
    } else if f & HTML != 0 {
        fmt.color = p.markup;
    } else if f & TASK != 0 {
        fmt.color = p.list_marker;
    } else if !content && f & MARKUP_CONTAINERS != 0 {
        // Символы разметки: «#», «**», «[»…«](url)», «>», «|».
        fmt.color = if f & ITEM != 0 && f & (HEADING | STRONG | EMPH | LINK | QUOTE) == 0 {
            p.list_marker
        } else {
            p.markup
        };
    } else {
        if f & HEADING != 0 {
            fmt.color = p.heading;
        } else if f & LINK != 0 {
            fmt.color = p.link;
            fmt.underline = Stroke::new(1.0, p.link);
        } else if f & STRONG != 0 {
            fmt.color = p.strong;
        } else if f & QUOTE != 0 {
            fmt.color = p.quote;
        }
        if f & EMPH != 0 {
            fmt.italics = true;
        }
        if f & STRIKE != 0 {
            fmt.strikethrough = Stroke::new(1.0, fmt.color);
        }
    }

    match key.search {
        1 => fmt.background = p.match_bg,
        2 => fmt.background = p.current_match_bg,
        _ => {}
    }
    fmt
}

/// Строит `LayoutJob` для текста. `matches` — найденные совпадения (по возрастанию),
/// `current` — индекс текущего.
pub fn layout_job(
    text: &str,
    font: &FontId,
    palette: &Palette,
    matches: &[Range<usize>],
    current: Option<usize>,
) -> LayoutJob {
    let flags = classify(text);
    let mut search = vec![0u8; text.len()];
    for (i, m) in matches.iter().enumerate() {
        let v = if Some(i) == current { 2 } else { 1 };
        if let Some(slice) = search.get_mut(m.clone()) {
            slice.fill(v);
        }
    }

    let mut job = LayoutJob {
        text: text.to_owned(),
        ..Default::default()
    };
    let key_at = |i: usize| StyleKey {
        flags: flags[i],
        search: search[i],
    };
    let mut start = 0;
    while start < text.len() {
        let key = key_at(start);
        let mut end = start + 1;
        while end < text.len() && key_at(end) == key {
            end += 1;
        }
        // Границы стилей совпадают с границами событий парсера, то есть символов,
        // но на всякий случай не режем символ посередине.
        while !text.is_char_boundary(end) {
            end += 1;
        }
        job.sections.push(LayoutSection {
            leading_space: 0.0,
            byte_range: start.into()..end.into(),
            format: format_for(key, font, palette),
        });
        start = end;
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section_at(job: &LayoutJob, pos: usize) -> &LayoutSection {
        job.sections
            .iter()
            .find(|s| usize::from(s.byte_range.start) <= pos && pos < usize::from(s.byte_range.end))
            .unwrap()
    }

    fn color_of(text: &str, needle: &str) -> Color32 {
        let p = Palette::from_visuals(&egui::Visuals::dark());
        let job = layout_job(text, &FontId::monospace(12.0), &p, &[], None);
        let pos = text.find(needle).unwrap();
        section_at(&job, pos).format.color
    }

    #[test]
    fn heading_text_and_marker() {
        let p = Palette::from_visuals(&egui::Visuals::dark());
        assert_eq!(color_of("# Title", "Title"), p.heading);
        assert_eq!(color_of("# Title", "#"), p.markup);
    }

    #[test]
    fn inline_elements() {
        let p = Palette::from_visuals(&egui::Visuals::dark());
        let text = "a **bold** `code` [link](http://x)";
        assert_eq!(color_of(text, "bold"), p.strong);
        assert_eq!(color_of(text, "**"), p.markup);
        assert_eq!(color_of(text, "code"), p.code);
        assert_eq!(color_of(text, "link"), p.link);
        assert_eq!(color_of(text, "http"), p.markup);
        assert_eq!(color_of(text, "a "), p.text);
    }

    #[test]
    fn list_markers_and_code_blocks() {
        let p = Palette::from_visuals(&egui::Visuals::dark());
        assert_eq!(color_of("- item", "-"), p.list_marker);
        assert_eq!(color_of("- item", "item"), p.text);
        let block = "```rust\nfn main() {}\n```\n";
        assert_eq!(color_of(block, "fn"), p.code);
        assert_eq!(color_of(block, "```"), p.markup);
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)] // один диапазон, а не диапазон чисел
    fn sections_cover_text_without_gaps() {
        let text = "# Заголовок\n\n- [x] пункт **жирный**\n\n> цитата\n";
        let p = Palette::from_visuals(&egui::Visuals::dark());
        let job = layout_job(text, &FontId::monospace(12.0), &p, &[3..9], Some(0));
        let mut pos = 0;
        for s in &job.sections {
            assert_eq!(usize::from(s.byte_range.start), pos);
            pos = s.byte_range.end.into();
            assert!(text.is_char_boundary(pos));
        }
        assert_eq!(pos, text.len());
    }

    #[test]
    fn search_matches_get_background() {
        let p = Palette::from_visuals(&egui::Visuals::dark());
        let job = layout_job(
            "abc abc",
            &FontId::monospace(12.0),
            &p,
            &[0..3, 4..7],
            Some(1),
        );
        let bg = |pos: usize| section_at(&job, pos).format.background;
        assert_eq!(bg(0), p.match_bg);
        assert_eq!(bg(5), p.current_match_bg);
    }
}
