//! GFM alerts (`> [!NOTE]` и др.) как на GitHub: значки Octicons (MIT, `licenses/octicons/`),
//! названия и цвета. Общие для превью (`egui_commonmark`) и HTML-экспорта.

use std::sync::{Arc, LazyLock};

use egui::Color32;
use egui_commonmark::{Alert, AlertBundle};

pub struct AlertKind {
    /// Как в разметке: `[!NOTE]`.
    pub id: &'static str,
    /// Заголовок над текстом.
    pub title: &'static str,
    /// Класс в HTML (`markdown-alert-…`) и переменная цвета в CSS экспорта (`--…`).
    pub css: &'static str,
    /// SVG 16×16, один `path` без заливки.
    pub icon: &'static str,
    /// Цвета GitHub — те же, что `--note` и др. в `export.rs` (проверяет тест).
    pub light: Color32,
    pub dark: Color32,
}

pub const ALERTS: [AlertKind; 5] = [
    AlertKind {
        id: "NOTE",
        title: "Note",
        css: "note",
        icon: include_str!("../assets/octicons/info-16.svg"),
        light: Color32::from_rgb(0x09, 0x69, 0xda),
        dark: Color32::from_rgb(0x44, 0x93, 0xf8),
    },
    AlertKind {
        id: "TIP",
        title: "Tip",
        css: "tip",
        icon: include_str!("../assets/octicons/light-bulb-16.svg"),
        light: Color32::from_rgb(0x1a, 0x7f, 0x37),
        dark: Color32::from_rgb(0x3f, 0xb9, 0x50),
    },
    AlertKind {
        id: "IMPORTANT",
        title: "Important",
        css: "important",
        icon: include_str!("../assets/octicons/report-16.svg"),
        light: Color32::from_rgb(0x82, 0x50, 0xdf),
        dark: Color32::from_rgb(0xab, 0x7d, 0xf8),
    },
    AlertKind {
        id: "WARNING",
        title: "Warning",
        css: "warning",
        icon: include_str!("../assets/octicons/alert-16.svg"),
        light: Color32::from_rgb(0x9a, 0x67, 0x00),
        dark: Color32::from_rgb(0xd2, 0x99, 0x22),
    },
    AlertKind {
        id: "CAUTION",
        title: "Caution",
        css: "caution",
        icon: include_str!("../assets/octicons/stop-16.svg"),
        light: Color32::from_rgb(0xd1, 0x24, 0x2f),
        dark: Color32::from_rgb(0xf8, 0x51, 0x49),
    },
];

/// SVG значка с заливкой `fill` (`currentColor` — цветом текста в HTML).
pub fn icon_svg(kind: &AlertKind, fill: &str) -> String {
    kind.icon
        .replacen("<svg", &format!("<svg fill=\"{fill}\""), 1)
}

/// Набор alerts для превью: значки белые — `egui_commonmark` окрашивает их цветом alert.
pub fn preview_bundle(dark: bool) -> AlertBundle {
    static BUNDLES: LazyLock<[AlertBundle; 2]> =
        LazyLock::new(|| [make_bundle(false), make_bundle(true)]);
    BUNDLES[usize::from(dark)].clone()
}

fn make_bundle(dark: bool) -> AlertBundle {
    AlertBundle::from_alerts(
        ALERTS
            .iter()
            .map(|kind| Alert {
                accent_color: if dark { kind.dark } else { kind.light },
                // Запасной символ — если картинка не загрузится.
                icon: '!',
                icon_image: Some(egui::ImageSource::Bytes {
                    // Расширение `.svg` — по нему egui выбирает загрузчик.
                    uri: format!("bytes://octicon-{}.svg", kind.css).into(),
                    bytes: egui::load::Bytes::Shared(Arc::from(
                        icon_svg(kind, "#ffffff").into_bytes(),
                    )),
                }),
                identifier: kind.id.to_owned(),
                identifier_rendered: kind.title.to_owned(),
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_are_plain_single_path_svgs() {
        for kind in &ALERTS {
            assert!(kind.icon.starts_with("<svg"), "{}", kind.id);
            assert!(kind.icon.contains("viewBox=\"0 0 16 16\""), "{}", kind.id);
            assert_eq!(kind.icon.matches("<path").count(), 1, "{}", kind.id);
            assert!(!kind.icon.contains("<script"), "{}", kind.id);
            let filled = icon_svg(kind, "currentColor");
            assert!(filled.starts_with("<svg fill=\"currentColor\""));
        }
    }
}
