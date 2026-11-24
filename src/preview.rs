//! Панель превью: рендер сегментов документа.

mod search;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::diagram::{DiagramKey, DiagramService, DiagramState, DiagramTheme};
use crate::document::Document;
use crate::i18n::{tr, trf};
use crate::math::MathCache;
use crate::scroll_sync::{PreviewGeometry, drag_scroll_delta};
use crate::segment::{self, Segment, SegmentKind};
use crate::style;
use crate::viewer::DiagramViewer;

/// Пауза в наборе, после которой перерисовываются диаграммы.
const DIAGRAM_DEBOUNCE: Duration = Duration::from_millis(500);

/// Размер основного текста превью, pt. Заголовки считаются от него в `egui_commonmark`.
const PREVIEW_BODY_SIZE: f32 = 14.0;

/// Ширина колонки превью при View → Readable width, pt: 50 em — ~85–90 символов в строке
/// (удобная для чтения длина строки — 45–90 символов). В pt — масштабируется вместе
/// с текстом (Ctrl+колесо), длина строки не меняется.
const READABLE_WIDTH: f32 = 50.0 * PREVIEW_BODY_SIZE;

/// Текст сносок относительно основного (GitHub: 12 px при 16 px, у нас 12 pt при 14 pt).
const FOOTNOTE_SCALE: f32 = 0.85;

/// Шрифты и цвета превью: размеры — от `PREVIEW_BODY_SIZE` (код чуть мельче, как на
/// GitHub), цвета текста — для чтения (см. [`style::apply_reading_colors`]).
fn apply_preview_style(ui: &mut egui::Ui) {
    let styles = &mut ui.style_mut().text_styles;
    for (style, scale) in [
        (egui::TextStyle::Body, 1.0),
        (egui::TextStyle::Button, 1.0),
        (egui::TextStyle::Monospace, 0.9),
        (egui::TextStyle::Small, 0.85),
    ] {
        if let Some(font) = styles.get_mut(&style) {
            font.size = PREVIEW_BODY_SIZE * scale;
        }
    }
    style::apply_reading_colors(ui);
}

#[derive(Default)]
pub struct Preview {
    /// Колонка по центру шириной не больше [`READABLE_WIDTH`] (View → Readable width).
    pub readable_width: bool,
    md_cache: CommonMarkCache,
    search: search::Search,
    /// Полоса документа, которую пересекло выделение. Сохраняем раскладку в ней,
    /// даже когда её начало или конец прокручены за пределы экрана.
    selection_span: Option<egui::Rangef>,
    selecting: bool,
    /// Версия документа, для которой посчитаны `segments`.
    version: Option<(u64, u64)>,
    segments: Vec<Segment>,
    /// Сноски, на которые есть ссылки, по порядку первой ссылки: индекс сегмента
    /// определения. Показываются в конце документа, как на GitHub.
    footnotes: Vec<usize>,
    /// Номера сносок по меткам — для `egui_commonmark`.
    footnote_numbers: HashMap<String, usize>,
    /// Время последней правки текущего документа; `None` — рендерить сразу.
    last_edit: Option<Instant>,
    /// Последняя успешно показанная диаграмма для каждого mermaid-блока (по порядку) —
    /// её показываем, пока рендерится новая версия.
    shown: Vec<Option<DiagramKey>>,
    /// Открытое окно просмотра диаграммы крупно.
    viewer: Option<DiagramViewer>,
    /// Высоты отрисованных Markdown-блоков (с отступом после блока) по ключу
    /// [`block_key`]: невидимые блоки не рисуются, а занимают запомненное место.
    heights: HashMap<u64, BlockHeight>,
    /// Номер кадра — чтобы подтверждать высоту замерами из разных кадров.
    frame: u64,
    /// Общий с `render_math_fn`: `egui_commonmark` требует `'static` замыкание.
    math: Rc<RefCell<MathCache>>,
}

