//! Редактор Markdown: подсветка, номера строк, поиск и замена, команды форматирования,
//! продолжение списков, команды над строками.

mod commands;
mod highlight;
mod lines;
mod search;

use std::ops::Range;
use std::sync::Arc;

use egui::text::{CCursor, CCursorRange, LayoutJob};
use egui::text_edit::TextEditState;
use egui::{Galley, Id, Key, KeyboardShortcut, Modifiers};

use crate::document::Document;
use crate::i18n::{tr, trf};
use commands::Edit;
use highlight::Palette;

const TEXT_ID: &str = "markdown_editor";

/// Геометрия текста редактора за кадр — для синхронной прокрутки.
pub struct EditorGeometry {
    galley: Arc<Galley>,
    /// Верх текста в координатах содержимого прокрутки.
    galley_top: f32,
    pub offset: f32,
    pub max_offset: f32,
}

impl EditorGeometry {
    /// Байт документа в верхней строке, видимой при прокрутке `offset`.
    pub fn byte_at_offset(&self, text: &str, offset: f32) -> usize {
        let cursor = self
            .galley
            .cursor_from_pos(egui::vec2(0.0, offset - self.galley_top));
        char_to_byte(text, cursor.index.0)
    }

    /// Прокрутка, при которой байт `byte` оказывается в верхней строке.
    pub fn offset_for_byte(&self, text: &str, byte: usize) -> f32 {
        let rect = self
            .galley
            .pos_from_cursor(CCursor::new(byte_to_char(text, byte)));
        (self.galley_top + rect.min.y).clamp(0.0, self.max_offset)
    }
}

pub struct Editor {
    /// Размер шрифта редактора, pt (меню View).
    pub font_size: f32,
    highlight: Option<HighlightCache>,
    search: SearchState,
    has_focus: bool,
    /// Прокрутить так, чтобы этот байт был виден (например, найденное совпадение).
    reveal: Option<usize>,
    /// Прокрутка, которую нужно применить в следующем кадре.
    pending_offset: Option<f32>,
    /// Переход (оглавление): поставить этот байт в верхнюю строку панели.
    jump: Option<usize>,
    /// Id текстового поля: под ним egui хранит курсор и историю отмены —
    /// у каждой вкладки свой.
    text_id: Id,
    /// Ещё не показан: у egui под `text_id` может лежать курсор прежней вкладки
    /// с тем же id (память egui сохраняется между запусками) — забыть его.
    fresh: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            font_size: crate::style::EDITOR_FONT_SIZE,
            highlight: None,
            search: SearchState::default(),
            has_focus: false,
            reveal: None,
            pending_offset: None,
            jump: None,
            text_id: Id::new(TEXT_ID),
            fresh: true,
        }
    }
}

struct HighlightCache {
    key: (u64, u64, Option<usize>, bool, u32),
    job: LayoutJob,
}

#[derive(Default)]
struct SearchState {
    open: bool,
    replace: bool,
    focus_query: bool,
    query: String,
    replacement: String,
    case_sensitive: bool,
    matches: Vec<Range<usize>>,
    current: Option<usize>,
    /// Для чего посчитаны `matches`: (версия документа, запрос, регистр).
    computed_for: Option<((u64, u64), String, bool)>,
    /// Растёт при каждом пересчёте — часть ключа кэша подсветки.
    generation: u64,
    /// После пересчёта выбрать первое совпадение не раньше этого байта.
    resume_from: Option<usize>,
}

impl Editor {
    /// Редактор вкладки `tab`: своё состояние текстового поля (курсор, отмена).
    pub fn for_tab(tab: u64) -> Self {
        Self {
            text_id: Id::new((TEXT_ID, tab)),
            ..Self::default()
        }
    }

