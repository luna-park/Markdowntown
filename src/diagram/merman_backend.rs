//! Реализация [`DiagramRenderer`] на крейте `merman`.

use merman::svg::SvgPipeline;
use merman::{
    Engine, MermaidConfig, OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest,
};

use super::{DiagramRenderer, DiagramTheme};

/// По одному `Renderer` на тему: тема задаётся конфигом хоста при создании.
pub struct MermanRenderer {
    light: Renderer,
    dark: Renderer,
}

impl MermanRenderer {
    pub fn new() -> Self {
        Self {
            light: Renderer::new(),
            dark: Renderer::new().with_engine(Engine::new().with_site_config(
                MermaidConfig::from_value(serde_json::json!({ "theme": "dark" })),
            )),
        }
    }
}

impl DiagramRenderer for MermanRenderer {
    fn render_svg(&self, source: &str, theme: DiagramTheme) -> Result<String, String> {
        let renderer = match theme {
            DiagramTheme::Light => &self.light,
            DiagramTheme::Dark => &self.dark,
        };
        let request = SvgRequest {
            // resvg не умеет <foreignObject> — без этого пропадут подписи.
            pipeline: Some(SvgPipeline::resvg_safe()),
            ..Default::default()
        };
        match renderer
            .render(RenderRequest::svg(source, OperationControl::new(), request))
            .map_err(|e| e.to_string())?
        {
            RenderOutput::Svg(Some(svg)) => Ok(svg.svg().to_owned()),
            _ => Err("No Mermaid diagram detected".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_both_themes() {
        let r = MermanRenderer::new();
        for theme in [DiagramTheme::Light, DiagramTheme::Dark] {
            let svg = r
                .render_svg("flowchart TD\n  A[Старт] --> B", theme)
                .unwrap();
            assert!(svg.contains("<svg"));
            assert!(!svg.contains("<foreignObject"));
        }
    }

    #[test]
    fn reports_syntax_errors() {
        let r = MermanRenderer::new();
        let err = r
            .render_svg("flowchart TD\n  A[open --> B", DiagramTheme::Light)
            .unwrap_err();
        assert!(err.contains("parse error"), "{err}");
        assert!(r.render_svg("", DiagramTheme::Light).is_err());
    }
}
