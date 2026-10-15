use std::path::Path;

use crate::document::Document;
use crate::editor::Editor;
use crate::preview::Preview;
use crate::scroll_sync::ScrollSync;

const WELCOME: &str = r#"# Markdowntown

A simple **Markdown** editor with a live _preview_.

- Open a file with **Ctrl+O**, save with **Ctrl+S**
- Format with **Ctrl+B**, *Ctrl+I*, `Ctrl+E`, links with Ctrl+K
- Find with Ctrl+F, replace with Ctrl+H
- [x] GitHub-flavored Markdown
- [x] Mermaid diagrams

```mermaid
graph TD
    A[Editor] --> B[Preview]
```
"#;

/// Минимальная ширина панели редактора, px.
const MIN_EDITOR_WIDTH: f32 = 200.0;
/// Минимальная ширина превью, px: его нельзя сжать разделителем до нуля.
const MIN_PREVIEW_WIDTH: f32 = 280.0;

/// Допустимая ширина панели редактора при ширине окна `window_width`: превью остаётся
/// не уже `MIN_PREVIEW_WIDTH`. В очень узком окне обе панели делят его поровну.
fn editor_width_range(window_width: f32) -> egui::Rangef {
    let max = window_width - MIN_PREVIEW_WIDTH;
    if max >= MIN_EDITOR_WIDTH {
        egui::Rangef::new(MIN_EDITOR_WIDTH, max)
    } else {
        egui::Rangef::point((window_width / 2.0).max(0.0))
    }
}

pub struct MarkdowntownApp {
    doc: Document,
    editor: Editor,
    preview: Preview,
    scroll_sync: ScrollSync,
    /// Прокрутка панелей от синхронизации, вычисленная в прошлом кадре.
    pending_scroll: (Option<f32>, Option<f32>),
    /// Доля ширины окна под редактор: сохраняется при изменении размера окна,
    /// обновляется, когда пользователь двигает разделитель.
    editor_ratio: f32,
    /// Ширина окна в прошлом кадре — чтобы заметить изменение размера.
    last_window_width: f32,
    status: String,
}

impl MarkdowntownApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::style::apply_global(&cc.egui_ctx);
        let mut app = Self {
            doc: Document::new_untitled(WELCOME),
            editor: Editor::default(),
            preview: Preview::default(),
            scroll_sync: ScrollSync::default(),
            pending_scroll: (None, None),
            editor_ratio: 0.5,
            last_window_width: 0.0,
            status: String::new(),
        };
        if let Some(path) = std::env::args_os().nth(1) {
            app.load(Path::new(&path));
        }
        app
    }

    fn open_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Markdown", &["md", "markdown"])
            .add_filter("All files", &["*"])
            .pick_file()
        {
            self.load(&path);
        }
    }

    fn load(&mut self, path: &Path) {
        match Document::load(path) {
            Ok(doc) => {
                self.status = format!("Opened {}", path.display());
                self.doc = doc;
            }
            Err(e) => self.status = format!("Cannot open {}: {e}", path.display()),
        }
    }

    fn save(&mut self, save_as: bool) {
        let result = match (self.doc.path(), save_as) {
            (Some(_), false) => self.doc.save(),
            _ => {
                let Some(path) = rfd::FileDialog::new()
                    .add_filter("Markdown", &["md"])
                    .set_file_name(self.doc.display_name())
                    .save_file()
                else {
                    return;
                };
                self.doc.save_as(&path)
            }
        };
        self.status = match (result, self.doc.path()) {
            (Ok(()), Some(path)) => format!("Saved {}", path.display()),
            (Ok(()), None) => "Saved".to_owned(),
            (Err(e), _) => format!("Cannot save: {e}"),
        };
    }

    fn window_title(&self) -> String {
        let dirty = if self.doc.is_dirty() { "• " } else { "" };
        format!("{dirty}{} — Markdowntown", self.doc.display_name())
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let open = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
        let save = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
        let save_as = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::S);

        // save_as проверяем первым: Ctrl+Shift+S иначе съест Ctrl+S
        if ctx.input_mut(|i| i.consume_shortcut(&save_as)) {
            self.save(true);
        } else if ctx.input_mut(|i| i.consume_shortcut(&save)) {
            self.save(false);
        } else if ctx.input_mut(|i| i.consume_shortcut(&open)) {
            self.open_dialog();
        }
    }
}

