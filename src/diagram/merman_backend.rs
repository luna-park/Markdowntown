//! Реализация [`DiagramRenderer`] на крейте `merman`.

use merman::svg::SvgPipeline;
use std::time::Duration;

use merman::{Engine, MermaidConfig, RenderOutput, RenderRequest, Renderer, SvgRequest};

use super::{DiagramRenderer, DiagramTheme, RenderControl};

/// Предел времени на одну диаграмму — защита от зависания на огромном исходнике.
const RENDER_DEADLINE: Duration = Duration::from_secs(10);

/// По одному `Renderer` на тему: тема задаётся конфигом хоста при создании.
pub struct MermanRenderer {
    light: Renderer,
    dark: Renderer,
}

impl MermanRenderer {
    pub fn new() -> Self {
        Self {
            light: Renderer::new(),
            dark: Renderer::new().with_engine(
                Engine::new().with_site_config(MermaidConfig::from_value(dark_config())),
            ),
        }
    }
}

/// Тёмная тема mermaid с исправленными палитрами.
///
/// В штатной `dark` часть категориальных цветов почти чёрная (`cScale1` = `#0b0000`,
/// `cScale9`, `pie1`…) и сливается с тёмным фоном: страдают mindmap, timeline, pie.
/// Заменяем их цветами средней яркости, на которых читаются светлые подписи.
fn dark_config() -> serde_json::Value {
    // Приглушённые тона: фон узлов mindmap/timeline со светлым текстом.
    const SCALE: [&str; 12] = [
        "#3b4c63", "#2f5f55", "#5c4170", "#6e4d2c", "#2f5384", "#723b3b", "#4b6131", "#6f5a24",
        "#2f6170", "#643c5f", "#4a4f78", "#2d6a6a",
    ];
    // Насыщенные тона для секторов pie (рисуются с прозрачностью `pieOpacity`).
    const PIE: [&str; 12] = [
        "#4e79a7", "#e15759", "#59a14f", "#b07aa1", "#f28e2b", "#76b7b2", "#9c755f", "#a68a1f",
        "#5f6fc4", "#d4708f", "#3f9a8c", "#8a8f99",
    ];

    let mut vars = serde_json::Map::new();
    for (i, color) in SCALE.iter().enumerate() {
        vars.insert(format!("cScale{i}"), (*color).into());
        vars.insert(format!("cScaleLabel{i}"), "#e8e8e8".into());
    }
    for (i, color) in PIE.iter().enumerate() {
        vars.insert(format!("pie{}", i + 1), (*color).into());
    }
    // Корень mindmap и основная ветка gitGraph: штатно серый с тёмным текстом.
    vars.insert("git0".into(), "#4b6a9b".into());
    vars.insert("gitBranchLabel0".into(), "#ffffff".into());
    vars.insert("pieOpacity".into(), "0.85".into());
    vars.insert("pieSectionTextColor".into(), "#ffffff".into());
    vars.insert("pieStrokeColor".into(), "#1b1b1b".into());
    vars.insert("pieOuterStrokeColor".into(), "#1b1b1b".into());

    serde_json::json!({ "theme": "dark", "themeVariables": vars })
}

impl DiagramRenderer for MermanRenderer {
    fn render_svg(
        &self,
        source: &str,
        theme: DiagramTheme,
        control: &RenderControl,
    ) -> Result<String, String> {
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
            .render(RenderRequest::svg(
                source,
                control.0.clone().with_deadline(RENDER_DEADLINE),
                request,
            ))
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

    fn render(r: &MermanRenderer, source: &str, theme: DiagramTheme) -> Result<String, String> {
        r.render_svg(source, theme, &RenderControl::new())
    }

    #[test]
    fn renders_both_themes() {
        let r = MermanRenderer::new();
        for theme in [DiagramTheme::Light, DiagramTheme::Dark] {
            let svg = render(&r, "flowchart TD\n  A[Старт] --> B", theme).unwrap();
            assert!(svg.contains("<svg"));
            assert!(!svg.contains("<foreignObject"));
        }
    }

    #[test]
    fn reports_syntax_errors() {
        let r = MermanRenderer::new();
        let err = render(&r, "flowchart TD\n  A[open --> B", DiagramTheme::Light).unwrap_err();
        assert!(err.contains("parse error"), "{err}");
        assert!(render(&r, "", DiagramTheme::Light).is_err());
    }

    #[test]
    fn cancelled_control_stops_render() {
        let control = RenderControl::new();
        control.cancel();
        let result = MermanRenderer::new().render_svg(
            "flowchart TD\n  A --> B",
            DiagramTheme::Light,
            &control,
        );
        assert!(result.is_err());
    }

    #[test]
    fn dark_palette_replaces_near_black_colors() {
        let svg = render(
            &MermanRenderer::new(),
            "pie\n  \"A\" : 1\n  \"B\" : 2",
            DiagramTheme::Dark,
        )
        .unwrap();
        assert!(!svg.contains("#0b0000"), "stock dark pie color leaked");
        assert!(svg.contains("#4e79a7"), "custom pie palette not applied");
    }

    #[test]
    fn dark_mindmap_root_is_readable() {
        let svg = render(
            &MermanRenderer::new(),
            "mindmap
  root((Root))
    Child",
            DiagramTheme::Dark,
        )
        .unwrap();
        assert!(
            svg.contains(".section-root rect,#mindmap .section-root path,#mindmap .section-root circle,#mindmap .section-root polygon{fill:#4b6a9b;}"),
            "root fill not overridden"
        );
        assert!(svg.contains(".section-root text{fill:#ffffff;}"));
    }
}
