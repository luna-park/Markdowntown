//! Панель превью: рендер сегментов документа.

use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::document::Document;
use crate::segment::{self, Segment, SegmentKind};

#[derive(Default)]
pub struct Preview {
    md_cache: CommonMarkCache,
    /// Версия документа, для которой посчитаны `segments`.
    version: Option<(u64, u64)>,
    segments: Vec<Segment>,
}

impl Preview {
    pub fn show(&mut self, ui: &mut egui::Ui, doc: &mut Document) {
        if self.version != Some(doc.version()) {
            self.segments = segment::split(&doc.text);
            self.version = Some(doc.version());
        }

        let image_base = doc.base_dir().map(file_uri_prefix);

        for (i, seg) in self.segments.iter().enumerate() {
            let edited = ui
                .push_id(i, |ui| match &seg.kind {
                    SegmentKind::Markdown => {
                        show_markdown(ui, &mut self.md_cache, image_base.as_deref(), doc, seg)
                    }
                    SegmentKind::Mermaid { source } => {
                        show_mermaid_placeholder(ui, source);
                        false
                    }
                })
                .inner;
            if edited {
                // Диапазоны сегментов устарели — пересчитаем на следующем кадре.
                ui.ctx().request_repaint();
                break;
            }
        }
    }
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

fn show_mermaid_placeholder(ui: &mut egui::Ui, source: &str) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.weak("Mermaid diagram (rendering is not implemented yet)");
        ui.label(egui::RichText::new(source.trim_end()).monospace());
    });
    ui.add_space(8.0);
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