impl Preview {
    /// Рисует превью в собственной области прокрутки. `scroll_to` — прокрутка от
    /// синхронизации с редактором. Возвращает якоря для синхронизации.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        doc: &mut Document,
        scroll_to: Option<f32>,
        preview_only: bool,
        diagrams: &mut DiagramService,
    ) -> PreviewGeometry {
        apply_preview_style(ui);
        self.frame += 1;
        self.math.borrow_mut().begin_frame();
        let math = Rc::clone(&self.math);
        let math_fn = move |ui: &mut egui::Ui, tex: &str, inline: bool| {
            math.borrow_mut().show(ui, tex, inline);
        };
        let ctx = ui.ctx().clone();
        diagrams.begin_frame(&ctx);

        if self.version != Some(doc.version()) {
            let same_doc = self.version.is_some_and(|(id, _)| id == doc.version().0);
            self.last_edit = same_doc.then(Instant::now);
            if !same_doc {
                self.shown.clear();
            }
            let split = segment::split_all(&doc.text);
            self.segments = split.segments;
            self.footnotes = split
                .footnotes
                .iter()
                .filter_map(|f| {
                    self.segments
                        .iter()
                        .position(|s| s.range.start == f.definition)
                })
                .collect();
            self.footnote_numbers = split
                .footnotes
                .into_iter()
                .enumerate()
                .map(|(i, f)| (f.label, i + 1))
                .collect();
            self.version = Some(doc.version());
        }

        let reveal = if preview_only {
            self.search.show(
                ui,
                &doc.text,
                doc.version(),
                &self.segments,
                &self.footnotes,
            )
        } else {
            None
        };

        let may_request = match self.last_edit {
            None => true,
            Some(t) => {
                let elapsed = t.elapsed();
                if elapsed < DIAGRAM_DEBOUNCE {
                    ctx.request_repaint_after(DIAGRAM_DEBOUNCE - elapsed);
                }
                elapsed >= DIAGRAM_DEBOUNCE
            }
        };
        let diagram_params = DiagramParams {
            theme: DiagramTheme::from_visuals(ui.visuals()),
            scale: ctx.pixels_per_point(),
            may_request,
        };

        let image_base = doc.base_dir().map(file_uri_prefix);
        let mut mermaid_index = 0;
        let mut anchors = Vec::new();
        let mut footnote_tops = Vec::new();
        // Порядок показа: блоки документа, затем сноски (`true` — в разделе сносок).
        let order: Vec<(usize, bool)> = (0..self.segments.len())
            .map(|i| (i, false))
            .chain(self.footnotes.iter().map(|&i| (i, true)))
            .collect();

        if (!self.selecting && !crate::preview_menu::has_selection(&ctx))
            || ctx.input(|i| i.pointer.primary_pressed() && !i.modifiers.shift)
        {
            self.selection_span = None;
        }
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("preview_scroll")
            .animated(reveal.is_none() && !self.selecting)
            .auto_shrink(false);
        if let Some(offset) = scroll_to {
            scroll = scroll.vertical_scroll_offset(offset);
        }
        // Отступ между блоками — как пустая строка между абзацами.
        let block_gap = ui.text_style_height(&egui::TextStyle::Body);
        let scale_bits = ctx.pixels_per_point().to_bits();
        if self.heights.len() > 100_000 {
            // Ключи старых версий блоков копятся при наборе — изредка сбрасываем.
            self.heights.clear();
        }

        let max_width = if self.readable_width {
            READABLE_WIDTH
        } else {
            f32::INFINITY
        };
        // Колонка — внутри области прокрутки: полоса прокрутки остаётся у края панели,
        // колесо мыши работает и над полями по бокам.
        let output = scroll.show_viewport(ui, |ui, viewport| {
            let view = viewport.y_range();
            let extend = |span: egui::Rangef| {
                egui::Rangef::new(span.min.min(view.min), span.max.max(view.max))
            };
            let keep = self
                .selection_span
                .map(|span| if self.selecting { extend(span) } else { span });
            egui_commonmark::begin_selection_tracking(&ctx, keep.is_some());
            centered_column(ui, max_width, |ui| {
                let content_top = ui.min_rect().top();
                let width = ui.available_width();
                let item_spacing = ui.spacing().item_spacing.y;
                for &(i, in_footnotes) in &order {
                    let seg = &self.segments[i];
                    if in_footnotes && footnote_tops.is_empty() {
                        footnotes_rule(ui, block_gap);
                    }
                    let top = ui.cursor().top() - content_top;
                    if in_footnotes {
                        // Якоря — по возрастанию байтов, поэтому у сносок внизу свой список.
                        footnote_tops.push((seg.range.start, top));
                    } else {
                        anchors.push((seg.range.start, top));
                    }
                    match &seg.kind {
                        // Определение сноски на своём месте не показывается — оно внизу.
                        SegmentKind::Markdown {
                            footnote: Some(_), ..
                        } if !in_footnotes => {}
                        SegmentKind::Markdown { appendix, .. } => {
                            let block = &doc.text[seg.range.clone()];
                            let mut key = block_key(block, appendix, width, scale_bits);
                            if in_footnotes {
                                key = key.rotate_left(1); // другой стиль — другая высота
                            }
                            if let Some(&BlockHeight {
                                height,
                                stable: true,
                                ..
                            }) = self.heights.get(&key)
                                && reveal != Some(i)
                                && keep.is_none_or(|span| {
                                    !span.intersects(egui::Rangef::new(top, top + height))
                                })
                                && (top + height < viewport.min.y || top > viewport.max.y)
                            {
                                // Невидимый блок: занять его место, не раскладывая текст.
                                ui.allocate_space(egui::vec2(
                                    width,
                                    (height - item_spacing).max(0.0),
                                ));
                                continue;
                            }
                            // Блок рисуется в дочерней области ровно по ширине панели и занимает
                            // в превью ровно эту ширину: если что-то в нём окажется шире,
                            // оно обрежется по краю, а не растянет (через `max_rect`) все
                            // следующие блоки.
                            let block_rect = egui::Rect::from_min_size(
                                ui.cursor().min,
                                egui::vec2(width, f32::INFINITY),
                            );
                            // Пропущенные и отрисованные блоки по-разному расходуют auto-id.
                            // Id выделенного текста должен переживать появление блоков выше него.
                            let block_id =
                                ui.make_persistent_id(("markdown_block", i, in_footnotes));
                            let mut block_ui = ui.new_child(
                                egui::UiBuilder::new()
                                    .id(block_id)
                                    .max_rect(block_rect)
                                    .layout(*ui.layout()),
                            );
                            if in_footnotes {
                                footnote_style(&mut block_ui);
                            }
                            let found_rect = Cell::new(None);
                            let render = RenderContext {
                                math_fn: &math_fn,
                                image_base: image_base.as_deref(),
                                footnote_numbers: &self.footnote_numbers,
                                highlights: preview_only
                                    .then(|| {
                                        self.search.highlights(
                                            i,
                                            (reveal == Some(i)).then_some(&found_rect),
                                        )
                                    })
                                    .flatten(),
                            };
                            let edited = show_markdown(
                                &mut block_ui,
                                &mut self.md_cache,
                                &render,
                                doc,
                                seg,
                                appendix,
                            );
                            let used = egui::Rect::from_min_size(
                                block_rect.min,
                                egui::vec2(width, block_ui.min_rect().height()),
                            );
                            ui.advance_cursor_after_rect(used);
                            if let Some(rect) = found_rect.get() {
                                ui.scroll_to_rect(rect, None);
                            }
                            if edited {
                                // Диапазоны сегментов устарели — пересчитаем на следующем кадре.
                                ctx.request_repaint();
                                break;
                            }
                            // Сноски — плотнее, как пункты списка на GitHub.
                            ui.add_space(if in_footnotes {
                                (block_gap * 0.4).round()
                            } else {
                                block_gap
                            });
                            let height = ui.cursor().top() - content_top - top;
                            let frame = self.frame;
                            let entry = self.heights.entry(key).or_insert(BlockHeight {
                                height,
                                frame,
                                stable: false,
                            });
                            if entry.frame != frame {
                                entry.stable = (entry.height - height).abs() < 0.5;
                                entry.height = height;
                                entry.frame = frame;
                            }
                        }
                        SegmentKind::Mermaid { source } => {
                            if self.shown.len() <= mermaid_index {
                                self.shown.resize(mermaid_index + 1, None);
                            }
                            let clicked = ui
                                .push_id(i, |ui| {
                                    show_mermaid(
                                        ui,
                                        diagrams,
                                        &mut self.shown[mermaid_index],
                                        source,
                                        &diagram_params,
                                    )
                                })
                                .inner;
                            if clicked {
                                self.viewer = Some(DiagramViewer::new(source));
                            }
                            mermaid_index += 1;
                        }
                    }
                }
            });
            let (pressed, dragged) = egui_commonmark::finish_selection_tracking(&ctx);
            self.selection_span = if pressed {
                Some(keep.map_or(view, extend))
            } else {
                keep
            };
            self.selecting = pressed;
            if dragged {
                scroll_during_selection(ui);
            }
        });

        if let Some(viewer) = &mut self.viewer
            && !viewer.show(&ctx, diagrams, diagram_params.theme)
        {
            self.viewer = None;
        }
        // После всех запросов кадра: отменить задания, которые больше не нужны.
        diagrams.end_frame();

        crate::preview_menu::note_area(&ctx, output.inner_rect);
        crate::preview_menu::show(&ctx);

        let viewport = output.inner_rect.height();
        PreviewGeometry {
            anchors,
            offset: output.state.offset.y,
            max_offset: (output.content_size.y - viewport).max(0.0),
            content_height: output.content_size.y,
            footnotes: footnote_tops,
        }
    }
}

