use std::path::PathBuf;

use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

const SAMPLE: &str = r#"# mdview

Простой редактор **Markdown** с _превью_.

- списки
- `inline code`
- [ссылки](https://commonmark.org)

```rust
fn main() {
    println!("Hello");
}
```

| Колонка | Значение |
|---------|----------|
| a       | 1        |

```mermaid
graph TD
    A[Редактор] --> B[Превью]
```
"#;

pub struct MdViewApp {
    text: String,
    path: Option<PathBuf>,
    dirty: bool,
    md_cache: CommonMarkCache,
    status: String,
}

impl MdViewApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            text: SAMPLE.to_owned(),
            path: None,
            dirty: false,
            md_cache: CommonMarkCache::default(),
            status: String::new(),
        }
    }

    fn open(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Markdown", &["md", "markdown"])
            .pick_file()
        else {
            return;
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                self.text = text;
                self.status = format!("Открыт {}", path.display());
                self.path = Some(path);
                self.dirty = false;
            }
            Err(e) => self.status = format!("Ошибка чтения: {e}"),
        }
    }

    fn save(&mut self, save_as: bool) {
        let path = match (&self.path, save_as) {
            (Some(p), false) => p.clone(),
            _ => match rfd::FileDialog::new()
                .add_filter("Markdown", &["md"])
                .set_file_name("untitled.md")
                .save_file()
            {
                Some(p) => p,
                None => return,
            },
        };
        match std::fs::write(&path, &self.text) {
            Ok(()) => {
                self.status = format!("Сохранён {}", path.display());
                self.path = Some(path);
                self.dirty = false;
            }
            Err(e) => self.status = format!("Ошибка записи: {e}"),
        }
    }

    fn window_title(&self) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".to_owned());
        format!("{}{name} — mdview", if self.dirty { "• " } else { "" })
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
            self.open();
        }
    }
}

impl eframe::App for MdViewApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_shortcuts(&ctx);

        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("Файл", |ui| {
                    if ui.button("Открыть… (Ctrl+O)").clicked() {
                        self.open();
                    }
                    if ui.button("Сохранить (Ctrl+S)").clicked() {
                        self.save(false);
                    }
                    if ui.button("Сохранить как… (Ctrl+Shift+S)").clicked() {
                        self.save(true);
                    }
                });
            });
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.label(&self.status);
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
                            egui::TextEdit::multiline(&mut self.text)
                                .code_editor()
                                .desired_width(f32::INFINITY),
                        );
                        if resp.changed() {
                            self.dirty = true;
                        }
                    });
            });

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("preview_scroll")
                .show(ui, |ui| {
                    CommonMarkViewer::new().show(ui, &mut self.md_cache, &self.text);
                });
        });

        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.window_title()));
    }
}
