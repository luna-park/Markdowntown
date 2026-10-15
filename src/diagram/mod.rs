//! Рендер диаграмм Mermaid: исходник → SVG → текстура egui в фоновом потоке.

mod merman_backend;
mod raster;
mod service;

pub use merman_backend::MermanRenderer;
pub use raster::Rasterizer;
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

/// Токен отмены задания рендера: клонируется, `cancel()` видна всем копиям.
#[derive(Clone, Debug)]
pub struct RenderControl(merman::OperationControl);

impl RenderControl {
    pub fn new() -> Self {
        Self(merman::OperationControl::new())
    }

    pub fn cancel(&self) {
        self.0.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

/// Движок Mermaid → SVG. SVG должен быть пригоден для `resvg` (без `<foreignObject>`).
pub trait DiagramRenderer: Send {
    /// Должен прерываться, если `control` отменён (тогда ошибка не показывается).
    fn render_svg(
        &self,
        source: &str,
        theme: DiagramTheme,
        control: &RenderControl,
    ) -> Result<String, String>;
}
