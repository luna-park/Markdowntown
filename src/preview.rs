//! Панель превью: рендер сегментов документа.

use std::time::{Duration, Instant};

use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::diagram::{DiagramKey, DiagramService, DiagramState, DiagramTheme};
use crate::document::Document;
use crate::segment::{self, Segment, SegmentKind};
use crate::viewer::DiagramViewer;

/// Пауза в наборе, после которой перерисовываются диаграммы.
const DIAGRAM_DEBOUNCE: Duration = Duration::from_millis(500);

#[derive(Default)]
pub struct Preview {
    md_cache: CommonMarkCache,
    /// Создаётся при первом показе: нужен `egui::Context`.
    diagrams: Option<DiagramService>,
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
}

impl Preview {
    pub fn show(&mut self, ui: &mut egui::Ui, doc: &mut Document) {
        let ctx = ui.ctx().clone();
        let diagrams = self
            .diagrams
            .get_or_insert_with(|| DiagramService::new(&ctx));
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

        for (i, seg) in self.segments.iter().enumerate() {
            let edited = ui
                .push_id(i, |ui| match &seg.kind {
                    SegmentKind::Markdown => {
                        show_markdown(ui, &mut self.md_cache, image_base.as_deref(), doc, seg)
                    }
                    SegmentKind::Mermaid { source } => {
                        if self.shown.len() <= mermaid_index {
                            self.shown.resize(mermaid_index + 1, None);
                        }
                        let clicked = show_mermaid(
                            ui,
                            diagrams,
                            &mut self.shown[mermaid_index],
                            source,
                            &diagram_params,
                        );
                        if clicked {
                            self.viewer = Some(DiagramViewer::new(source));
                        }
                        mermaid_index += 1;
                        false
                    }
                })
                .inner;
            if edited {
                // Диапазоны сегментов устарели — пересчитаем на следующем кадре.
                ctx.request_repaint();
                break;
            }
        }

        if let Some(viewer) = &mut self.viewer
            && !viewer.show(&ctx, diagrams, diagram_params.theme)
        {
            self.viewer = None;
        }
        // После всех запросов кадра: отменить задания, которые больше не нужны.
        diagrams.end_frame();
    }
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

/// Рендерит Markdown-сегмент. Возвращает `true`, если превью изменило документ
/// (переключили чекбокс task-list).
fn show_markdown(
    ui: &mut egui::Ui,
    cache: &mut CommonMarkCache,
    image_base: Option<&str>,
    doc: &mut Document,
    seg: &Segment,
) -> bool {
    let mut viewer = CommonMarkViewer::new();
    if let Some(base) = image_base {
        viewer = viewer.default_implicit_uri_scheme(base);
    }
    let mut text = doc.text[seg.range.clone()].to_owned();
    let response = viewer.show_mut(ui, cache, &mut text);
    if response.response.changed() {
        doc.text.replace_range(seg.range.clone(), &text);
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
