//! Панель превью: рендер сегментов документа.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::diagram::{DiagramKey, DiagramService, DiagramState, DiagramTheme};
use crate::document::Document;
use crate::math::MathCache;
use crate::scroll_sync::PreviewGeometry;
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
    /// Версия документа, для которой посчитаны `segments`.
    version: Option<(u64, u64)>,
    segments: Vec<Segment>,
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
            self.segments = segment::split(&doc.text);
            self.version = Some(doc.version());
        }

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

        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("preview_scroll")
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
            centered_column(ui, max_width, |ui| {
                let content_top = ui.min_rect().top();
                let width = ui.available_width();
                let item_spacing = ui.spacing().item_spacing.y;
                for (i, seg) in self.segments.iter().enumerate() {
                    let top = ui.cursor().top() - content_top;
                    anchors.push((seg.range.start, top));
                    match &seg.kind {
                        SegmentKind::Markdown { appendix } => {
                            let block = &doc.text[seg.range.clone()];
                            let key = block_key(block, appendix, width, scale_bits);
                            if let Some(&BlockHeight {
                                height,
                                stable: true,
                                ..
                            }) = self.heights.get(&key)
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
                            let mut block_ui = ui.new_child(
                                egui::UiBuilder::new()
                                    .id_salt(i)
                                    .max_rect(block_rect)
                                    .layout(*ui.layout()),
                            );
                            let edited = show_markdown(
                                &mut block_ui,
                                &mut self.md_cache,
                                &math_fn,
                                image_base.as_deref(),
                                doc,
                                seg,
                                appendix,
                            );
                            let used = egui::Rect::from_min_size(
                                block_rect.min,
                                egui::vec2(width, block_ui.min_rect().height()),
                            );
                            ui.advance_cursor_after_rect(used);
                            if edited {
                                // Диапазоны сегментов устарели — пересчитаем на следующем кадре.
                                ctx.request_repaint();
                                break;
                            }
                            ui.add_space(block_gap);
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
            })
        });

        if let Some(viewer) = &mut self.viewer
            && !viewer.show(&ctx, diagrams, diagram_params.theme)
        {
            self.viewer = None;
        }
        // После всех запросов кадра: отменить задания, которые больше не нужны.
        diagrams.end_frame();

        let viewport = output.inner_rect.height();
        PreviewGeometry {
            anchors,
            offset: output.state.offset.y,
            max_offset: (output.content_size.y - viewport).max(0.0),
            content_height: output.content_size.y,
        }
    }
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
                .on_hover_text("Click to enlarge");
            clicked = response.clicked();
            *shown = Some(key);
        }
        Diagram::Failed(error) => {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("Mermaid error: {error}"),
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
                        ui.weak("Rendering diagram…");
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

/// Рендерит Markdown-блок. `appendix` — невидимый хвост (определения ссылок и сносок
/// из других блоков). Возвращает `true`, если превью изменило документ
/// (переключили чекбокс task-list).
fn show_markdown(
    ui: &mut egui::Ui,
    cache: &mut CommonMarkCache,
    math_fn: &egui_commonmark::RenderMathFn,
    image_base: Option<&str>,
    doc: &mut Document,
    seg: &Segment,
    appendix: &str,
) -> bool {
    let block_len = seg.range.len();
    let mut viewer = CommonMarkViewer::new()
        .render_until(block_len)
        .render_math_fn(Some(math_fn));
    if let Some(base) = image_base {
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