/// ScrollArea блокирует колесо, пока другой виджет захватил мышь.
fn scroll_during_selection(ui: &egui::Ui) {
    let delta = ui.input(|i| {
        let wheel = if i.modifiers.command || i.modifiers.ctrl {
            0.0
        } else {
            i.smooth_scroll_delta().y
        };
        let edge = drag_scroll_delta(
            i.pointer.latest_pos().map(|p| p.y),
            ui.clip_rect().y_range(),
            i.stable_dt,
        )
        .unwrap_or_default();
        wheel - edge
    });
    if delta != 0.0 {
        ui.scroll_with_delta_animation(
            egui::vec2(0.0, delta),
            egui::style::ScrollAnimation::none(),
        );
    }
}

/// Раздел сносок в конце документа — под линией, как на GitHub.
fn footnotes_rule(ui: &mut egui::Ui, gap: f32) {
    ui.add_space(gap);
    let y = ui.painter().round_to_pixel_center(ui.cursor().top());
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter().hline(ui.max_rect().x_range(), y, stroke);
    ui.add_space(gap);
}

/// Текст сноски — мельче и приглушённее основного (как у GitHub).
fn footnote_style(ui: &mut egui::Ui) {
    for font in ui.style_mut().text_styles.values_mut() {
        font.size *= FOOTNOTE_SCALE;
    }
    ui.visuals_mut().override_text_color = Some(ui.visuals().weak_text_color());
}

