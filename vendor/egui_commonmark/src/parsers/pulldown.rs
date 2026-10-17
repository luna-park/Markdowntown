use std::iter::Peekable;
use std::ops::Range;

use crate::{CommonMarkCache, CommonMarkOptions};

use egui::{self, Id, Pos2, TextStyle, Ui};

use crate::{List, break_row};
use egui_commonmark_backend::elements::*;
use egui_commonmark_backend::misc::*;
use egui_commonmark_backend::pulldown::*;
use pulldown_cmark::{CowStr, HeadingLevel};

/// Newline logic is constructed by the following:
/// All elements try to insert a newline before them (if they are allowed)
/// and end their own line.
struct Newline {
    /// Whether a newline should not be inserted before a widget. This is only for
    /// the first widget.
    should_not_start_newline_forced: bool,
    /// Whether an element should insert a newline before it
    should_start_newline: bool,
    /// Whether an element should end it's own line using a newline
    /// This will have to be set to false in cases such as when blocks are within
    /// a list.
    should_end_newline: bool,
    /// only false when the widget is the last one.
    should_end_newline_forced: bool,
}

impl Default for Newline {
    fn default() -> Self {
        Self {
            should_not_start_newline_forced: true,
            should_start_newline: true,
            should_end_newline: true,
            should_end_newline_forced: true,
        }
    }
}

impl Newline {
    pub fn can_insert_end(&self) -> bool {
        self.should_end_newline && self.should_end_newline_forced
    }

    pub fn can_insert_start(&self) -> bool {
        self.should_start_newline && !self.should_not_start_newline_forced
    }

    pub fn try_insert_start(&self, ui: &mut Ui) {
        if self.can_insert_start() {
            newline(ui);
        }
    }

    pub fn try_insert_end(&self, ui: &mut Ui) {
        if self.can_insert_end() {
            newline(ui);
        }
    }
}

/// Markdowntown patch: роль события на верхнем уровне пункта списка.
#[derive(Clone, Copy)]
enum ItemPart {
    /// Блок целиком: таблица и цитата (`process_event` забирает их события), линия.
    Block,
    /// Начало блока, события которого идут следом: абзац, заголовок, код.
    BlockStart,
    BlockEnd,
    /// Текст тесного списка (без абзаца).
    Inline,
    /// Внутри блока, служебные события, вложенный список.
    Other,
}

fn item_part(event: &pulldown_cmark::Event, in_block: bool) -> ItemPart {
    use pulldown_cmark::{Event, Tag, TagEnd};
    match event {
        Event::Start(Tag::Table(_) | Tag::BlockQuote(_)) | Event::Rule => ItemPart::Block,
        Event::Start(
            Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::CodeBlock(_)
            | Tag::HtmlBlock
            | Tag::DefinitionList,
        ) => ItemPart::BlockStart,
        Event::End(
            TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::CodeBlock
            | TagEnd::HtmlBlock
            | TagEnd::DefinitionList,
        ) => ItemPart::BlockEnd,
        _ if in_block => ItemPart::Other,
        Event::Start(Tag::List(_)) | Event::End(_) | Event::TaskListMarker(_) => ItemPart::Other,
        _ => ItemPart::Inline,
    }
}

/// Markdowntown patch: блок в пункте списка — в своей области от верха строки, с той же
/// раскладкой, что у документа. Раскладка пункта (`horizontal_wrapped`) центрирует виджет
/// по высоте строки, и после `break_row` (строка нулевой высоты) блок вылезал вверх.
fn top_aligned<R>(ui: &mut Ui, enabled: bool, add: impl FnOnce(&mut Ui) -> R) -> R {
    if !enabled {
        return add(ui);
    }
    // Не `available_rect_before_wrap`: в строке с выравниванием по центру он начинается
    // выше курсора на половину минимальной высоты строки.
    let top_left = ui.cursor().min;
    let max_rect = ui.max_rect();
    let rect = egui::Rect::from_min_max(
        top_left,
        egui::pos2(max_rect.right(), max_rect.bottom().max(top_left.y)),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(ui.next_auto_id())
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::BOTTOM).with_main_wrap(true)),
    );
    let inner = add(&mut child);
    ui.advance_cursor_after_rect(child.min_rect());
    inner
}

