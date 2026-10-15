//! Разбиение документа на сегменты для превью: каждый блок верхнего уровня
//! (абзац, заголовок, список, таблица, цитата, …) — отдельный Markdown-сегмент,
//! блок ```` ```mermaid ```` — сегмент-диаграмма.
//!
//! Поблочный рендер нужен, чтобы не рисовать невидимые блоки. Связи между блоками
//! сохраняются через `appendix`: к тексту блока при рендере дописываются определения
//! ссылок (`[ref]: url`) и сносок, на которые он ссылается, — они разбираются вместе
//! с блоком, но не отображаются.
//!
//! Mermaid внутри списка или цитаты остаётся частью своего блока (обычный код).

use std::collections::HashMap;
use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentKind {
    /// `appendix` — невидимый хвост для рендера (определения ссылок и сносок).
    Markdown {
        appendix: String,
    },
    Mermaid {
        source: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Байтовый диапазон в исходном тексте.
    pub range: Range<usize>,
    /// Номер первой строки (с 0).
    pub first_line: usize,
    pub kind: SegmentKind,
}

/// Те же опции, что у `egui_commonmark`, — чтобы границы блоков совпадали.
pub(crate) fn parser_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_DEFINITION_LIST
        | Options::ENABLE_MATH
}

pub(crate) fn is_mermaid(info: &str) -> bool {
    info.split_whitespace()
        .next()
        .is_some_and(|lang| lang.eq_ignore_ascii_case("mermaid"))
}

enum Block {
    Markdown,
    Mermaid(String),
}

pub fn split(text: &str) -> Vec<Segment> {
    let mut blocks: Vec<(Range<usize>, Block)> = Vec::new();
    // Определения сносок верхнего уровня: метка → диапазон.
    let mut footnotes: HashMap<String, Range<usize>> = HashMap::new();
    let mut depth = 0usize;
    let mut current: Option<(usize, Block)> = None;

    let mut events = Parser::new_ext(text, parser_options()).into_offset_iter();
    for (event, range) in events.by_ref() {
        match event {
            Event::Start(tag) => {
                if depth == 0 {
                    let block = match &tag {
                        Tag::CodeBlock(CodeBlockKind::Fenced(info)) if is_mermaid(info) => {
                            Block::Mermaid(String::new())
                        }
                        Tag::FootnoteDefinition(label) => {
                            footnotes.insert(label.to_string(), range.clone());
                            Block::Markdown
                        }
                        _ => Block::Markdown,
                    };
                    current = Some((range.start, block));
                }
                depth += 1;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0
                    && let Some((start, block)) = current.take()
                {
                    blocks.push((start..range.end, block));
                }
            }
            Event::Text(t) => {
                if depth == 1
                    && let Some((_, Block::Mermaid(source))) = current.as_mut()
                {
                    source.push_str(&t);
                }
            }
            // Блоки без вложенности.
            Event::Rule | Event::Html(_) if depth == 0 => {
                blocks.push((range, Block::Markdown));
            }
            _ => {}
        }
    }
    // Определения ссылок событий не дают — берём их исходный текст.
    let link_defs: String = events
        .reference_definitions()
        .iter()
        .map(|(_, def)| text[def.span.clone()].trim_end())
        .collect::<Vec<_>>()
        .join("\n");

    // pulldown-cmark иногда даёт блоку диапазон, захватывающий следующий блок
    // (список определений + абзац после него) — обрезаем по началу следующего.
    for i in 1..blocks.len() {
        let next_start = blocks[i].0.start;
        let prev = &mut blocks[i - 1].0;
        prev.end = prev.end.min(next_start).max(prev.start);
    }

    // Номера строк — одним проходом: блоки идут по возрастанию смещений.
    let mut line = 0;
    let mut counted_to = 0;
    blocks
        .into_iter()
        .map(|(range, block)| {
            line += count_newlines(&text.as_bytes()[counted_to..range.start]);
            counted_to = range.start;
            let kind = match block {
                Block::Mermaid(source) => SegmentKind::Mermaid { source },
                Block::Markdown => SegmentKind::Markdown {
                    appendix: appendix(text, &range, &link_defs, &footnotes),
                },
            };
            Segment {
                first_line: line,
                range,
                kind,
            }
        })
        .collect()
}