/// Колонка по центру `ui` шириной не больше `max_width`.
fn centered_column<R>(
    ui: &mut egui::Ui,
    max_width: f32,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let full = ui.available_width();
    let width = full.min(max_width);
    // Целые точки: дробный сдвиг размыл бы текст.
    let left = ui.cursor().left() + ((full - width) / 2.0).floor();
    let rect = egui::Rect::from_min_size(
        egui::pos2(left, ui.cursor().top()),
        egui::vec2(width, f32::INFINITY),
    );
    // Границы колонки — едва заметный пунктир чуть снаружи, во всю видимую высоту
    // (неподвижен при прокрутке, как направляющая).
    const GUIDE_GAP: f32 = 16.0;
    if left - ui.cursor().left() > GUIDE_GAP * 2.0 {
        let visible = ui.clip_rect();
        let stroke = egui::Stroke::new(1.0, ui.visuals().weak_text_color().gamma_multiply(0.3));
        for x in [left - GUIDE_GAP, left + width + GUIDE_GAP] {
            // По центру пикселя (с учётом масштаба экрана) — линия без размытия.
            let x = ui.painter().round_to_pixel_center(x);
            ui.painter().extend(egui::Shape::dashed_line(
                &[
                    egui::pos2(x, visible.top()),
                    egui::pos2(x, visible.bottom()),
                ],
                stroke,
                3.0,
                5.0,
            ));
        }
    }
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), add)
        .inner
}

