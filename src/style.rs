//! Общие настройки внешнего вида: размеры шрифтов интерфейса и цвета текста.

use egui::{Color32, FontId, TextStyle};

/// Размер обычного текста интерфейса (меню, кнопки, подписи), pt. В egui по умолчанию 12.5.
pub const UI_TEXT_SIZE: f32 = 14.0;
/// Размер шрифта редактора по умолчанию, pt.
pub const EDITOR_FONT_SIZE: f32 = 14.0;
pub const EDITOR_FONT_SIZE_RANGE: std::ops::RangeInclusive<f32> = 8.0..=40.0;

/// Увеличенные шрифты интерфейса — для обеих тем. Вызывать один раз при старте.
pub fn apply_global(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(11.0)),
            (TextStyle::Body, FontId::proportional(UI_TEXT_SIZE)),
            (TextStyle::Button, FontId::proportional(UI_TEXT_SIZE)),
            (TextStyle::Heading, FontId::proportional(20.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
    });
}

/// Цвет основного текста для чтения — как у GitHub. В тёмной теме чуть темнее
/// GitHub (`#e6edf3`): в egui нет полужирного шрифта, «жирный» — это белый цвет,
/// и он должен отличаться от обычного текста.
pub fn reading_text_color(dark_mode: bool) -> Color32 {
    if dark_mode {
        Color32::from_rgb(0xc9, 0xd1, 0xd9)
    } else {
        Color32::from_rgb(0x1f, 0x23, 0x28)
    }
}

/// Цвета текста для чтения в `ui` (превью): основной текст светлее стандартного egui,
/// «жирный» — максимально контрастный.
pub fn apply_reading_colors(ui: &mut egui::Ui) {
    let visuals = ui.visuals_mut();
    let dark = visuals.dark_mode;
    visuals.widgets.noninteractive.fg_stroke.color = reading_text_color(dark);
    visuals.widgets.active.fg_stroke.color = if dark { Color32::WHITE } else { Color32::BLACK };
}
