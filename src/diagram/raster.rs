//! Растеризация SVG в `egui::ColorImage` через `resvg`.

use std::sync::Arc;

use resvg::{tiny_skia, usvg};

/// Ограничение стороны картинки в пикселях — защита от огромных viewBox.
const MAX_SIDE: f32 = 8192.0;

pub struct Rasterized {
    pub image: egui::ColorImage,
    /// Размер диаграммы в логических точках egui (1 px SVG = 1 точка).
    pub size: egui::Vec2,
}

pub struct Rasterizer {
    options: usvg::Options<'static>,
}

impl Rasterizer {
    /// Загружает системные шрифты (Segoe UI, Arial и т.д.) — это занимает
    /// заметное время, поэтому создавать в фоновом потоке.
    pub fn new() -> Self {
        let mut fontdb = usvg::fontdb::Database::new();
        fontdb.load_system_fonts();
        Self {
            options: usvg::Options {
                fontdb: Arc::new(fontdb),
                ..Default::default()
            },
        }
    }

    /// `scale` — пикселей на точку (`pixels_per_point`), чтобы на HiDPI было чётко.
    pub fn rasterize(&self, svg: &str, scale: f32) -> Result<Rasterized, String> {
        let tree = usvg::Tree::from_str(svg, &self.options).map_err(|e| e.to_string())?;
        let size = tree.size();
        let scale = scale.min(MAX_SIDE / size.width().max(size.height()).max(1.0));
        let w = (size.width() * scale).ceil().max(1.0) as u32;
        let h = (size.height() * scale).ceil().max(1.0) as u32;
        let mut pixmap =
            tiny_skia::Pixmap::new(w, h).ok_or_else(|| format!("cannot allocate {w}x{h} image"))?;
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        Ok(Rasterized {
            // tiny-skia хранит RGBA с премультиплицированной альфой — как и ожидает egui.
            image: egui::ColorImage::from_rgba_premultiplied(
                [w as usize, h as usize],
                pixmap.data(),
            ),
            size: egui::vec2(size.width(), size.height()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="100" height="50" fill="red"/></svg>"#;

    #[test]
    fn rasterizes_with_scale() {
        let r = Rasterizer::new();
        let out = r.rasterize(SVG, 2.0).unwrap();
        assert_eq!(out.image.size, [200, 100]);
        assert_eq!(out.size, egui::vec2(100.0, 50.0));
        assert_eq!(out.image.pixels[0], egui::Color32::RED);
    }

    #[test]
    fn huge_svg_is_clamped() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100000" height="10"></svg>"#;
        let out = Rasterizer::new().rasterize(svg, 2.0).unwrap();
        assert!(out.image.size[0] <= MAX_SIDE as usize);
    }

    #[test]
    fn invalid_svg_is_error() {
        assert!(Rasterizer::new().rasterize("not svg", 1.0).is_err());
    }
}