/// Невидимый хвост блока: все определения ссылок (если блок может на них ссылаться)
/// и определения сносок, на которые блок ссылается, если они вне блока.
fn appendix(
    text: &str,
    block: &Range<usize>,
    link_defs: &str,
    footnotes: &HashMap<String, Range<usize>>,
) -> String {
    let body = &text[block.clone()];
    let mut parts: Vec<&str> = Vec::new();
    if !link_defs.is_empty() && body.contains(']') {
        parts.push(link_defs);
    }
    for label in footnote_refs(body) {
        if let Some(def) = footnotes.get(label)
            && !(block.start <= def.start && def.end <= block.end)
        {
            parts.push(text[def.clone()].trim_end());
        }
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("\n\n{}\n", parts.join("\n\n"))
    }
}

/// Метки сносок `[^label]` в тексте (возможны лишние кандидаты — это безвредно).
fn footnote_refs(text: &str) -> impl Iterator<Item = &str> {
    text.match_indices("[^").filter_map(|(i, _)| {
        let rest = &text[i + 2..];
        let end = rest.find(']')?;
        let label = &rest[..end];
        (!label.is_empty() && !label.contains(char::is_whitespace)).then_some(label)
    })
}

fn count_newlines(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| b == b'\n').count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(&'static str, usize)> {
        split(text)
            .into_iter()
            .map(|s| {
                let k = match s.kind {
                    SegmentKind::Markdown { .. } => "md",
                    SegmentKind::Mermaid { .. } => "mermaid",
                };
                (k, s.first_line)
            })
            .collect()
    }

    fn appendix_of(text: &str, index: usize) -> String {
        match &split(text)[index].kind {
            SegmentKind::Markdown { appendix } => appendix.clone(),
            SegmentKind::Mermaid { .. } => panic!("not markdown"),
        }
    }

    #[test]
    fn every_top_level_block_is_a_segment() {
        let text = "# T\n\ntext\nmore\n\n- a\n- b\n\n---\n\n| x |\n|---|\n| 1 |\n";
        assert_eq!(
            kinds(text),
            [("md", 0), ("md", 2), ("md", 5), ("md", 8), ("md", 10)]
        );
        let segs = split(text);
        assert_eq!(&text[segs[1].range.clone()], "text\nmore\n");
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
    fn info_string_variants() {
        assert_eq!(kinds("~~~Mermaid title\npie\n~~~\n"), [("mermaid", 0)]);
        assert_eq!(kinds("```rust\nfn main() {}\n```\n"), [("md", 0)]);
        assert_eq!(kinds("```mermaidx\npie\n```\n"), [("md", 0)]);
    }

    #[test]
    fn nested_mermaid_stays_in_its_block() {
        let text = "- item\n\n  ```mermaid\n  pie\n  ```\n\n> ```mermaid\n> pie\n> ```\n";
        assert_eq!(kinds(text), [("md", 0), ("md", 6)]);
    }

    #[test]
    fn unclosed_fence_takes_rest_of_document() {
        let text = "a\n\n```mermaid\ngraph TD\n";
        assert_eq!(kinds(text), [("md", 0), ("mermaid", 2)]);
    }

    #[test]
    fn segments_are_ordered_and_on_char_boundaries() {
        let text = "Привет\n\n```mermaid\ngraph TD\n  А-->Б\n```\nПока\n\n> цитата\n";
        let segs = split(text);
        for w in segs.windows(2) {
            assert!(w[0].range.end <= w[1].range.start);
        }
        for s in &segs {
            assert!(text.is_char_boundary(s.range.start));
            assert!(text.is_char_boundary(s.range.end));
        }
    }

    #[test]
    fn appendix_carries_link_and_footnote_definitions() {
        let text = "See [docs][d] and a note[^n].\n\nOther text.\n\n[d]: https://example.com\n\n[^n]: The note.\n";
        // Первый блок ссылается и на ссылку, и на сноску.
        let a = appendix_of(text, 0);
        assert!(a.contains("[d]: https://example.com"), "{a:?}");
        assert!(a.contains("[^n]: The note."), "{a:?}");
        // Блок без ссылок — без хвоста.
        assert_eq!(appendix_of(text, 1), "");
        // Определение сноски не дублируется в своём же блоке.
        assert!(!appendix_of(text, 2).contains("[^n]"));
    }

    #[test]
    fn overlapping_block_ranges_are_trimmed() {
        // У списка определений pulldown-cmark захватывает и абзац после него.
        let text = "Term
: Def.

Paragraph after.
";
        let segs = split(text);
        assert_eq!(segs.len(), 2);
        assert!(segs[0].range.end <= segs[1].range.start);
        assert!(!text[segs[0].range.clone()].contains("Paragraph"));
    }

    #[test]
    fn footnote_ref_scanner() {
        let refs: Vec<_> = footnote_refs("a[^1] b[^note] c[^ bad] d[^]").collect();
        assert_eq!(refs, ["1", "note"]);
    }
}