/// Запомненная высота блока. Пропускать блок можно, только когда высота подтверждена
/// одинаковыми замерами в двух разных кадрах: таблицы (`Grid`) и картинки устанавливают
/// свой размер не с первого кадра. Одинаковые блоки делят запись, поэтому замер
/// в том же кадре не считается подтверждением.
#[derive(Clone, Copy)]
struct BlockHeight {
    height: f32,
    frame: u64,
    stable: bool,
}

struct DiagramParams {
    theme: DiagramTheme,
    scale: f32,
    may_request: bool,
}

/// Владеющая копия [`DiagramState`] — чтобы не держать заём сервиса.
enum Diagram {
    Ready(egui::TextureHandle, egui::Vec2),
    Failed(String),
    Pending,
}

impl From<DiagramState<'_>> for Diagram {
    fn from(state: DiagramState<'_>) -> Self {
        match state {
            DiagramState::Ready { texture, size } => Self::Ready(texture.clone(), size),
            DiagramState::Failed(e) => Self::Failed(e.to_owned()),
            DiagramState::Pending => Self::Pending,
        }
    }
}

/// Возвращает `true`, если по диаграмме кликнули (открыть крупно).
fn show_mermaid(
    ui: &mut egui::Ui,
    diagrams: &mut DiagramService,
    shown: &mut Option<DiagramKey>,
    source: &str,
    params: &DiagramParams,
) -> bool {
    let key = DiagramKey::new(source, params.theme, params.scale);
    let current: Diagram = diagrams.get(key, source, params.may_request).into();
    let previous: Option<Diagram> = shown
        .filter(|prev| *prev != key)
        .map(|prev| diagrams.peek(prev).into());

    let mut clicked = false;
    match current {
        Diagram::Ready(texture, size) => {
            let response = show_diagram_image(ui, &texture, size, false)
                .on_hover_cursor(egui::CursorIcon::ZoomIn)
                .on_hover_text(tr!("Click to enlarge", "Нажмите, чтобы увеличить"));
            clicked = response.clicked();
            *shown = Some(key);
        }
        Diagram::Failed(error) => {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    trf!("Mermaid error: {error}", "Ошибка Mermaid: {error}"),
                );
                if let Some(Diagram::Ready(texture, size)) = &previous {
                    show_diagram_image(ui, texture, *size, true);
                } else {
                    ui.label(egui::RichText::new(source.trim_end()).monospace().weak());
                }
            });
        }
        Diagram::Pending => match &previous {
            Some(Diagram::Ready(texture, size)) => {
                show_diagram_image(ui, texture, *size, true);
            }
            _ => {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak(tr!("Rendering diagram…", "Диаграмма рисуется…"));
                    });
                });
            }
        },
    }
    ui.add_space(8.0);
    clicked
}

