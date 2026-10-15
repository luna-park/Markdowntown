use std::path::Path;

use crate::document::Document;
use crate::preview::Preview;

const WELCOME: &str = r#"# mdview

A simple **Markdown** editor with a live _preview_.

- Open a file with **Ctrl+O**, save with **Ctrl+S**
- [x] GitHub-flavored Markdown
- [x] Mermaid diagrams

```mermaid
graph TD
    A[Editor] --> B[Preview]
```
"#;

pub struct MdViewApp {
    doc: Document,
    preview: Preview,
    status: String,
}

impl MdViewApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self {
            doc: Document::new_untitled(WELCOME),
            preview: Preview::default(),
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
        format!("{dirty}{} — mdview", self.doc.display_name())
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

impl eframe::App for MdViewApp {
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

        egui::Panel::left("editor")
            .resizable(true)
            .default_size(ctx.content_rect().width() / 2.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("editor_scroll")
                    .show(ui, |ui| {
                        let resp = ui.add_sized(
                            ui.available_size(),
                            egui::TextEdit::multiline(&mut self.doc.text)
                                .code_editor()
                                .desired_width(f32::INFINITY),
                        );
                        if resp.changed() {
                            self.doc.mark_edited();
                        }
                    });
            });

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("preview_scroll")
                .show(ui, |ui| self.preview.show(ui, &mut self.doc));
        });

        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.window_title()));
    }
}
