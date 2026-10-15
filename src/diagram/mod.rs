//! Рендер диаграмм Mermaid: исходник → SVG → текстура egui в фоновом потоке.

mod merman_backend;
mod raster;
mod service;

pub use service::{DiagramKey, DiagramService, DiagramState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagramTheme {
    Light,
    Dark,
}

impl DiagramTheme {
    pub fn from_visuals(visuals: &egui::Visuals) -> Self {
        if visuals.dark_mode {
            Self::Dark
        } else {
            Self::Light
        }
    }
}

/// Движок Mermaid → SVG. SVG должен быть пригоден для `resvg` (без `<foreignObject>`).
pub trait DiagramRenderer: Send {
    fn render_svg(&self, source: &str, theme: DiagramTheme) -> Result<String, String>;
}