impl eframe::App for MarkdowntownApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_shortcuts(&ctx);

        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open… (Ctrl+O)").clicked() {
                        self.open_dialog();
                    }
                    if ui.button("Save (Ctrl+S)").clicked() {
                        self.save(false);
                    }
                    if ui.button("Save as… (Ctrl+Shift+S)").clicked() {
                        self.save(true);
                    }
                });
                ui.menu_button("View", |ui| {
                    ui.checkbox(&mut self.scroll_sync.enabled, "Synchronized scrolling");
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Editor font size");
                        ui.add(
                            egui::DragValue::new(&mut self.editor.font_size)
                                .range(crate::style::EDITOR_FONT_SIZE_RANGE)
                                .speed(0.2)
                                .suffix(" pt"),
                        );
                    });
                    ui.weak("Whole UI: Ctrl+Plus / Ctrl+Minus / Ctrl+0");
                });
            });
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let format = self.doc.format();
                    ui.label(format.line_ending.label());
                    ui.separator();
                    ui.label(if format.bom { "UTF-8 BOM" } else { "UTF-8" });
                });
            });
        });

        let (editor_scroll, preview_scroll) = std::mem::take(&mut self.pending_scroll);
        let window_width = ui.available_width();
        let editor_width_range = editor_width_range(window_width);
        let mut editor_panel = egui::Panel::left("editor")
            .resizable(true)
            .size_range(editor_width_range);
        if (window_width - self.last_window_width).abs() > 0.5 {
            // Размер окна изменился: сохранить пропорцию, а не ширину в пикселях.
            let width = (window_width * self.editor_ratio)
                .clamp(editor_width_range.min, editor_width_range.max);
            editor_panel = editor_panel.exact_size(width);
            self.last_window_width = window_width;
        }
        let editor = editor_panel.show(ui, |ui| self.editor.show(ui, &mut self.doc, editor_scroll));
        if window_width > 0.0 {
            self.editor_ratio = (editor.response.rect.width() / window_width).clamp(0.1, 0.9);
        }
        let editor = editor.inner;
        let preview = egui::CentralPanel::default()
            .show(ui, |ui| {
                self.preview.show(ui, &mut self.doc, preview_scroll)
            })
            .inner;

        self.pending_scroll = self.scroll_sync.update(&self.doc.text, &editor, &preview);
        if self.pending_scroll != (None, None) {
            ctx.request_repaint();
        }

        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.window_title()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_keeps_minimum_width() {
        let r = editor_width_range(1200.0);
        assert_eq!(
            (r.min, r.max),
            (MIN_EDITOR_WIDTH, 1200.0 - MIN_PREVIEW_WIDTH)
        );
        // Окно уже суммы минимумов — поровну, без противоречивого диапазона.
        let r = editor_width_range(400.0);
        assert_eq!((r.min, r.max), (200.0, 200.0));
    }

    /// Все не-ASCII символы, которые выводятся в интерфейсе (кнопки, подписи).
    /// Во встроенных шрифтах egui многих символов нет (например, ↑ ↓ ▲ ✓ →) —
    /// они рисуются квадратиками. Добавляя новый символ в UI, допишите его сюда.
    const UI_SYMBOLS: &[char] = &['⏶', '⏷', '×', '…', '−'];

    #[test]
    fn ui_symbols_exist_in_default_fonts() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |_| {});
        output.textures_delta.clear();
        let font = egui::FontId::proportional(14.0);
        let missing: Vec<char> = UI_SYMBOLS
            .iter()
            .copied()
            .filter(|&c| !ctx.fonts_mut(|f| f.has_glyph(&font, c)))
            .collect();
        assert!(missing.is_empty(), "no glyphs for {missing:?}");
    }
}