    /// `scroll_to` — прокрутка от синхронизации с превью.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        doc: &mut Document,
        scroll_to: Option<f32>,
    ) -> EditorGeometry {
        let ctx = ui.ctx().clone();
        let text_id = self.text_id;
        if std::mem::take(&mut self.fresh) {
            ctx.data_mut(|d| d.remove::<TextEditState>(text_id));
        }

        self.handle_search_shortcuts(&ctx, doc, text_id);
        if self.search.open {
            self.search_bar(ui, doc, text_id);
        }
        self.update_matches(doc);
        if self.has_focus {
            self.handle_editing_keys(&ctx, doc, text_id);
        }

        let range = crate::style::EDITOR_FONT_SIZE_RANGE;
        let font = egui::FontId::new(
            self.font_size.clamp(*range.start(), *range.end()),
            crate::fonts::editor_family(),
        );
        let palette = Palette::from_visuals(ui.visuals());
        let line_count = doc.text.bytes().filter(|&b| b == b'\n').count() + 1;
        let digit_width = ctx.fonts_mut(|f| f.glyph_width(&font, '0'));
        let gutter_width = digit_width * line_count.to_string().len().max(2) as f32 + 12.0;

        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("editor_scroll")
            .auto_shrink(false);
        if let Some(offset) = scroll_to.or(self.pending_offset.take()) {
            scroll = scroll.vertical_scroll_offset(offset);
        }

        let highlight = &mut self.highlight;
        let search = &self.search;
        let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
            let text = buf.as_str();
            let key = (
                hash_str(text),
                search.generation,
                search.current,
                ui.visuals().dark_mode,
                font.size.to_bits(),
            );
            if highlight.as_ref().is_none_or(|h| h.key != key) {
                let matches: &[Range<usize>] = if search.open { &search.matches } else { &[] };
                *highlight = Some(HighlightCache {
                    key,
                    job: highlight::layout_job(text, &font, &palette, matches, search.current),
                });
            }
            let mut job = highlight.as_ref().expect("just set").job.clone();
            job.wrap.max_width = wrap_width;
            ui.fonts_mut(|f| f.layout_job(job))
        };

        let mut galley_top = 0.0;
        let mut text_output = None;
        let scroll_output = scroll.show(ui, |ui| {
            let content_top = ui.min_rect().top();
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let (gutter, _) = ui.allocate_exact_size(
                    egui::vec2(gutter_width, ui.available_height()),
                    egui::Sense::hover(),
                );
                let output = egui::TextEdit::multiline(&mut doc.text)
                    .id(text_id)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(40)
                    .frame(egui::Frame::NONE)
                    .layouter(&mut layouter)
                    .show(ui);
                paint_line_numbers(ui, gutter, &output, &font, palette.markup);
                galley_top = output.galley_pos.y - content_top;
                text_output = Some(output);
            });
        });
        let output = text_output.expect("text edit is always shown");

        if output.response.response.changed() {
            doc.mark_edited();
        }
        self.has_focus = output.response.response.has_focus();

        let viewport = scroll_output.inner_rect.height();
        let geometry = EditorGeometry {
            galley: output.galley.clone(),
            galley_top,
            offset: scroll_output.state.offset.y,
            max_offset: (scroll_output.content_size.y - viewport).max(0.0),
        };

        if let Some(byte) = self.jump.take() {
            // Небольшой зазор сверху, чтобы заголовок не прилипал к краю.
            let y = geometry.offset_for_byte(&doc.text, byte) - 8.0;
            self.pending_offset = Some(y.max(0.0));
            ctx.request_repaint();
        }

        // Показать совпадение поиска: если не видно — прокрутить к нему.
        if let Some(byte) = self.reveal.take() {
            let y = geometry.offset_for_byte(&doc.text, byte);
            if y < geometry.offset || y > geometry.offset + viewport - 40.0 {
                self.pending_offset = Some((y - viewport / 3.0).max(0.0));
                ctx.request_repaint();
            }
        }
        geometry
    }

    /// Перейти к байту `byte`: курсор — туда, строка — вверху панели (оглавление).
    pub fn jump_to(&mut self, ctx: &egui::Context, text: &str, byte: usize) {
        let text_id = self.text_id;
        let mut state = TextEditState::load(ctx, text_id).unwrap_or_default();
        let cursor = CCursor::new(byte_to_char(text, byte));
        state.cursor.set_char_range(Some(CCursorRange::one(cursor)));
        state.store(ctx, text_id);
        ctx.memory_mut(|m| m.request_focus(text_id));
        // Курсор задан здесь — не сбрасывать его при первом показе.
        self.fresh = false;
        self.jump = Some(byte);
    }

    fn handle_search_shortcuts(&mut self, ctx: &egui::Context, doc: &Document, text_id: Id) {
        let find = KeyboardShortcut::new(Modifiers::COMMAND, Key::F);
        let replace = KeyboardShortcut::new(Modifiers::COMMAND, Key::H);
        let open_replace = ctx.input_mut(|i| i.consume_shortcut(&replace));
        if open_replace || ctx.input_mut(|i| i.consume_shortcut(&find)) {
            self.search.open = true;
            self.search.replace = open_replace;
            self.search.focus_query = true;
            // Однострочное выделение в редакторе становится запросом.
            if let Some(sel) = selection(ctx, text_id, &doc.text)
                && !sel.is_empty()
                && !doc.text[sel.clone()].contains('\n')
            {
                self.search.query = doc.text[sel].to_owned();
            }
        }
    }

    fn search_bar(&mut self, ui: &mut egui::Ui, doc: &mut Document, text_id: Id) {
        let ctx = ui.ctx().clone();
        let mut close = false;
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.search.query)
                    .hint_text(tr!("Find", "Найти"))
                    .desired_width(200.0),
            );
            if std::mem::take(&mut self.search.focus_query) {
                response.request_focus();
            }
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if enter {
                let back = ui.input(|i| i.modifiers.shift);
                self.step_match(if back { -1 } else { 1 });
                response.request_focus();
            }
            if response.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                close = true;
            }

            // Обычная кнопка с рамкой; включённая — залита цветом выделения.
            let case = ui
                .add(egui::Button::new("Aa").selected(self.search.case_sensitive))
                .on_hover_text(if self.search.case_sensitive {
                    tr!("Match case: on", "Учитывать регистр: да")
                } else {
                    tr!("Match case: off", "Учитывать регистр: нет")
                });
            if case.clicked() {
                self.search.case_sensitive = !self.search.case_sensitive;
            }
            let total = self.search.matches.len();
            let status = match (self.search.query.is_empty(), self.search.current) {
                (true, _) => String::new(),
                (false, _) if total == 0 => tr!("No results", "Не найдено").to_owned(),
                (false, Some(i)) => trf!("{} of {total}", "{} из {total}", i + 1),
                (false, None) => trf!("{total} found", "Найдено: {total}"),
            };
            ui.label(status);
            if ui
                .button("⏶")
                .on_hover_text(tr!("Previous (Shift+Enter)", "Предыдущее (Shift+Enter)"))
                .clicked()
            {
                self.step_match(-1);
            }
            if ui
                .button("⏷")
                .on_hover_text(tr!("Next (Enter)", "Следующее (Enter)"))
                .clicked()
            {
                self.step_match(1);
            }
            if !self.search.replace && ui.button(tr!("Replace…", "Заменить…")).clicked()
            {
                self.search.replace = true;
            }
            if ui
                .button("×")
                .on_hover_text(tr!("Close (Esc)", "Закрыть (Esc)"))
                .clicked()
            {
                close = true;
            }
        });
        if self.search.replace {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.search.replacement)
                        .hint_text(tr!("Replace with", "Заменить на"))
                        .desired_width(200.0),
                );
                let has_current = self.search.current.is_some();
                if ui
                    .add_enabled(has_current, egui::Button::new(tr!("Replace", "Заменить")))
                    .clicked()
                {
                    self.replace_current(&ctx, doc, text_id);
                }
                if ui
                    .add_enabled(
                        !self.search.matches.is_empty(),
                        egui::Button::new(tr!("All", "Все")),
                    )
                    .on_hover_text(tr!("Replace all", "Заменить все"))
                    .clicked()
                {
                    self.replace_all(&ctx, doc, text_id);
                }
            });
        }
        ui.separator();
        if close {
            self.search.open = false;
            self.search.matches.clear();
            self.search.current = None;
            self.search.computed_for = None;
            self.search.generation += 1;
        }
    }

    fn update_matches(&mut self, doc: &Document) {
        let s = &mut self.search;
        if !s.open {
            return;
        }
        let key = (doc.version(), s.query.clone(), s.case_sensitive);
        if s.computed_for.as_ref() == Some(&key) {
            return;
        }
        let query_changed = s
            .computed_for
            .as_ref()
            .is_none_or(|(_, q, c)| *q != s.query || *c != s.case_sensitive);
        let previous_start = s.current.and_then(|i| s.matches.get(i)).map(|m| m.start);
        s.matches = search::find_all(&doc.text, &s.query, s.case_sensitive);
        s.generation += 1;
        s.computed_for = Some(key);

        // Какое совпадение сделать текущим: после замены — следующее за ней,
        // при новом запросе — первое, при правке текста — ближайшее к прежнему.
        let from = s
            .resume_from
            .take()
            .or(if query_changed { None } else { previous_start });
        s.current = if s.matches.is_empty() {
            None
        } else {
            let from = from.unwrap_or(0);
            Some(s.matches.iter().position(|m| m.start >= from).unwrap_or(0))
        };
        if query_changed && let Some(i) = s.current {
            self.reveal = Some(s.matches[i].start);
        }
    }

    fn step_match(&mut self, delta: isize) {
        let n = self.search.matches.len();
        if n == 0 {
            return;
        }
        let i = match self.search.current {
            Some(i) => (i as isize + delta).rem_euclid(n as isize) as usize,
            None => 0,
        };
        self.search.current = Some(i);
        self.search.generation += 1;
        self.reveal = Some(self.search.matches[i].start);
    }

    fn replace_current(&mut self, ctx: &egui::Context, doc: &mut Document, text_id: Id) {
        let Some(range) = self
            .search
            .current
            .and_then(|i| self.search.matches.get(i))
            .cloned()
        else {
            return;
        };
        let end = range.start + self.search.replacement.len();
        self.search.resume_from = Some(end);
        apply_edit(
            ctx,
            text_id,
            doc,
            Edit {
                range,
                replacement: self.search.replacement.clone(),
                selection: end..end,
            },
        );
    }

    fn replace_all(&mut self, ctx: &egui::Context, doc: &mut Document, text_id: Id) {
        let new_text =
            search::replace_all(&doc.text, &self.search.matches, &self.search.replacement);
        // Одна правка на весь текст — отменяется одним Ctrl+Z.
        apply_edit(
            ctx,
            text_id,
            doc,
            Edit {
                range: 0..doc.text.len(),
                replacement: new_text,
                selection: 0..0,
            },
        );
    }

    /// Горячие клавиши форматирования, команды над строками и Enter в списках.
    /// Обрабатываются до `TextEdit`, чтобы он не получил эти нажатия.
    fn handle_editing_keys(&mut self, ctx: &egui::Context, doc: &mut Document, text_id: Id) {
        let Some(sel) = selection(ctx, text_id, &doc.text) else {
            return;
        };
        let shortcut = |key| KeyboardShortcut::new(Modifiers::COMMAND, key);
        let shift_shortcut =
            |key| KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, key);
        let alt = |key| KeyboardShortcut::new(Modifiers::ALT, key);
        let consume = |s: KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(&s));
        let text = doc.text.as_str();

        // Ctrl+Shift+… — раньше Ctrl+…: egui сопоставляет сочетания без учёта лишнего Shift.
        // Ctrl+0 (обычная строка вместо заголовка) не занимаем — это сброс масштаба;
        // повторное Ctrl+N на заголовке уровня N делает его обычной строкой.
        let edit = if consume(shift_shortcut(Key::D)) {
            Some(lines::duplicate_lines(text, sel))
        } else if consume(shift_shortcut(Key::K)) {
            Some(lines::delete_lines(text, sel))
        } else if consume(alt(Key::ArrowUp)) {
            lines::move_lines(text, sel, true)
        } else if consume(alt(Key::ArrowDown)) {
            lines::move_lines(text, sel, false)
        } else if let Some(level) = HEADING_KEYS.iter().position(|&key| consume(shortcut(key))) {
            lines::set_heading(text, sel, level + 1)
        } else if consume(shortcut(Key::Enter)) {
            lines::toggle_task(text, sel)
        } else if let Some(back) = tab_press(ctx) {
            let edit = if back {
                lines::outdent(text, sel.clone())
            } else {
                lines::indent(text, sel.clone())
            };
            // Иначе `TextEdit` заменил бы выделение на несколько строк табуляцией.
            if edit.is_some() || lines::is_multiline(text, &sel) {
                let modifiers = if back {
                    Modifiers::SHIFT
                } else {
                    Modifiers::NONE
                };
                ctx.input_mut(|i| i.consume_key(modifiers, Key::Tab));
            }
            edit
        } else if consume(shortcut(Key::B)) {
            Some(commands::toggle_wrap(&doc.text, sel, "**"))
        } else if consume(shortcut(Key::I)) {
            Some(commands::toggle_wrap(&doc.text, sel, "*"))
        } else if consume(shortcut(Key::E)) {
            Some(commands::toggle_wrap(&doc.text, sel, "`"))
        } else if consume(shortcut(Key::K)) {
            Some(commands::insert_link(&doc.text, sel))
        } else if sel.is_empty()
            && ctx.input(|i| i.key_pressed(Key::Enter) && i.modifiers.is_none())
        {
            commands::continue_list(&doc.text, sel.start).inspect(|_| {
                ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
            })
        } else {
            None
        };
        if let Some(edit) = edit {
            apply_edit(ctx, text_id, doc, edit);
        }
    }
}