/// Диаграмма по центру; шире панели — уменьшается. `stale` — приглушённая
/// предыдущая версия, пока рендерится новая.
fn show_diagram_image(
    ui: &mut egui::Ui,
    texture: &egui::TextureHandle,
    size: egui::Vec2,
    stale: bool,
) -> egui::Response {
    let available = ui.available_width();
    let size = if size.x > available {
        size * (available / size.x)
    } else {
        size
    };
    let mut image = egui::Image::from_texture(egui::load::SizedTexture::new(texture.id(), size));
    if stale {
        image = image.tint(egui::Color32::from_white_alpha(110));
    }
    ui.vertical_centered(|ui| ui.add(image.sense(egui::Sense::click())))
        .inner
}

/// Ключ кэша высоты Markdown-блока: от текста (с невидимым хвостом), ширины и масштаба
/// зависит раскладка.
fn block_key(block: &str, appendix: &str, width: f32, scale_bits: u32) -> u64 {
    let mut h = DefaultHasher::new();
    block.hash(&mut h);
    appendix.hash(&mut h);
    width.to_bits().hash(&mut h);
    scale_bits.hash(&mut h);
    h.finish()
}

/// Общее для всех блоков кадра.
struct RenderContext<'a> {
    math_fn: &'a egui_commonmark::RenderMathFn,
    /// Префикс относительных путей картинок (папка документа).
    image_base: Option<&'a str>,
    footnote_numbers: &'a HashMap<String, usize>,
    highlights: Option<egui_commonmark::TextHighlights<'a>>,
}

/// Рендерит Markdown-блок. `appendix` — невидимый хвост (определения ссылок и сносок
/// из других блоков). Возвращает `true`, если превью изменило документ
/// (переключили чекбокс task-list).
fn show_markdown(
    ui: &mut egui::Ui,
    cache: &mut CommonMarkCache,
    render: &RenderContext,
    doc: &mut Document,
    seg: &Segment,
    appendix: &str,
) -> bool {
    let block_len = seg.range.len();
    let mut viewer = CommonMarkViewer::new()
        .render_until(block_len)
        .render_math_fn(Some(render.math_fn))
        .footnote_numbers(render.footnote_numbers)
        .text_highlights(render.highlights)
        .alerts(crate::alerts::preview_bundle(ui.visuals().dark_mode));
    if let Some(base) = render.image_base {
        viewer = viewer.default_implicit_uri_scheme(base);
    }
    let mut text = String::with_capacity(block_len + appendix.len());
    text.push_str(&doc.text[seg.range.clone()]);
    text.push_str(appendix);
    let response = viewer.show_mut(ui, cache, &mut text);
    if response.response.changed() {
        // Чекбокс меняет «[ ]» на «[x]» — длина блока прежняя, хвост не трогается.
        doc.text
            .replace_range(seg.range.clone(), &text[..text.len() - appendix.len()]);
        doc.mark_edited();
        return true;
    }
    false
}

