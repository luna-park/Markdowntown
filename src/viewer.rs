//! Окно просмотра диаграммы крупно: масштаб, прокрутка, перетаскивание.

use crate::diagram::{DiagramKey, DiagramService, DiagramState, DiagramTheme};

const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 8.0;
/// Растр рендерится с кратностью не больше этой — выше картинка просто растягивается.
const MAX_RASTER_MULTIPLIER: f32 = 4.0;

pub struct DiagramViewer {
    source: String,
    zoom: f32,
    /// Подогнать масштаб под окно, как только станет известен размер диаграммы.
    fit_pending: bool,
    /// Прокрутка и видимая область с прошлого кадра — для масштаба вокруг точки.
    offset: egui::Vec2,
    viewport: egui::Rect,
    /// Новая прокрутка, которую нужно применить в этом кадре.
    pending_offset: Option<egui::Vec2>,
}

impl DiagramViewer {
    pub fn new(source: &str) -> Self {
        Self {
            source: source.to_owned(),
            zoom: 1.0,
            fit_pending: true,
            offset: egui::Vec2::ZERO,
            viewport: egui::Rect::NOTHING,
            pending_offset: None,
        }
    }

    /// Рисует окно. Возвращает `false`, когда окно закрыли.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        diagrams: &mut DiagramService,
        theme: DiagramTheme,
    ) -> bool {
        let ppp = ctx.pixels_per_point();
        let base_key = DiagramKey::new(&self.source, theme, ppp);
        // При увеличении нужен растр плотнее, иначе картинка размоется.
        let multiplier = self.zoom.clamp(1.0, MAX_RASTER_MULTIPLIER).ceil();
        let sharp_key = base_key.with_scale(ppp * multiplier);

        let sharp = ready(diagrams.get(sharp_key, &self.source, true));
        let image = sharp.or_else(|| ready(diagrams.get(base_key, &self.source, true)));
        let error = match diagrams.peek(base_key) {
            DiagramState::Failed(e) => Some(e.to_owned()),
            _ => None,
        };

        let mut open = true;
        egui::Window::new("Diagram")
            .id(egui::Id::new("diagram_viewer"))
            .open(&mut open)
            .default_size([900.0, 650.0])
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| self.contents(ui, image, error));

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            open = false;
        }
        open
    }

    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        image: Option<(egui::TextureHandle, egui::Vec2)>,
        error: Option<String>,
    ) {
        if let Some(error) = error {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("Mermaid error: {error}"),
            );
            return;
        }
        let Some((texture, size)) = image else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Rendering diagram…");
            });
            return;
        };

        ui.horizontal(|ui| {
            if ui.button("−").on_hover_text("Zoom out").clicked() {
                self.zoom_at(size, self.zoom / 1.25, None);
            }
            if ui.button("+").on_hover_text("Zoom in").clicked() {
                self.zoom_at(size, self.zoom * 1.25, None);
            }
            if ui.button("100%").clicked() {
                self.zoom_at(size, 1.0, None);
            }
            if ui.button("Fit").clicked() {
                self.fit_pending = true;
            }
            ui.label(format!("{:.0}%", self.zoom * 100.0));
            ui.weak("Ctrl+wheel to zoom, drag to pan, Esc to close");
        });
        ui.separator();

        if self.fit_pending {
            let viewport = ui.available_size();
            let fit = (viewport.x / size.x).min(viewport.y / size.y);
            self.zoom = fit.clamp(MIN_ZOOM, 1.0);
            self.pending_offset = Some(egui::Vec2::ZERO);
            self.fit_pending = false;
        }

        // Ctrl+колесо (и жест «щипок») над диаграммой — масштаб вокруг курсора.
        if self.viewport.is_positive() {
            let (zoom_delta, pointer) = ui.input(|i| (i.zoom_delta(), i.pointer.hover_pos()));
            if zoom_delta != 1.0
                && let Some(pointer) = pointer.filter(|p| self.viewport.contains(*p))
            {
                self.zoom_at(size, self.zoom * zoom_delta, Some(pointer));
            }
        }

        let content = size * self.zoom;
        let mut scroll = egui::ScrollArea::both()
            .auto_shrink(false)
            .scroll_source(egui::scroll_area::ScrollSource::ALL);
        if let Some(offset) = self.pending_offset.take() {
            scroll = scroll.scroll_offset(offset);
        }
        let output = scroll.show(ui, |ui| {
            // Меньше окна — по центру, больше — с прокруткой.
            let area = content.max(ui.available_size());
            let (rect, _) = ui.allocate_exact_size(area, egui::Sense::hover());
            egui::Image::from_texture(egui::load::SizedTexture::new(texture.id(), content))
                .paint_at(ui, egui::Rect::from_center_size(rect.center(), content));
        });
        self.offset = output.state.offset;
        self.viewport = output.inner_rect;
    }

    /// Меняет масштаб так, чтобы точка `anchor` (по умолчанию центр окна) осталась на месте.
    fn zoom_at(&mut self, size: egui::Vec2, zoom: f32, anchor: Option<egui::Pos2>) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        if zoom == self.zoom || !self.viewport.is_positive() {
            self.zoom = zoom;
            return;
        }
        let anchor = anchor.map_or(self.viewport.size() / 2.0, |p| p - self.viewport.min);
        self.pending_offset = Some(scroll_after_zoom(
            size,
            self.viewport.size(),
            self.offset,
            anchor,
            self.zoom,
            zoom,
        ));
        self.zoom = zoom;
    }
}