/// Ctrl+1…6 — заголовки H1–H6.
const HEADING_KEYS: [Key; 6] = [
    Key::Num1,
    Key::Num2,
    Key::Num3,
    Key::Num4,
    Key::Num5,
    Key::Num6,
];

/// Нажат Tab без модификаторов (`Some(false)`) или Shift+Tab (`Some(true)`).
fn tab_press(ctx: &egui::Context) -> Option<bool> {
    ctx.input(|i| {
        if !i.key_pressed(Key::Tab) {
            None
        } else if i.modifiers.is_none() {
            Some(false)
        } else {
            i.modifiers.shift_only().then_some(true)
        }
    })
}

/// Применяет правку как отдельный шаг отмены и ставит выделение.
fn apply_edit(ctx: &egui::Context, text_id: Id, doc: &mut Document, edit: Edit) {
    let mut state = TextEditState::load(ctx, text_id).unwrap_or_default();
    let old_range = state
        .cursor
        .char_range()
        .unwrap_or_else(|| CCursorRange::one(CCursor::new(0)));
    let mut undoer = state.undoer();
    undoer.add_undo(&(old_range, doc.text.clone()));
    state.set_undoer(undoer);

    edit.apply(&mut doc.text);
    doc.mark_edited();

    let start = CCursor::new(byte_to_char(&doc.text, edit.selection.start));
    let end = CCursor::new(byte_to_char(&doc.text, edit.selection.end));
    state
        .cursor
        .set_char_range(Some(CCursorRange::two(start, end)));
    state.store(ctx, text_id);
}