/// Префикс для относительных путей картинок в формате, который понимает
/// файловый загрузчик `egui_extras`: `file:///F:/docs/`, `file:///home/u/`,
/// а UNC-путь `\\server\share` — `file://server/share/`.
fn file_uri_prefix(dir: &std::path::Path) -> String {
    let dir = dir.to_string_lossy().replace('\\', "/");
    let dir = dir.trim_end_matches('/');
    match dir.strip_prefix("//") {
        Some(unc) => format!("file://{unc}/"),
        None => format!("file:///{}/", dir.trim_start_matches('/')),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn select_while_scrolling(wheel: bool, up: bool) {
        use egui::{Event, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect, vec2};

        let ctx = egui::Context::default();
        let mut preview = Preview::default();
        let mut diagrams = DiagramService::new(&ctx);
        let text: String = (0..100)
            .map(|i| format!("Paragraph {i:02} has some text.\n\n"))
            .collect();
        let mut doc = Document::new_untitled(text);
        let mut frame = |events, offset| {
            let mut geometry = None;
            let rect = Rect::from_min_size(Pos2::ZERO, vec2(500.0, 250.0));
            let mut input = egui::RawInput {
                screen_rect: Some(rect),
                events,
                ..Default::default()
            };
            crate::preview_menu::filter_input(&ctx, &mut input);
            let mut output = ctx.run_ui(input, |ui| {
                if up {
                    egui::Panel::top("test_tabs")
                        .exact_size(55.0)
                        .show(ui, |_| {});
                }
                egui::Panel::bottom("test_status")
                    .exact_size(60.0)
                    .show(ui, |_| {});
                egui::CentralPanel::default().show(ui, |ui| {
                    geometry = Some(preview.show(ui, &mut doc, offset, true, &mut diagrams));
                });
            });
            output.textures_delta.clear();
            (geometry.unwrap(), output)
        };
        let before = if up { 690.0 } else { 0.0 };
        frame(vec![], Some(before));
        let (_, initial) = frame(vec![], Some(before));
        let start_text = if up { "Paragraph 22" } else { "Paragraph 00" };
        let pos = initial
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text().starts_with(start_text) => {
                    Some(text.pos)
                }
                _ => None,
            })
            .unwrap();
        let start = pos + vec2(1.0, 6.0);
        let end = if up {
            egui::pos2(pos.x + 55.0, 2.0)
        } else {
            pos + vec2(55.0, if wheel { 100.0 } else { 224.0 })
        };
        let button = |pos, pressed| Event::PointerButton {
            pos,
            pressed,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        };
        frame(vec![Event::PointerMoved(start)], None);
        frame(vec![button(start, true)], None);
        frame(vec![Event::PointerMoved(end)], None);
        let mut offset = 0.0;
        for _ in 0..24 {
            let events = if wheel {
                vec![Event::MouseWheel {
                    unit: MouseWheelUnit::Point,
                    phase: egui::TouchPhase::Move,
                    delta: vec2(0.0, -30.0),
                    modifiers: Modifiers::NONE,
                }]
            } else {
                vec![]
            };
            offset = frame(events, None).0.offset;
        }
        assert!(
            if up {
                offset < before - 180.0
            } else {
                offset > 180.0
            },
            "selection scroll offset: {offset}"
        );
        frame(vec![button(end, false)], None);
        let after_release = frame(vec![], None).0.offset;
        frame(
            vec![Event::PointerMoved(egui::pos2(pos.x + 55.0, 100.0))],
            None,
        );
        let (after_wheel, _) = frame(
            vec![Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                phase: egui::TouchPhase::Move,
                delta: vec2(0.0, -300.0),
                modifiers: Modifiers::NONE,
            }],
            None,
        );
        assert!(
            after_wheel.offset > after_release,
            "wheel after release: {}",
            after_wheel.offset
        );
        let (_, output) = frame(vec![Event::Copy], None);
        let copied = output
            .platform_output
            .commands
            .iter()
            .find_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text.as_str()),
                _ => None,
            });
        assert!(
            copied.is_some_and(|text| if up {
                text.contains("Paragraph 14") && text.contains("Paragraph 21")
            } else {
                text.starts_with("Paragraph 00") && text.contains("Paragraph 09")
            }),
            "selected range after scroll: {copied:?}"
        );
    }

    #[test]
    fn mouse_drag_scrolls_and_keeps_preview_selection() {
        select_while_scrolling(false, false);
    }

    #[test]
    fn wheel_scrolls_during_preview_selection() {
        select_while_scrolling(true, false);
    }

    #[test]
    fn mouse_drag_scrolls_up_and_keeps_preview_selection() {
        select_while_scrolling(false, true);
    }

    #[test]
    fn hard_break_selection_copies_first_line_without_covering_next_glyph() {
        use egui::{Event, Modifiers, PointerButton};

        let ctx = egui::Context::default();
        crate::style::apply_global(&ctx);
        let mut cache = CommonMarkCache::default();
        let mut frame = |events| {
            let mut input = egui::RawInput {
                events,
                ..Default::default()
            };
            crate::preview_menu::filter_input(&ctx, &mut input);
            let mut output = ctx.run_ui(input, |ui| {
                crate::preview_menu::note_area(ui.ctx(), ui.max_rect());
                CommonMarkViewer::new().show(
                    ui,
                    &mut cache,
                    "Первая строка  \nВторая строка  \nТретья строка",
                );
            });
            output.textures_delta.clear();
            output
        };
        let first = frame(vec![]);
        let rows: Vec<_> = first
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text().ends_with("строка") => {
                    Some(text.pos)
                }
                _ => None,
            })
            .collect();
        assert_eq!(rows.len(), 3);
        let select_from = rows[0] + egui::vec2(1.0, 4.0);
        let select_to = rows[2] + egui::vec2(30.0, 4.0);
        let button = |pos, pressed| Event::PointerButton {
            pos,
            pressed,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        };
        frame(vec![Event::PointerMoved(select_from)]);
        frame(vec![
            button(select_from, true),
            Event::PointerMoved(select_to),
        ]);
        frame(vec![]);
        frame(vec![button(select_to, false)]);
        let copied = frame(vec![Event::Copy]);
        assert!(copied.platform_output.commands.iter().any(|command|
            matches!(command, egui::OutputCommand::CopyText(text) if text.starts_with("Первая строка\nВторая строка"))),
            "{:?}", copied.platform_output.commands);
        assert!(
            !copied.shapes.iter().any(|clipped| matches!(&clipped.shape,
                egui::Shape::Text(text) if text.galley.text() == "\n" &&
                    text.galley.rows.iter().any(|row| !row.visuals.mesh.indices.is_empty())
            )),
            "a layout newline must not paint a selection over a text row"
        );
    }

    /// Выравнивание однострочных ячеек таблицы (патч `egui_commonmark`).
    #[test]
    fn table_cell_shift() {
        use egui_commonmark::cell_shift;
        use pulldown_cmark::Alignment;
        // Столбец 100, текст 40, запас 4: свободно 56.
        assert_eq!(cell_shift(Some(Alignment::Right), 100.0, 40.0), 56.0);
        assert_eq!(cell_shift(Some(Alignment::Center), 100.0, 40.0), 28.0);
        assert_eq!(cell_shift(Some(Alignment::Left), 100.0, 40.0), 0.0);
        assert_eq!(cell_shift(None, 100.0, 40.0), 0.0);
        // Не помещается в строку — без сдвига (переносится от левого края).
        assert_eq!(cell_shift(Some(Alignment::Right), 100.0, 150.0), 0.0);
    }

    /// Ширины столбцов таблицы (патч `egui_commonmark`, см. vendor/README.md).
    #[test]
    fn table_column_widths() {
        use egui_commonmark::column_widths;
        // Естественные ширины помещаются — без переносов.
        assert_eq!(
            column_widths(&[10.0, 20.0], &[50.0, 60.0], 200.0),
            [50.0, 60.0]
        );
        // Не помещаются: короткий столбец не растёт, длинный забирает остаток.
        let w = column_widths(&[40.0, 30.0], &[40.0, 500.0], 200.0);
        assert_eq!(w[0], 40.0);
        assert!((w[1] - 160.0).abs() < 1e-3);
        // Не помещаются даже минимумы — минимумы (горизонтальная прокрутка).
        assert_eq!(
            column_widths(&[100.0, 120.0], &[300.0, 400.0], 150.0),
            [100.0, 120.0]
        );
    }

    #[test]
    fn file_uri_prefix_uses_forward_slashes() {
        assert_eq!(
            file_uri_prefix(std::path::Path::new(r"F:\docs\notes")),
            "file:///F:/docs/notes/"
        );
        assert_eq!(
            file_uri_prefix(std::path::Path::new(r"\\server\share\docs")),
            "file://server/share/docs/"
        );
        assert_eq!(
            file_uri_prefix(std::path::Path::new("/home/u/")),
            "file:///home/u/"
        );
    }
}