/// Markdowntown patch: есть ли в пункте абзацы верхнего уровня — признак свободного списка.
fn has_top_level_paragraph(events: &[(pulldown_cmark::Event, Range<usize>)]) -> bool {
    let mut depth = 0usize;
    for (event, _) in events {
        match event {
            pulldown_cmark::Event::Start(pulldown_cmark::Tag::Paragraph) if depth == 0 => {
                return true;
            }
            pulldown_cmark::Event::Start(_) => depth += 1,
            pulldown_cmark::Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

#[derive(Default)]
struct DefinitionList {
    is_first_item: bool,
    is_def_list_def: bool,
}

pub struct CommonMarkViewerInternal {
    curr_table: usize,
    text_style: Style,
    list: List,
    link: Option<Link>,
    image: Option<Image>,
    line: Newline,
    code_block: Option<CodeBlock>,

    /// Only populated if the html_fn option has been set
    html_block: String,
    is_list_item: bool,
    def_list: DefinitionList,
    is_table: bool,
    /// Markdowntown patch: выравнивание столбцов текущей таблицы (`:---:`, `---:`).
    table_alignments: Vec<pulldown_cmark::Alignment>,
    is_blockquote: bool,
    checkbox_events: Vec<CheckboxClickEvent>,
    deferred_scroll_to_heading: Option<String>,
}

pub(crate) struct CheckboxClickEvent {
    pub(crate) checked: bool,
    pub(crate) span: Range<usize>,
}

impl CommonMarkViewerInternal {
    pub fn new() -> Self {
        Self {
            curr_table: 0,
            text_style: Style::default(),
            list: List::default(),
            link: None,
            image: None,
            line: Newline::default(),
            is_list_item: false,
            def_list: Default::default(),
            code_block: None,
            html_block: String::new(),
            is_table: false,
            table_alignments: Vec::new(),
            is_blockquote: false,
            checkbox_events: Vec::new(),
            deferred_scroll_to_heading: None,
        }
    }
}

fn parser_options_extras(
    is_math_enabled: bool,
    is_scroll_to_heading_enabled: bool,
) -> pulldown_cmark::Options {
    let mut result = parser_options();
    if is_math_enabled {
        result |= pulldown_cmark::Options::ENABLE_MATH;
    }
    if is_scroll_to_heading_enabled {
        result |= pulldown_cmark::Options::ENABLE_HEADING_ATTRIBUTES;
    }
    result
}

impl CommonMarkViewerInternal {
    /// Be aware that this acquires egui::Context internally.
    /// If split Id is provided then split points will be populated
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        text: &str,
        split_points_id: Option<Id>,
    ) -> (egui::InnerResponse<()>, Vec<CheckboxClickEvent>) {
        let max_width = options.max_width(ui);
        let layout = egui::Layout::left_to_right(egui::Align::BOTTOM).with_main_wrap(true);

        let re = ui.allocate_ui_with_layout(egui::vec2(max_width, 0.0), layout, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let height = ui.text_style_height(&TextStyle::Body);
            ui.set_row_height(height);

            let mut events = pulldown_cmark::Parser::new_ext(
                text,
                parser_options_extras(options.math_fn.is_some(), options.enable_scroll_to_heading),
            )
            .into_offset_iter()
            .enumerate()
            .peekable();

            // Markdowntown patch: `render_until` — хвост текста не рисуется, а его
            // начало считается концом документа (без завершающего перевода строки).
            let hidden = |span: &std::ops::Range<usize>| {
                options.render_until.is_some_and(|end| span.start >= end)
            };
            while let Some((index, (e, src_span))) = events.next() {
                if hidden(&src_span) {
                    break;
                }
                let start_position = ui.next_widget_position();
                let is_element_end = matches!(e, pulldown_cmark::Event::End(_));
                let should_add_split_point = self.list.is_inside_a_list() && is_element_end;

                if events.peek().is_none_or(|(_, (_, span))| hidden(span)) {
                    self.line.should_end_newline_forced = false;
                }

                self.process_event(ui, &mut events, e, src_span, cache, options, max_width);

                if let Some(source_id) = split_points_id
                    && should_add_split_point
                {
                    let scroll_cache = scroll_cache(cache, &source_id);
                    let end_position = ui.next_widget_position();

                    let split_point_exists = scroll_cache
                        .split_points
                        .iter()
                        .any(|(i, _, _)| *i == index);

                    if !split_point_exists {
                        scroll_cache
                            .split_points
                            .push((index, start_position, end_position));
                    }
                }

                if index == 0 {
                    self.line.should_not_start_newline_forced = false;
                }
            }

            // deferral to make it consistent no matter whether the target is before or after the link
            *cache.scroll_to_id_target_mut() = self.deferred_scroll_to_heading.take();

            if let Some(source_id) = split_points_id {
                scroll_cache(cache, &source_id).page_size =
                    Some(ui.next_widget_position().to_vec2());
            }
        });

        (re, std::mem::take(&mut self.checkbox_events))
    }

    pub(crate) fn show_scrollable(
        &mut self,
        source_id: Id,
        ui: &mut egui::Ui,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        text: &str,
    ) {
        let available_size = ui.available_size();
        let scroll_id = source_id.with("_scroll_area");

        let Some(page_size) = scroll_cache(cache, &source_id).page_size else {
            egui::ScrollArea::vertical()
                .id_salt(scroll_id)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    self.show(ui, cache, options, text, Some(source_id));
                });
            // Prevent repopulating points twice at startup
            scroll_cache(cache, &source_id).available_size = available_size;
            return;
        };

        let events = pulldown_cmark::Parser::new_ext(
            text,
            parser_options_extras(options.math_fn.is_some(), options.enable_scroll_to_heading),
        )
        .into_offset_iter()
        .collect::<Vec<_>>();

        let num_rows = events.len();

        egui::ScrollArea::vertical()
            .id_salt(scroll_id)
            // Elements have different widths, so the scroll area cannot try to shrink to the
            // content, as that will mean that the scroll bar will move when loading elements
            // with different widths.
            .auto_shrink([false, true])
            .show_viewport(ui, |ui, viewport| {
                ui.set_height(page_size.y);
                let layout = egui::Layout::left_to_right(egui::Align::BOTTOM).with_main_wrap(true);

                let max_width = options.max_width(ui);
                ui.allocate_ui_with_layout(egui::vec2(max_width, 0.0), layout, |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let scroll_cache = scroll_cache(cache, &source_id);

                    // finding the first element that's not in the viewport anymore
                    let (first_event_index, _, first_end_position) = scroll_cache
                        .split_points
                        .iter()
                        .filter(|(_, _, end_position)| end_position.y < viewport.min.y)
                        .nth_back(1)
                        .copied()
                        .unwrap_or((0, Pos2::ZERO, Pos2::ZERO));

                    // finding the last element that's just outside the viewport
                    let last_event_index = scroll_cache
                        .split_points
                        .iter()
                        .filter(|(_, start_position, _)| start_position.y > viewport.max.y)
                        .nth(1)
                        .map(|(index, _, _)| *index)
                        .unwrap_or(num_rows);

                    ui.allocate_space(first_end_position.to_vec2());

                    // only rendering the elements that are inside the viewport
                    let mut events = events
                        .into_iter()
                        .enumerate()
                        .skip(first_event_index)
                        .take(last_event_index - first_event_index)
                        .peekable();

                    while let Some((i, (e, src_span))) = events.next() {
                        if events.peek().is_none() {
                            self.line.should_end_newline_forced = false;
                        }

                        self.process_event(ui, &mut events, e, src_span, cache, options, max_width);

                        if i == 0 {
                            self.line.should_not_start_newline_forced = false;
                        }
                    }
                });
            });

        // Forcing full re-render to repopulate split points for the new size
        let scroll_cache = scroll_cache(cache, &source_id);
        if available_size != scroll_cache.available_size {
            scroll_cache.available_size = available_size;
            scroll_cache.page_size = None;
            scroll_cache.split_points.clear();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn process_event<'e>(
        &mut self,
        ui: &mut Ui,
        events: &mut Peekable<impl Iterator<Item = EventIteratorItem<'e>>>,
        event: pulldown_cmark::Event,
        src_span: Range<usize>,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        max_width: f32,
    ) {
        self.event(ui, event, src_span, cache, options, max_width);

        self.def_list_def_wrapping(events, max_width, cache, options, ui);
        self.item_list_wrapping(events, max_width, cache, options, ui);
        self.table(events, cache, options, ui, max_width);
        self.blockquote(events, max_width, cache, options, ui);
    }

    fn def_list_def_wrapping<'e>(
        &mut self,
        events: &mut Peekable<impl Iterator<Item = EventIteratorItem<'e>>>,
        max_width: f32,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        ui: &mut Ui,
    ) {
        if self.def_list.is_def_list_def {
            self.def_list.is_def_list_def = false;

            let item_events = delayed_events(events, |tag| {
                matches!(tag, pulldown_cmark::TagEnd::DefinitionListDefinition)
            });

            let mut events_iter = item_events.into_iter().enumerate().peekable();

            self.line.try_insert_start(ui);

            // Proccess a single event separately so that we do not insert spaces where we do not
            // want them
            self.line.should_start_newline = false;
            if let Some((_, (e, src_span))) = events_iter.next() {
                self.process_event(ui, &mut events_iter, e, src_span, cache, options, max_width);
            }

            ui.label(" ".repeat(options.indentation_spaces));
            self.line.should_start_newline = true;
            self.line.should_end_newline = false;
            // Required to ensure that the content is aligned with the identation
            ui.horizontal_wrapped(|ui| {
                while let Some((_, (e, src_span))) = events_iter.next() {
                    self.process_event(
                        ui,
                        &mut events_iter,
                        e,
                        src_span,
                        cache,
                        options,
                        max_width,
                    );
                }
            });
            self.line.should_end_newline = true;

            // Only end the definition items line if it is not the last element in the list
            if !matches!(
                events.peek(),
                Some((
                    _,
                    (
                        pulldown_cmark::Event::End(pulldown_cmark::TagEnd::DefinitionList),
                        _
                    )
                ))
            ) {
                self.line.try_insert_end(ui);
            }
        }
    }

    fn item_list_wrapping<'e>(
        &mut self,
        events: &mut impl Iterator<Item = EventIteratorItem<'e>>,
        max_width: f32,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        ui: &mut Ui,
    ) {
        if self.is_list_item {
            self.is_list_item = false;

            let item_events = delayed_events_list_item(events);
            // Markdowntown patch: отступы между блоками пункта и между пунктами — как на
            // GitHub (1em = отступ между блоками превью). Внутри пункта переводы строк между
            // блоками отключены, и в оригинале абзацы склеивались в одну строку, а код
            // и таблицы вставали в конец строки текста.
            let gap = ui.text_style_height(&TextStyle::Body);
            let has_paragraph = has_top_level_paragraph(&item_events);
            let loose = self.list.start_item(ui, options, has_paragraph, gap);
            let mut events_iter = item_events.into_iter().enumerate().peekable();
            // Что было последним на верхнем уровне пункта: `Some(true)` — блок (у него
            // на GitHub отступ снизу), `Some(false)` — текст тесного списка.
            let mut last_block: Option<bool> = None;

            // Required to ensure that the content of the list item is aligned with
            // the * or - when wrapping
            ui.horizontal_wrapped(|ui| {
                // Код и таблицы — по ширине пункта, а не всей панели.
                let max_width = max_width.min(ui.max_rect().width());
                let mut in_block = false;
                while let Some((_, (e, src_span))) = events_iter.next() {
                    let part = item_part(&e, in_block);
                    let starts_content = match part {
                        ItemPart::Block | ItemPart::BlockStart => true,
                        ItemPart::Inline => last_block == Some(true),
                        ItemPart::BlockEnd | ItemPart::Other => false,
                    };
                    if starts_content && let Some(after_block) = last_block {
                        let row_gap = if loose || after_block {
                            gap
                        } else {
                            ui.spacing().item_spacing.y
                        };
                        break_row(ui, row_gap);
                    }
                    self.process_event(
                        ui,
                        &mut events_iter,
                        e,
                        src_span,
                        cache,
                        options,
                        max_width,
                    );
                    match part {
                        ItemPart::BlockStart => in_block = true,
                        ItemPart::Block | ItemPart::BlockEnd => {
                            in_block = false;
                            last_block = Some(true);
                        }
                        ItemPart::Inline => last_block = Some(false),
                        ItemPart::Other => {}
                    }
                }
            });
            self.list.end_item(loose || last_block == Some(true));
        }
    }

    fn blockquote<'e>(
        &mut self,
        events: &mut Peekable<impl Iterator<Item = EventIteratorItem<'e>>>,
        max_width: f32,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        ui: &mut Ui,
    ) {
        if self.is_blockquote {
            // Markdowntown patch: вложенные цитаты. В оригинале события собирались до первого
            // `End(BlockQuote)` (конца вложенной цитаты) и рисовались через `event`, без
            // обёрток — вложенная цитата не получала свою полосу. Теперь сбор учитывает
            // глубину, а содержимое идёт через `process_event`, как у пунктов списка.
            self.is_blockquote = false;
            let mut collected_events = delayed_events_blockquote(events);
            self.line.try_insert_start(ui);

            // Currently the blockquotes are made in such a way that they need a newline at the end
            // and the start so when this is the first element in the markdown the newline must be
            // manually enabled
            self.line.should_not_start_newline_forced = false;
            let alert = parse_alerts(&options.alerts, &mut collected_events);
            // Внутри пункта списка переводы строк между блоками отключены — в цитате они
            // нужны, как вне списка: без них полоса цитаты короче строки текста.
            let start = std::mem::replace(&mut self.line.should_start_newline, true);
            let end = std::mem::replace(&mut self.line.should_end_newline, true);
            top_aligned(ui, self.list.is_inside_a_list(), |ui| {
                if let Some(alert) = alert {
                    egui_commonmark_backend::alert_ui(alert, ui, |ui| {
                        self.process_events(ui, collected_events, cache, options, max_width);
                    })
                } else {
                    blockquote(ui, ui.visuals().weak_text_color(), |ui| {
                        // Вложенная цитата не должна снимать признак цитаты с текста внешней.
                        let was_quote = std::mem::replace(&mut self.text_style.quote, true);
                        self.process_events(ui, collected_events, cache, options, max_width);
                        self.text_style.quote = was_quote;
                    });
                }
            });
            self.line.should_start_newline = start;
            self.line.should_end_newline = end;

            if events.peek().is_none() {
                self.line.should_end_newline_forced = false;
            }

            self.line.try_insert_end(ui);
        }
    }

    /// Markdowntown patch: рисует собранные события с обработкой вложенных блоков.
    fn process_events(
        &mut self,
        ui: &mut Ui,
        collected: Vec<(pulldown_cmark::Event, Range<usize>)>,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        max_width: f32,
    ) {
        let mut events = collected.into_iter().enumerate().peekable();
        while let Some((_, (event, src_span))) = events.next() {
            self.process_event(ui, &mut events, event, src_span, cache, options, max_width);
        }
    }

    fn table<'e>(
        &mut self,
        events: &mut Peekable<impl Iterator<Item = EventIteratorItem<'e>>>,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        ui: &mut Ui,
        max_width: f32,
    ) {
        if self.is_table {
            self.line.try_insert_start(ui);
            // Markdowntown patch: таблица — с новой строки, даже если перевод строки
            // отключён (иначе она вставала в конец строки текста и сжималась в остаток).
            break_row(ui, ui.spacing().item_spacing.y);

            let id = ui.id().with("_table").with(self.curr_table);
            self.curr_table += 1;

            // Markdowntown patch: таблица по ширине панели с переносом текста в ячейках
            // (вместо `Grid`, который не переносит текст и растягивался вширь).
            // Ширины столбцов — как при автоматической раскладке таблиц в браузере:
            // см. `column_widths`. Если даже минимальные ширины не помещаются, таблица
            // прокручивается по горизонтали внутри себя.
            let Table { header, rows } = parse_table(events);
            let frame = egui::Frame::group(ui.style());
            let frame_width = frame.total_margin().sum().x;
            let available = (ui.available_width().min(max_width) - frame_width).max(0.0);
            // Отступы в ячейке: по вертикали — как у GitHub (6 px), чтобы строки не сливались.
            let pad = egui::vec2(6.0, 6.0);

            let columns = std::iter::once(&header)
                .chain(rows.iter())
                .map(Vec::len)
                .max()
                .unwrap_or(0);
            let mut min = vec![0.0f32; columns];
            let mut max = vec![0.0f32; columns];
            // Естественная ширина каждой ячейки (строки — как в таблице, заголовок первым) —
            // для выравнивания однострочных ячеек.
            let mut naturals: Vec<Vec<f32>> = Vec::new();
            for (r, row) in std::iter::once(&header).chain(rows.iter()).enumerate() {
                let row_is_header = r == 0;
                let mut row_naturals = Vec::with_capacity(row.len());
                for (c, cell) in row.iter().enumerate() {
                    let (cell_min, cell_max) = measure_cell(ui, cell, row_is_header);
                    min[c] = min[c].max(cell_min + 2.0 * pad.x);
                    max[c] = max[c].max(cell_max + 2.0 * pad.x);
                    row_naturals.push(cell_max);
                }
                naturals.push(row_naturals);
            }
            let alignments = std::mem::take(&mut self.table_alignments);
            let widths = column_widths(&min, &max, available);
            let total: f32 = widths.iter().sum();

            let in_list = self.list.is_inside_a_list();
            top_aligned(ui, in_list, |ui| egui::ScrollArea::horizontal()
                .id_salt(id.with("_scroll"))
                .auto_shrink([true, true])
                .show(ui, |ui| {
                    // Строки — сверху вниз: родитель (абзац `egui_commonmark`) раскладывает
                    // слева направо с переносом, и рамка унаследовала бы это.
                    frame.show(ui, |ui| ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let row_height = ui.spacing().interact_size.y;
                        let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
                        let stripe = ui.visuals().faint_bg_color;
                        let left = ui.cursor().left();
                        let top = ui.cursor().top();
                        let mut header_bottom = top;
                        let mut bottom = top;

                        for (r, row) in std::iter::once(header).chain(rows).enumerate() {
                            let row_top = ui.cursor().top();
                            let background = ui.painter().add(egui::Shape::Noop);
                            let mut height: f32 = 0.0;
                            let mut x = left;
                            for (c, cell) in row.into_iter().enumerate() {
                                let width = widths.get(c).copied().unwrap_or(0.0);
                                let natural = naturals
                                    .get(r)
                                    .and_then(|row| row.get(c))
                                    .copied()
                                    .unwrap_or(0.0);
                                let shift = cell_shift(
                                    alignments.get(c).copied(),
                                    width - 2.0 * pad.x,
                                    natural,
                                );
                                let rect = egui::Rect::from_min_size(
                                    egui::pos2(x + pad.x + shift, row_top + pad.y),
                                    // Как `horizontal_wrapped`: область высотой в одну строку,
                                    // строки переноса наращиваются вниз (бесконечная высота
                                    // дала бы NaN, слишком большая — одну огромную строку).
                                    egui::vec2((width - 2.0 * pad.x - shift).max(1.0), row_height),
                                );
                                let mut cell_ui = ui.new_child(
                                    egui::UiBuilder::new()
                                        .id_salt((r, c))
                                        .max_rect(rect)
                                        .layout(
                                            egui::Layout::left_to_right(egui::Align::Center)
                                                .with_main_wrap(true),
                                        ),
                                );
                                // Заголовок — жирным, как на GitHub.
                                let was_strong = self.text_style.strong;
                                self.text_style.strong |= r == 0;
                                for (e, src_span) in cell {
                                    // Служебные события таблицы: `parse_row` кладёт конец
                                    // предыдущей ячейки в начало следующей, а его обработчик
                                    // рисует пробел-отступ для `Grid` — здесь отступ задаёт `pad`.
                                    if matches!(
                                        e,
                                        pulldown_cmark::Event::Start(
                                            pulldown_cmark::Tag::TableCell
                                                | pulldown_cmark::Tag::TableRow
                                        ) | pulldown_cmark::Event::End(
                                            pulldown_cmark::TagEnd::TableCell
                                                | pulldown_cmark::TagEnd::TableRow
                                        )
                                    ) {
                                        continue;
                                    }
                                    let tmp_start = std::mem::replace(
                                        &mut self.line.should_start_newline,
                                        false,
                                    );
                                    let tmp_end =
                                        std::mem::replace(&mut self.line.should_end_newline, false);
                                    self.event(
                                        &mut cell_ui,
                                        e,
                                        src_span,
                                        cache,
                                        options,
                                        rect.width(),
                                    );
                                    self.line.should_start_newline = tmp_start;
                                    self.line.should_end_newline = tmp_end;
                                }
                                self.text_style.strong = was_strong;
                                height = height.max(cell_ui.min_rect().height() + 2.0 * pad.y);
                                x += width;
                            }
                            let row_rect = egui::Rect::from_min_size(
                                egui::pos2(left, row_top),
                                egui::vec2(total, height),
                            );
                            // Полосы — на каждой второй строке тела таблицы.
                            if r > 0 && r % 2 == 0 {
                                ui.painter().set(
                                    background,
                                    egui::Shape::rect_filled(row_rect, 0.0, stripe),
                                );
                            }
                            ui.advance_cursor_after_rect(row_rect);
                            if r == 0 {
                                header_bottom = row_rect.bottom();
                            }
                            bottom = row_rect.bottom();
                        }

                        // Линия под заголовком и вертикальные разделители столбцов.
                        let painter = ui.painter();
                        painter.hline(left..=left + total, header_bottom, stroke);
                        let mut x = left;
                        for width in widths.iter().take(widths.len().saturating_sub(1)) {
                            x += width;
                            painter.vline(x, top..=bottom, stroke);
                        }
                    }));
                }));

            self.is_table = false;
            if events.peek().is_none() {
                self.line.should_end_newline_forced = false;
            }

            self.line.try_insert_end(ui);
        }
    }

    fn event(
        &mut self,
        ui: &mut Ui,
        event: pulldown_cmark::Event,
        src_span: Range<usize>,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        max_width: f32,
    ) {
        match event {
            pulldown_cmark::Event::Start(tag) => self.start_tag(ui, tag, cache, options),
            pulldown_cmark::Event::End(tag) => self.end_tag(ui, tag, cache, options, max_width),
            pulldown_cmark::Event::Text(text) => {
                self.event_text(text, ui);
            }
            pulldown_cmark::Event::Code(text) => {
                self.text_style.code = true;
                self.event_text(text, ui);
                self.text_style.code = false;
            }
            pulldown_cmark::Event::InlineHtml(text) => {
                self.event_text(text, ui);
            }

            pulldown_cmark::Event::Html(text) => {
                if options.html_fn.is_some() {
                    self.html_block.push_str(&text);
                } else {
                    self.event_text(text, ui);
                }
            }
            pulldown_cmark::Event::FootnoteReference(footnote) => {
                footnote_start(ui, &footnote);
            }
            pulldown_cmark::Event::SoftBreak => {
                soft_break(ui);
            }
            pulldown_cmark::Event::HardBreak => newline(ui),
            pulldown_cmark::Event::Rule => {
                self.line.try_insert_start(ui);
                rule(ui, self.line.can_insert_end());
            }
            pulldown_cmark::Event::TaskListMarker(mut checkbox) => {
                if options.mutable {
                    if ui
                        .add(egui::Checkbox::without_text(&mut checkbox))
                        .clicked()
                    {
                        self.checkbox_events.push(CheckboxClickEvent {
                            checked: checkbox,
                            span: src_span,
                        });
                    }
                } else {
                    ui.add(ImmutableCheckbox::without_text(&mut checkbox));
                }
                // Markdowntown patch: пробел между флажком и текстом — без него слипались.
                let font = TextStyle::Body.resolve(ui.style());
                let space = ui.fonts_mut(|f| f.glyph_width(&font, ' '));
                ui.add_space(space);
            }
            pulldown_cmark::Event::InlineMath(tex) => {
                if let Some(math_fn) = options.math_fn {
                    math_fn(ui, &tex, true);
                }
            }
            pulldown_cmark::Event::DisplayMath(tex) => {
                if let Some(math_fn) = options.math_fn {
                    math_fn(ui, &tex, false);
                }
            }
        }
    }

    fn event_text(&mut self, text: CowStr, ui: &mut Ui) {
        let rich_text = self.text_style.to_richtext(ui, &text);
        if let Some(image) = &mut self.image {
            image.alt_text.push(rich_text);
        } else if let Some(block) = &mut self.code_block {
            block.content.push_str(&text);
        } else if let Some(link) = &mut self.link {
            link.text.push(rich_text);
        } else {
            ui.label(rich_text);
        }
    }

    fn start_tag(
        &mut self,
        ui: &mut Ui,
        tag: pulldown_cmark::Tag,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
    ) {
        match tag {
            pulldown_cmark::Tag::Paragraph => {
                self.line.try_insert_start(ui);
            }
            pulldown_cmark::Tag::Heading { level, id, .. } => {
                if let Some(scroll_target) = cache.scroll_to_id_target()
                    && let Some(id) = id
                    && id.into_string() == scroll_target
                {
                    ui.scroll_to_cursor(Some(egui::Align::TOP));
                    cache.scroll_to_id_target_mut().take();
                }

                // Headings should always insert a newline even if it is at the start.
                // Whether this is okay in all scenarios is a different question.
                // Markdowntown patch: кроме самого первого элемента — превью рисует
                // каждый блок отдельно, и отступ перед блоком задаёт само.
                if !self.line.should_not_start_newline_forced {
                    newline(ui);
                }
                self.text_style.heading = Some(match level {
                    HeadingLevel::H1 => 0,
                    HeadingLevel::H2 => 1,
                    HeadingLevel::H3 => 2,
                    HeadingLevel::H4 => 3,
                    HeadingLevel::H5 => 4,
                    HeadingLevel::H6 => 5,
                });
            }

            // deliberately not using the built in alerts from pulldown-cmark as
            // the markdown itself cannot be localized :( e.g: [!TIP]
            pulldown_cmark::Tag::BlockQuote(_) => {
                self.is_blockquote = true;
            }
            pulldown_cmark::Tag::CodeBlock(c) => {
                match c {
                    pulldown_cmark::CodeBlockKind::Fenced(lang) => {
                        self.code_block = Some(crate::CodeBlock {
                            lang: Some(lang.to_string()),
                            content: "".to_string(),
                        });
                    }
                    pulldown_cmark::CodeBlockKind::Indented => {
                        self.code_block = Some(crate::CodeBlock {
                            lang: None,
                            content: "".to_string(),
                        });
                    }
                }
                self.line.try_insert_start(ui);
            }

            pulldown_cmark::Tag::List(point) => {
                if !self.list.is_inside_a_list() && self.line.can_insert_start() {
                    newline(ui);
                }

                if let Some(number) = point {
                    self.list.start_level_with_number(number);
                } else {
                    self.list.start_level_without_number();
                }
                self.line.should_start_newline = false;
                self.line.should_end_newline = false;
            }

            pulldown_cmark::Tag::Item => {
                // Markdowntown patch: маркер пункта рисует `item_list_wrapping` — отступ
                // перед пунктом зависит от его содержимого.
                self.is_list_item = true;
            }

            pulldown_cmark::Tag::FootnoteDefinition(note) => {
                self.line.try_insert_start(ui);

                self.line.should_start_newline = false;
                self.line.should_end_newline = false;
                footnote(ui, &note);
            }
            pulldown_cmark::Tag::Table(alignments) => {
                self.is_table = true;
                self.table_alignments = alignments;
            }
            pulldown_cmark::Tag::TableHead => {}
            pulldown_cmark::Tag::TableRow => {}
            pulldown_cmark::Tag::TableCell => {}
            pulldown_cmark::Tag::Emphasis => {
                self.text_style.emphasis = true;
            }
            pulldown_cmark::Tag::Strong => {
                self.text_style.strong = true;
            }
            pulldown_cmark::Tag::Strikethrough => {
                self.text_style.strikethrough = true;
            }
            pulldown_cmark::Tag::Link { dest_url, .. } => {
                self.link = Some(crate::Link {
                    destination: dest_url.to_string(),
                    text: Vec::new(),
                });
            }
            pulldown_cmark::Tag::Image { dest_url, .. } => {
                self.image = Some(crate::Image::new(&dest_url, options));
            }
            pulldown_cmark::Tag::HtmlBlock => {
                self.line.try_insert_start(ui);
            }
            pulldown_cmark::Tag::MetadataBlock(_) => {}

            pulldown_cmark::Tag::DefinitionList => {
                self.line.try_insert_start(ui);
                self.def_list.is_first_item = true;
            }
            pulldown_cmark::Tag::DefinitionListTitle => {
                // we disable newline as the first title should not insert a newline
                // as we have already done that upon the DefinitionList Tag
                if !self.def_list.is_first_item {
                    self.line.try_insert_start(ui)
                } else {
                    self.def_list.is_first_item = false;
                }
            }
            pulldown_cmark::Tag::DefinitionListDefinition => {
                self.def_list.is_def_list_def = true;
            }
            // Not yet supported
            pulldown_cmark::Tag::Superscript | pulldown_cmark::Tag::Subscript => {}
        }
    }

    fn end_tag(
        &mut self,
        ui: &mut Ui,
        tag: pulldown_cmark::TagEnd,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        max_width: f32,
    ) {
        match tag {
            pulldown_cmark::TagEnd::Paragraph => {
                self.line.try_insert_end(ui);
            }
            pulldown_cmark::TagEnd::Heading { .. } => {
                self.line.try_insert_end(ui);
                self.text_style.heading = None;
            }
            pulldown_cmark::TagEnd::BlockQuote(_) => {}
            pulldown_cmark::TagEnd::CodeBlock => {
                self.end_code_block(ui, cache, options, max_width);
            }

            pulldown_cmark::TagEnd::List(_) => {
                if self.list.is_last_level() {
                    self.line.should_start_newline = true;
                    self.line.should_end_newline = true;
                }

                self.list.end_level(ui, self.line.can_insert_end());

                if !self.list.is_inside_a_list() {
                    // Reset all the state and make it ready for the next list that occurs
                    self.list = List::default();
                }
            }
            pulldown_cmark::TagEnd::Item => {}
            pulldown_cmark::TagEnd::FootnoteDefinition => {
                self.line.should_start_newline = true;
                self.line.should_end_newline = true;
                self.line.try_insert_end(ui);
            }
            pulldown_cmark::TagEnd::Table => {}
            pulldown_cmark::TagEnd::TableHead => {}
            pulldown_cmark::TagEnd::TableRow => {}
            pulldown_cmark::TagEnd::TableCell => {
                // Ensure space between cells
                ui.label("  ");
            }
            pulldown_cmark::TagEnd::Emphasis => {
                self.text_style.emphasis = false;
            }
            pulldown_cmark::TagEnd::Strong => {
                self.text_style.strong = false;
            }
            pulldown_cmark::TagEnd::Strikethrough => {
                self.text_style.strikethrough = false;
            }
            pulldown_cmark::TagEnd::Link => {
                if let Some(link) = self.link.take() {
                    link.end(ui, cache, options, &mut self.deferred_scroll_to_heading);
                }
            }
            pulldown_cmark::TagEnd::Image => {
                if let Some(image) = self.image.take() {
                    image.end(ui, options);
                }
            }
            pulldown_cmark::TagEnd::HtmlBlock => {
                if let Some(html_fn) = options.html_fn {
                    html_fn(ui, &self.html_block);
                    self.html_block.clear();
                }
            }

            pulldown_cmark::TagEnd::MetadataBlock(_) => {}

            pulldown_cmark::TagEnd::DefinitionList => self.line.try_insert_end(ui),
            pulldown_cmark::TagEnd::DefinitionListTitle
            | pulldown_cmark::TagEnd::DefinitionListDefinition => {}
            pulldown_cmark::TagEnd::Superscript | pulldown_cmark::TagEnd::Subscript => {}
        }
    }

    fn end_code_block(
        &mut self,
        ui: &mut Ui,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        max_width: f32,
    ) {
        if let Some(block) = self.code_block.take() {
            top_aligned(ui, self.list.is_inside_a_list(), |ui| {
                block.end(ui, cache, options, max_width)
            });
            self.line.try_insert_end(ui);
        }
    }
}

