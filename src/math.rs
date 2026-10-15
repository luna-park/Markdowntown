//! Формулы LaTeX (`$…$`, `$$…$$`) → SVG через `ratex` (порт KaTeX). Глифы — контурами,
//! шрифты KaTeX вшиты (`ratex-svg/embed-fonts`); те же крейты использует `merman` для
//! формул в диаграммах, поэтому к размеру exe это почти ничего не добавляет.

use std::collections::HashMap;

use ratex_types::{Color, MathStyle};

use crate::diagram::Rasterizer;

/// Формула в SVG. Размеры — в единицах SVG (`font_size` единиц на em).
pub struct Formula {
    pub svg: String,
    pub width: f32,
    pub height: f32,
    /// Часть высоты ниже базовой линии.
    pub depth: f32,
}

/// `display` — выключная формула (`$$…$$`): крупнее пределы, дроби и т.п.
pub fn render(tex: &str, display: bool, font_size: f32, color: [u8; 3]) -> Result<Formula, String> {
    let ast = ratex_parser::parse(tex).map_err(|e| e.to_string())?;
    let style = if display {
        MathStyle::Display
    } else {
        MathStyle::Text
    };
    let [r, g, b] = color.map(|c| f32::from(c) / 255.0);
    let options = ratex_layout::LayoutOptions::default()
        .with_style(style)
        .with_color(Color::rgb(r, g, b));
    let list = ratex_layout::to_display_list(&ratex_layout::layout(&ast, &options));
    let svg = ratex_svg::render_to_svg(
        &list,
        &ratex_svg::SvgOptions {
            font_size: f64::from(font_size),
            padding: 0.0,
            stroke_width: 0.04 * f64::from(font_size),
            embed_glyphs: true,
            font_dir: String::new(),
        },
    );
    let em = |v: f64| v.max(0.0) as f32 * font_size;
    Ok(Formula {
        // ratex пишет размеры в pt — resvg увеличил бы их в 4/3 раза.
        svg: crate::export::with_intrinsic_size(&svg),
        width: em(list.width),
        height: em(list.total_height()),
        depth: em(list.depth),
    })
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    tex: String,
    display: bool,
    color: [u8; 3],
    /// `f32::to_bits` размера шрифта и `pixels_per_point`.
    size: u32,
    scale: u32,
}

/// Готовая формула: текстура, размер в точках, часть высоты ниже базовой линии.
struct Image {
    texture: egui::TextureHandle,
    size: egui::Vec2,
    depth: f32,
}

struct Entry {
    image: Result<Image, String>,
    used: u64,
}

/// Формулы крупнее окружающего текста, как в KaTeX (`.katex { font-size: 1.21em }`).
pub const MATH_SCALE: f32 = 1.21;

/// Кэш текстур формул для превью. Формулы рисуются сразу (миллисекунды на формулу),
/// а невидимые блоки превью не рисуются — считаются только формулы на экране.
#[derive(Default)]
pub struct MathCache {
    entries: HashMap<Key, Entry>,
    rasterizer: Option<Rasterizer>,
    frame: u64,
}

/// Сколько записей держать, прежде чем выбрасывать давно не нужные.
const MAX_ENTRIES: usize = 512;
/// «Давно» — столько кадров без показа.
const STALE_FRAMES: u64 = 600;

impl MathCache {
    /// Вызывать раз в кадр до рисования.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        if self.entries.len() > MAX_ENTRIES {
            let frame = self.frame;
            self.entries.retain(|_, e| frame - e.used < STALE_FRAMES);
        }
    }

    /// Рисует формулу (для `render_math_fn` в `egui_commonmark`).
    pub fn show(&mut self, ui: &mut egui::Ui, tex: &str, inline: bool) {
        let font_size = egui::TextStyle::Body.resolve(ui.style()).size * MATH_SCALE;
        let [r, g, b, _] = ui.visuals().text_color().to_array();
        let key = Key {
            tex: tex.to_owned(),
            display: !inline,
            color: [r, g, b],
            size: font_size.to_bits(),
            scale: ui.ctx().pixels_per_point().to_bits(),
        };
        let frame = self.frame;
        let rasterizer = self
            .rasterizer
            .get_or_insert_with(Rasterizer::without_fonts);
        let entry = self.entries.entry(key).or_insert_with_key(|key| Entry {
            image: rasterize(ui.ctx(), rasterizer, key, font_size),
            used: frame,
        });
        entry.used = frame;

        match &entry.image {
            Ok(image) => {
                let texture = &image.texture;
                let mut size = image.size;
                if inline {
                    show_inline(ui, texture.id(), size, image.depth);
                } else {
                    // Выключная формула — отдельной строкой по центру; шире панели — уменьшить.
                    ui.end_row();
                    let width = ui.available_width();
                    if size.x > width {
                        size *= width / size.x;
                    }
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, size.y),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| ui.add(egui::Image::new((texture.id(), size))),
                    );
                    ui.end_row();
                }
            }
            Err(error) => {
                let source = if inline {
                    format!("${tex}$")
                } else {
                    format!("$${tex}$$")
                };
                let color = ui.visuals().error_fg_color;
                ui.colored_label(color, source)
                    .on_hover_text(format!("Formula error: {error}"));
            }
        }
    }
}