/// Выделение в редакторе (байтовый диапазон, по возрастанию).
fn selection(ctx: &egui::Context, text_id: Id, text: &str) -> Option<Range<usize>> {
    let range = TextEditState::load(ctx, text_id)?.cursor.char_range()?;
    let [a, b] = [range.primary.index.0, range.secondary.index.0].map(|c| char_to_byte(text, c));
    Some(a.min(b)..a.max(b))
}

/// Номера логических строк в колонке слева (перенесённые строки не нумеруются).
fn paint_line_numbers(
    ui: &egui::Ui,
    gutter: egui::Rect,
    output: &egui::text_edit::TextEditOutput,
    font: &egui::FontId,
    color: egui::Color32,
) {
    let clip = ui.clip_rect();
    let painter = ui.painter();
    let mut line = 1;
    let mut starts_line = true;
    for row in &output.galley.rows {
        let top = output.galley_pos.y + row.pos.y;
        if starts_line && top + row.row.size.y >= clip.top() && top <= clip.bottom() {
            painter.text(
                egui::pos2(gutter.right() - 8.0, top),
                egui::Align2::RIGHT_TOP,
                line.to_string(),
                font.clone(),
                color,
            );
        }
        if row.ends_with_newline {
            line += 1;
            starts_line = true;
        } else {
            starts_line = false;
        }
        if top > clip.bottom() {
            break;
        }
    }
}

