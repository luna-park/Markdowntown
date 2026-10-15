//! Разбиение документа на сегменты: обычный Markdown и блоки ```` ```mermaid ````.
//!
//! Markdown-сегменты рендерит `egui_commonmark`, mermaid — наш код. Отделяются только
//! блоки верхнего уровня: mermaid внутри списка или цитаты остаётся частью Markdown,
//! иначе разрезанный контейнер отрисовался бы неверно.
//!
//! Ограничение: ссылки-сноски и reference-ссылки не видят определений из других
//! сегментов (каждый сегмент разбирается отдельно).

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentKind {
    Markdown,
    Mermaid { source: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Байтовый диапазон в исходном тексте.
    pub range: Range<usize>,
    /// Номер первой строки (с 0) — для синхронной прокрутки.
    pub first_line: usize,
    pub kind: SegmentKind,
}

/// Те же опции, что у `egui_commonmark`, — чтобы границы блоков совпадали.
fn parser_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_DEFINITION_LIST
}

fn is_mermaid(info: &str) -> bool {
    info.split_whitespace()
        .next()
        .is_some_and(|lang| lang.eq_ignore_ascii_case("mermaid"))
}

pub fn split(text: &str) -> Vec<Segment> {
    // Диапазоны и исходники mermaid-блоков верхнего уровня.
    let mut blocks: Vec<(Range<usize>, String)> = Vec::new();
    let mut depth = 0usize;
    let mut current: Option<(Range<usize>, String)> = None;

    for (event, range) in Parser::new_ext(text, parser_options()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info)))
                if depth == 0 && is_mermaid(&info) =>
            {
                current = Some((range, String::new()));
                depth += 1;
            }
            Event::Text(t) => {
                if let Some((_, src)) = current.as_mut() {
                    src.push_str(&t);
                }
            }
            Event::End(TagEnd::CodeBlock) if current.is_some() && depth == 1 => {
                blocks.extend(current.take());
                depth -= 1;
            }
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
    }

    let mut segments = Vec::with_capacity(blocks.len() * 2 + 1);
    let mut pos = 0;
    let push_markdown = |segments: &mut Vec<Segment>, range: Range<usize>| {
        let chunk = &text[range.clone()];
        if !chunk.trim().is_empty() {
            // Строка первого непустого символа, а не хвоста предыдущего блока.
            let content_start = range.start + (chunk.len() - chunk.trim_start().len());
            segments.push(Segment {
                first_line: line_of(text, content_start),
                range,
                kind: SegmentKind::Markdown,
            });
        }
    };
    for (range, source) in blocks {
        push_markdown(&mut segments, pos..range.start);
        pos = range.end;
        segments.push(Segment {
            first_line: line_of(text, range.start),
            range,
            kind: SegmentKind::Mermaid { source },
        });
    }
    push_markdown(&mut segments, pos..text.len());
    segments
}

fn line_of(text: &str, offset: usize) -> usize {
    text.as_bytes()[..offset]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(&str, usize)> {
        split(text)
            .into_iter()
            .map(|s| {
                let k = match s.kind {
                    SegmentKind::Markdown => "md",
                    SegmentKind::Mermaid { .. } => "mermaid",
                };
                (k, s.first_line)
            })
            .collect()
    }

    #[test]
    fn plain_markdown_is_one_segment() {
        let segs = split("# T\n\ntext\n");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].range, 0..10);
    }

    #[test]
    fn empty_text_has_no_segments() {
        assert!(split("").is_empty());
        assert!(split("\n\n  \n").is_empty());
    }

    #[test]
    fn splits_around_mermaid() {
        let text = "# T\n\n```mermaid\ngraph TD\n  A-->B\n```\n\nafter\n";
        assert_eq!(kinds(text), [("md", 0), ("mermaid", 2), ("md", 7)]);
        let segs = split(text);
        assert_eq!(
            segs[1].kind,
            SegmentKind::Mermaid {
                source: "graph TD\n  A-->B\n".into()
            }
        );
        assert!(text[segs[1].range.clone()].starts_with("```mermaid"));
    }

    #[test]
    fn mermaid_only() {
        assert_eq!(kinds("```mermaid\npie\n```\n"), [("mermaid", 0)]);
    }

    #[test]
    fn info_string_variants() {
        assert_eq!(kinds("~~~Mermaid title\npie\n~~~\n"), [("mermaid", 0)]);
        assert_eq!(kinds("```rust\nfn main() {}\n```\n"), [("md", 0)]);
        assert_eq!(kinds("```mermaidx\npie\n```\n"), [("md", 0)]);
    }

    #[test]
    fn nested_mermaid_stays_markdown() {
        let text = "- item\n\n  ```mermaid\n  pie\n  ```\n\n> ```mermaid\n> pie\n> ```\n";
        assert_eq!(kinds(text), [("md", 0)]);
    }

    #[test]
    fn unclosed_fence_takes_rest_of_document() {
        let text = "a\n\n```mermaid\ngraph TD\n";
        assert_eq!(kinds(text), [("md", 0), ("mermaid", 2)]);
    }

    #[test]
    fn segments_cover_text_in_order() {
        let text = "x\n```mermaid\npie\n```\ny\n```mermaid\npie\n```\n";
        let segs = split(text);
        assert_eq!(segs.len(), 4);
        for w in segs.windows(2) {
            assert!(w[0].range.end <= w[1].range.start);
        }
    }

    #[test]
    fn cyrillic_offsets_are_char_boundaries() {
        let text = "Привет\n\n```mermaid\ngraph TD\n  А-->Б\n```\nПока\n";
        for s in split(text) {
            assert!(text.is_char_boundary(s.range.start));
            assert!(text.is_char_boundary(s.range.end));
        }
    }
}