/// Строчная формула: базовая линия формулы — на базовой линии текста.
///
/// Строка абзаца (`horizontal_wrapped`) центрирует элементы по вертикали, поэтому
/// выделяем прямоугольник, симметричный относительно центра строки, а картинку рисуем
/// в нём со сдвигом. Высокая формула (дробь) раздвигает строку, как в браузере.
fn show_inline(ui: &mut egui::Ui, texture: egui::TextureId, size: egui::Vec2, depth: f32) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let row_height = ui.text_style_height(&egui::TextStyle::Body);
    let baseline = ui
        .fonts_mut(|f| f.layout_no_wrap("x".into(), font, egui::Color32::WHITE))
        .rows
        .first()
        .and_then(|row| row.glyphs.first())
        .map_or(row_height * 0.8, |glyph| glyph.pos.y);
    // Верх картинки относительно центра строки.
    let top = baseline - row_height / 2.0 - (size.y - depth);
    let half = (-top).max(top + size.y).max(row_height / 2.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size.x, 2.0 * half), egui::Sense::hover());
    let image_rect =
        egui::Rect::from_min_size(egui::pos2(rect.left(), rect.center().y + top), size);
    egui::Image::new((texture, size)).paint_at(ui, image_rect);
}

fn rasterize(
    ctx: &egui::Context,
    rasterizer: &Rasterizer,
    key: &Key,
    font_size: f32,
) -> Result<Image, String> {
    let formula = render(&key.tex, key.display, font_size, key.color)?;
    let raster = rasterizer.rasterize(&formula.svg, f32::from_bits(key.scale))?;
    let texture = ctx.load_texture(
        format!("math:{}", key.tex),
        raster.image,
        egui::TextureOptions::LINEAR,
    );
    Ok(Image {
        texture,
        size: raster.size,
        depth: formula.depth,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_with_size_from_font() {
        let f = render(r"\frac{a}{b} + x^2", false, 10.0, [0, 0, 0]).unwrap();
        assert!(f.svg.starts_with("<svg"));
        assert!(!f.svg.contains("pt\""), "sizes must be plain numbers");
        assert!(f.width > 10.0 && f.height > 10.0 && f.depth > 0.0);
        let big = render(r"\frac{a}{b} + x^2", false, 20.0, [0, 0, 0]).unwrap();
        assert!((big.width - 2.0 * f.width).abs() < 0.01);
    }

    #[test]
    fn display_style_is_larger() {
        let tex = r"\sum_{i=1}^n i";
        let inline = render(tex, false, 10.0, [0, 0, 0]).unwrap();
        let display = render(tex, true, 10.0, [0, 0, 0]).unwrap();
        assert!(display.height > inline.height);
    }

    #[test]
    fn color_is_applied_and_errors_reported() {
        let f = render("x", false, 10.0, [255, 0, 0]).unwrap();
        assert!(f.svg.contains("rgba(255,0,0,1)"), "{}", f.svg);
        assert!(render(r"\frac{1}{", false, 10.0, [0, 0, 0]).is_err());
    }

    #[test]
    fn rasterizes() {
        let f = render(r"\sqrt{2}", false, 14.0, [0, 0, 0]).unwrap();
        let r = Rasterizer::without_fonts().rasterize(&f.svg, 2.0).unwrap();
        assert!(
            (r.size.x - f.width).abs() < 1.0,
            "{} vs {}",
            r.size.x,
            f.width
        );
        assert!(r.image.pixels.iter().any(|p| p.a() > 0), "formula is blank");
    }
}