fn char_to_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(i, _)| i)
}

/// Индекс символа, в котором находится байт `byte`. Байт может быть не на границе
/// символа (например, получен интерполяцией при синхронной прокрутке) — тогда
/// округляется вниз до начала символа.
fn byte_to_char(text: &str, byte: usize) -> usize {
    text[..text.floor_char_boundary(byte)].chars().count()
}

fn hash_str(text: &str) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_byte_conversions() {
        let text = "aбв\nг";
        assert_eq!(char_to_byte(text, 0), 0);
        assert_eq!(char_to_byte(text, 2), 3);
        assert_eq!(char_to_byte(text, 100), text.len());
        assert_eq!(byte_to_char(text, 5), 3);
        assert_eq!(byte_to_char(text, text.len()), 5);
        // Байт внутри «б» (байты 1..3) — символ «б».
        assert_eq!(byte_to_char(text, 2), 1);
        assert_eq!(byte_to_char(text, 1000), 5);
    }

    /// Курсор под id текстового поля остаётся в памяти egui (она сохраняется между
    /// запусками): редактор новой вкладки с тем же id не должен его подхватить.
    #[test]
    fn new_editor_forgets_old_cursor() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, None);
        let mut doc = Document::new_untitled("line\n".repeat(100));
        let frame = |editor: &mut Editor, doc: &mut Document| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| editor.show(ui, doc, None));
            });
            out.textures_delta.clear();
        };
        let cursor = |editor: &Editor| {
            TextEditState::load(&ctx, editor.text_id).and_then(|s| s.cursor.char_range())
        };

        let mut old = Editor::for_tab(1);
        old.jump_to(&ctx, &doc.text, 50);
        frame(&mut old, &mut doc);
        assert!(cursor(&old).is_some());
        // Фокус в память не сохраняется — как после перезапуска.
        ctx.memory_mut(|m| m.surrender_focus(old.text_id));

        let mut new = Editor::for_tab(1);
        frame(&mut new, &mut doc);
        assert_eq!(cursor(&new), None);
    }

    /// Горячие клавиши команд над строками доходят до редактора через egui и не
    /// достаются `TextEdit` (он вставил бы табуляцию или сдвинул курсор).
    #[test]
    fn line_shortcuts_through_egui() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, None);
        let mut doc = Document::new_untitled("a\n- b\nc");
        let mut editor = Editor::for_tab(1);
        let press = |editor: &mut Editor, doc: &mut Document, key: Key, modifiers: Modifiers| {
            let events = vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ];
            let release = vec![egui::Event::ModifiersChanged(Modifiers::NONE)];
            for events in [events, release] {
                let input = egui::RawInput {
                    events,
                    ..Default::default()
                };
                let mut out = ctx.run_ui(input, |ui| {
                    egui::CentralPanel::default().show(ui, |ui| editor.show(ui, doc, None));
                });
                out.textures_delta.clear();
            }
        };
        let ctrl_shift = Modifiers::COMMAND | Modifiers::SHIFT;

        // Курсор в конце «- b», фокус — в редакторе.
        editor.jump_to(&ctx, &doc.text, 5);
        press(&mut editor, &mut doc, Key::F24, Modifiers::NONE);

        press(&mut editor, &mut doc, Key::Tab, Modifiers::NONE);
        assert_eq!(doc.text, "a\n  - b\nc");
        press(&mut editor, &mut doc, Key::Tab, Modifiers::SHIFT);
        assert_eq!(doc.text, "a\n- b\nc");
        press(&mut editor, &mut doc, Key::Enter, Modifiers::COMMAND);
        assert_eq!(doc.text, "a\n- [ ] b\nc");
        press(&mut editor, &mut doc, Key::ArrowDown, Modifiers::ALT);
        assert_eq!(doc.text, "a\nc\n- [ ] b");
        press(&mut editor, &mut doc, Key::ArrowUp, Modifiers::ALT);
        assert_eq!(doc.text, "a\n- [ ] b\nc");
        press(&mut editor, &mut doc, Key::D, ctrl_shift);
        assert_eq!(doc.text, "a\n- [ ] b\n- [ ] b\nc");
        press(&mut editor, &mut doc, Key::K, ctrl_shift);
        assert_eq!(doc.text, "a\n- [ ] b\nc");
        // Курсор — на строке, вставшей на место удалённой копии.
        press(&mut editor, &mut doc, Key::Num3, Modifiers::COMMAND);
        assert_eq!(doc.text, "a\n- [ ] b\n### c");
        // Отмена — по шагу на команду.
        press(&mut editor, &mut doc, Key::Z, Modifiers::COMMAND);
        assert_eq!(doc.text, "a\n- [ ] b\nc");
    }
}