/// Прокрутка после смены масштаба `from` → `to`, при которой точка диаграммы под
/// `anchor` (координаты внутри видимой области) остаётся под ним. Учитывает, что
/// диаграмма меньше окна рисуется по центру.
fn scroll_after_zoom(
    size: egui::Vec2,
    viewport: egui::Vec2,
    offset: egui::Vec2,
    anchor: egui::Vec2,
    from: f32,
    to: f32,
) -> egui::Vec2 {
    let origin = |zoom: f32| {
        let content = size * zoom;
        (content.max(viewport) - content) / 2.0
    };
    // Точка диаграммы в её собственных координатах (масштаб 1).
    let point = (offset + anchor - origin(from)) / from;
    let new_offset = origin(to) + point * to - anchor;
    let max_offset = (size * to - viewport).max(egui::Vec2::ZERO);
    new_offset.clamp(egui::Vec2::ZERO, max_offset)
}

fn ready(state: DiagramState<'_>) -> Option<(egui::TextureHandle, egui::Vec2)> {
    match state {
        DiagramState::Ready { texture, size } => Some((texture.clone(), size)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    #[test]
    fn zoom_keeps_anchor_point_in_place() {
        let size = vec2(1000.0, 800.0);
        let viewport = vec2(500.0, 400.0);
        // Масштаб 1, прокручено к (100, 50), якорь — центр окна → точка (350, 250).
        let offset = scroll_after_zoom(size, viewport, vec2(100.0, 50.0), viewport / 2.0, 1.0, 2.0);
        // При масштабе 2 точка (350, 250) → (700, 500); минус якорь (250, 200).
        assert_eq!(offset, vec2(450.0, 300.0));
    }

    #[test]
    fn zoom_out_to_smaller_than_viewport_resets_scroll() {
        let offset = scroll_after_zoom(
            vec2(1000.0, 800.0),
            vec2(500.0, 400.0),
            vec2(300.0, 300.0),
            vec2(10.0, 10.0),
            1.0,
            0.25,
        );
        assert_eq!(offset, egui::Vec2::ZERO);
    }

    #[test]
    fn zoom_in_from_centered_small_diagram() {
        // Диаграмма 100x100 в окне 400x400 по центру; якорь в центре окна = центр диаграммы.
        let offset = scroll_after_zoom(
            vec2(100.0, 100.0),
            vec2(400.0, 400.0),
            egui::Vec2::ZERO,
            vec2(200.0, 200.0),
            1.0,
            8.0,
        );
        // При 8x диаграмма 800x800; центр (400, 400) под центром окна → прокрутка (200, 200).
        assert_eq!(offset, vec2(200.0, 200.0));
    }
}
