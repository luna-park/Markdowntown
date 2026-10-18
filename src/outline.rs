//! Оглавление: список заголовков документа в боковой панели. Клик — переход к разделу,
//! текущий раздел (по верхней строке редактора или превью) подсвечен.

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};

use crate::i18n::tr;
use crate::segment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// 1–6.
    pub level: u8,
    pub text: String,
    /// Начало заголовка в документе.
    pub byte: usize,
}

/// Все заголовки документа, в том числе внутри списков и цитат.
pub fn headings(text: &str) -> Vec<Heading> {
    let mut out = Vec::new();
    let mut current: Option<Heading> = None;
    for (event, range) in Parser::new_ext(text, segment::parser_options()).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                current = Some(Heading {
                    level: level_number(level),
                    text: String::new(),
                    byte: range.start,
                });
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(mut h) = current.take() {
                    h.text = h.text.split_whitespace().collect::<Vec<_>>().join(" ");
                    out.push(h);
                }
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some(h) = &mut current {
                    h.text.push_str(&t);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(h) = &mut current {
                    h.text.push(' ');
                }
            }
            _ => {}
        }
    }
    out
}

fn level_number(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Индекс заголовка раздела, в котором находится байт `top` (последний заголовок не позже).
pub fn current_index(headings: &[Heading], top: usize) -> Option<usize> {
    headings.partition_point(|h| h.byte <= top).checked_sub(1)
}

/// Отступ на уровень вложенности, px.
const INDENT: f32 = 12.0;

#[derive(Default)]
pub struct Outline {
    /// Заголовки и версия документа, для которой они посчитаны.
    cache: Option<((u64, u64), Vec<Heading>)>,
}

impl Outline {
    pub fn headings(&mut self, text: &str, version: (u64, u64)) -> &[Heading] {
        if self.cache.as_ref().is_none_or(|(v, _)| *v != version) {
            self.cache = Some((version, headings(text)));
        }
        &self.cache.as_ref().expect("just set").1
    }

    /// Рисует список; возвращает байт заголовка, по которому кликнули.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        text: &str,
        version: (u64, u64),
        top: usize,
    ) -> Option<usize> {
        let headings = self.headings(text, version);
        if headings.is_empty() {
            ui.weak(tr!("No headings", "Нет заголовков"));
            return None;
        }
        // Уровни сдвигаются к самому крупному в документе: если H1 нет, H2 — без отступа.
        let min_level = headings.iter().map(|h| h.level).min().unwrap_or(1);
        let current = current_index(headings, top);
        let mut clicked = None;
        egui::ScrollArea::vertical()
            .id_salt("outline_scroll")
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (i, h) in headings.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add_space(f32::from(h.level - min_level) * INDENT);
                        let label = if h.text.is_empty() {
                            "(empty)"
                        } else {
                            &h.text
                        };
                        let mut text = egui::RichText::new(label);
                        if h.level == min_level {
                            text = text.strong();
                        }
                        let response = ui
                            .add(egui::Button::selectable(current == Some(i), text).truncate())
                            .on_hover_text(label);
                        if response.clicked() {
                            clicked = Some(h.byte);
                        }
                    });
                }
            });
        clicked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(text: &str) -> Vec<(u8, String, usize)> {
        headings(text)
            .into_iter()
            .map(|h| (h.level, h.text, h.byte))
            .collect()
    }

    #[test]
    fn collects_atx_and_setext_headings() {
        let text = "# One\n\ntext\n\nTwo **bold** `code`\n---\n\n### Three ###\n";
        assert_eq!(
            titles(text),
            [
                (1, "One".to_owned(), 0),
                (2, "Two bold code".to_owned(), 13),
                (3, "Three".to_owned(), 38),
            ]
        );
    }

    #[test]
    fn ignores_code_blocks_and_finds_nested() {
        let text = "```\n# not a heading\n```\n\n> ## Quoted\n\n- # In list\n";
        let found: Vec<_> = titles(text).into_iter().map(|(l, t, _)| (l, t)).collect();
        assert_eq!(found, [(2, "Quoted".to_owned()), (1, "In list".to_owned())]);
    }

    #[test]
    fn current_section() {
        let h = headings("intro\n\n# A\n\ntext\n\n# B\n");
        assert_eq!(current_index(&h, 0), None);
        assert_eq!(current_index(&h, 7), Some(0));
        assert_eq!(current_index(&h, 12), Some(0));
        assert_eq!(current_index(&h, 18), Some(1));
        assert_eq!(current_index(&h, 1000), Some(1));
    }

    #[test]
    fn cache_follows_version() {
        let mut outline = Outline::default();
        assert_eq!(outline.headings("# A\n", (1, 1)).len(), 1);
        // Та же версия — кэш, даже если текст другой.
        assert_eq!(outline.headings("# A\n# B\n", (1, 1)).len(), 1);
        assert_eq!(outline.headings("# A\n# B\n", (1, 2)).len(), 2);
    }
}