/// Markdowntown patch: события цитаты до её собственного `End(BlockQuote)` — с учётом
/// вложенных цитат (в отличие от `delayed_events`, который остановится на первом конце).
fn delayed_events_blockquote<'e>(
    events: &mut impl Iterator<Item = EventIteratorItem<'e>>,
) -> Vec<(pulldown_cmark::Event<'e>, Range<usize>)> {
    let mut depth = 0usize;
    let mut collected = Vec::new();
    for (_, (event, range)) in events {
        let end = match &event {
            pulldown_cmark::Event::Start(pulldown_cmark::Tag::BlockQuote(_)) => {
                depth += 1;
                false
            }
            pulldown_cmark::Event::End(pulldown_cmark::TagEnd::BlockQuote(_)) => {
                if depth == 0 {
                    true
                } else {
                    depth -= 1;
                    false
                }
            }
            _ => false,
        };
        collected.push((event, range));
        if end {
            break;
        }
    }
    collected
}

/// Markdowntown patch: минимальная (самое длинное слово) и естественная (в одну строку)
/// ширина содержимого ячейки таблицы. Приблизительно: по тексту и коду ячейки; жирный
/// текст (заголовок таблицы, `**…**`) — жирным шрифтом, он шире.
fn measure_cell(
    ui: &Ui,
    cell: &[(pulldown_cmark::Event<'_>, Range<usize>)],
    header: bool,
) -> (f32, f32) {
    let body = TextStyle::Body.resolve(ui.style());
    let mono = TextStyle::Monospace.resolve(ui.style());
    let bold = egui_commonmark_backend::misc::bold_family(ui)
        .map_or_else(|| body.clone(), |family| egui::FontId::new(body.size, family));
    let mut strong = 0usize;
    let width = |text: &str, font: &egui::FontId| {
        ui.fonts_mut(|f| {
            f.layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
                .size()
                .x
        })
    };
    let (mut min, mut natural) = (0.0f32, 0.0f32);
    for (event, _) in cell {
        let (text, font) = match event {
            pulldown_cmark::Event::Start(pulldown_cmark::Tag::Strong) => {
                strong += 1;
                continue;
            }
            pulldown_cmark::Event::End(pulldown_cmark::TagEnd::Strong) => {
                strong = strong.saturating_sub(1);
                continue;
            }
            pulldown_cmark::Event::Text(t) if header || strong > 0 => (t.as_ref(), &bold),
            pulldown_cmark::Event::Text(t) => (t.as_ref(), &body),
            pulldown_cmark::Event::Code(t) | pulldown_cmark::Event::InlineMath(t) => {
                (t.as_ref(), &mono)
            }
            _ => continue,
        };
        natural += width(text, font);
        for word in text.split_whitespace() {
            min = min.max(width(word, font));
        }
    }
    (min, natural.max(min))
}

/// Markdowntown patch: запас к замеренной ширине ячейки, px — замер приблизительный
/// (по тексту, а не по виджетам), и без запаса выровненная вправо ячейка могла бы
/// перенестись на вторую строку.
const ALIGN_SLACK: f32 = 4.0;

/// Markdowntown patch: сдвиг содержимого ячейки внутри столбца по выравниванию
/// (`:---:` — по центру, `---:` — вправо). Только для ячеек, которые помещаются в одну
/// строку: строки переноса в egui всегда идут от левого края.
pub fn cell_shift(alignment: Option<pulldown_cmark::Alignment>, inner: f32, natural: f32) -> f32 {
    let free = (inner - natural - ALIGN_SLACK).max(0.0);
    match alignment {
        Some(pulldown_cmark::Alignment::Center) => free / 2.0,
        Some(pulldown_cmark::Alignment::Right) => free,
        _ => 0.0,
    }
}

/// Markdowntown patch: ширины столбцов таблицы для доступной ширины `available` —
/// как автоматическая раскладка таблиц в браузере. Помещаются естественные ширины —
/// они и есть; иначе каждый столбец получает минимум, а остаток делится
/// пропорционально тому, насколько столбцу нужно больше минимума (длинные ячейки
/// переносятся, короткие — нет). Не помещаются даже минимумы — минимумы
/// (таблица прокручивается по горизонтали).
pub fn column_widths(min: &[f32], natural: &[f32], available: f32) -> Vec<f32> {
    let natural_total: f32 = natural.iter().sum();
    if natural_total <= available {
        return natural.to_vec();
    }
    let min_total: f32 = min.iter().sum();
    let extra = available - min_total;
    let wanted: f32 = natural.iter().zip(min).map(|(n, m)| n - m).sum();
    if extra <= 0.0 || wanted <= 0.0 {
        return min.to_vec();
    }
    min.iter()
        .zip(natural)
        .map(|(m, n)| m + extra * (n - m) / wanted)
        .collect()
}
