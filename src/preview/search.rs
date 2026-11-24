//! Поиск по тексту Markdown в порядке отображения блоков превью.

use std::{cell::Cell, collections::HashMap, ops::Range};

use egui::{Key, Modifiers};
use egui_commonmark::{TextHighlight, TextHighlights};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use crate::editor::search::find_all;
use crate::i18n::{tr, trf};
use crate::segment::{self, Segment, SegmentKind};

#[derive(Default)]
pub(super) struct Search {
    open: bool,
    query: String,
    case_sensitive: bool,
    version: Option<(u64, u64)>,
    matches: Vec<usize>,
    highlights: HashMap<usize, Vec<TextHighlight>>,
    current: usize,
}

impl Search {
    /// Возвращает блок, к текущему совпадению в котором нужно прокрутить панель.
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        text: &str,
        version: (u64, u64),
        segments: &[Segment],
        footnotes: &[usize],
    ) -> Option<usize> {
        let focus = ui.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::F));
        self.open |= focus;
        if !self.open {
            return None;
        }

        let mut step = 0;
        let mut changed = false;
        let mut close = ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
        ui.horizontal_wrapped(|ui| {
            let query = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text(tr!("Find", "Найти"))
                    .desired_width(200.0),
            );
            if focus {
                query.request_focus();
            }
            let enter = ui.input(|input| {
                input.events.iter().find_map(|event| match event {
                    egui::Event::Key {
                        key: Key::Enter,
                        pressed: true,
                        modifiers,
                        ..
                    } => Some(modifiers.shift),
                    _ => None,
                })
            });
            if (query.has_focus() || query.lost_focus())
                && let Some(backward) = enter
            {
                step = if backward { -1 } else { 1 };
                query.request_focus();
            }
            let case = ui
                .add(egui::Button::new("Aa").selected(self.case_sensitive))
                .on_hover_text(tr!("Match case", "Учитывать регистр"));
            if case.clicked() {
                self.case_sensitive = !self.case_sensitive;
            }

            changed = query.changed() || case.clicked() || self.version != Some(version);
            if changed {
                self.update(text, version, segments, footnotes);
            }
            if ui
                .button("⏶")
                .on_hover_text(tr!("Previous (Shift+Enter)", "Предыдущее (Shift+Enter)"))
                .clicked()
            {
                step = -1;
            }
            if ui
                .button("⏷")
                .on_hover_text(tr!("Next (Enter)", "Следующее (Enter)"))
                .clicked()
            {
                step = 1;
            }
            let total = self.matches.len();
            if total > 0 {
                self.current = (self.current as isize + step).rem_euclid(total as isize) as usize;
                ui.label(trf!("{} of {total}", "{} из {total}", self.current + 1));
            } else if !self.query.is_empty() {
                ui.label(tr!("No results", "Не найдено"));
            }
            close |= ui
                .button("×")
                .on_hover_text(tr!("Close (Esc)", "Закрыть (Esc)"))
                .clicked();
        });
        ui.separator();

        if close {
            self.open = false;
            return None;
        }
        if changed || step != 0 {
            return self.matches.get(self.current).copied();
        }
        None
    }

    pub(super) fn highlights<'a>(
        &'a self,
        segment: usize,
        reveal: Option<&'a Cell<Option<egui::Rect>>>,
    ) -> Option<TextHighlights<'a>> {
        if !self.open {
            return None;
        }
        Some(TextHighlights {
            ranges: self.highlights.get(&segment)?,
            current: self.current,
            reveal,
        })
    }

    fn update(
        &mut self,
        text: &str,
        version: (u64, u64),
        segments: &[Segment],
        footnotes: &[usize],
    ) {
        self.version = Some(version);
        self.matches.clear();
        self.highlights.clear();
        self.current = 0;
        if self.query.is_empty() {
            return;
        }

        let in_body = segments.iter().enumerate().filter_map(|(i, seg)| {
            matches!(&seg.kind, SegmentKind::Markdown { footnote: None, .. }).then_some(i)
        });
        for i in in_body.chain(footnotes.iter().copied()) {
            let seg = &segments[i];
            let SegmentKind::Markdown { appendix, .. } = &seg.kind else {
                continue;
            };
            let markdown = &text[seg.range.clone()];
            let input = if appendix.is_empty() {
                std::borrow::Cow::Borrowed(markdown)
            } else {
                std::borrow::Cow::Owned(format!("{markdown}{appendix}"))
            };
            let (visible, parts) = visible_text(&input, markdown.len());
            for range in find_all(&visible, &self.query, self.case_sensitive) {
                let index = self.matches.len();
                let first = parts.partition_point(|part| part.text.end <= range.start);
                let mut highlights = parts[first..]
                    .iter()
                    .take_while(|part| part.text.start < range.end)
                    .filter_map(|part| part.highlight(&range, index))
                    .peekable();
                if let Some(first) = highlights.peek_mut() {
                    first.first = true;
                    self.matches.push(i);
                    self.highlights.entry(i).or_default().extend(highlights);
                }
            }
        }
    }
}

struct Part {
    source: usize,
    text: Range<usize>,
}

impl Part {
    fn highlight(&self, range: &Range<usize>, index: usize) -> Option<TextHighlight> {
        let start = self.text.start.max(range.start);
        let end = self.text.end.min(range.end);
        (start < end).then(|| TextHighlight {
            source: self.source,
            bytes: start - self.text.start..end - self.text.start,
            index,
            first: false,
        })
    }
}

/// Текст события может отличаться от источника (например, &amp; или inline-код).
/// Диапазоны совпадений относятся к отображаемой строке, а байт источника связывает
/// её с событием рендера. Для блока кода всё содержимое рисуется одним виджетом.
fn visible_text(input: &str, end: usize) -> (String, Vec<Part>) {
    let mut visible = String::new();
    let mut parts = Vec::new();
    let mut in_image = false;
    let mut code = None;
    let events = Parser::new_ext(input, segment::parser_options()).into_offset_iter();
    for (event, span) in events.take_while(|(_, span)| span.start < end) {
        match event {
            Event::Start(Tag::Image { .. }) => {
                in_image = true;
                visible.push('\0');
            }
            Event::End(TagEnd::Image) => in_image = false,
            _ if in_image => {}
            Event::Start(Tag::CodeBlock(_)) => {
                visible.push('\0');
                code = Some((span.start, visible.len()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((source, start)) = code.take() {
                    let len = visible[start..].trim_end_matches('\n').len();
                    parts.push(Part {
                        source,
                        text: start..start + len,
                    });
                }
                visible.push('\0');
            }
            Event::Text(text) | Event::Code(text) | Event::Html(text) | Event::InlineHtml(text) => {
                let start = visible.len();
                visible.push_str(&text);
                if code.is_none() {
                    parts.push(Part {
                        source: span.start,
                        text: start..visible.len(),
                    });
                }
            }
            Event::SoftBreak | Event::HardBreak => visible.push(' '),
            Event::Start(Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. })
            | Event::End(
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link,
            ) => {}
            _ => visible.push('\0'),
        }
    }
    (visible, parts)
}
