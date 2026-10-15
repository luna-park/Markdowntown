use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::document::{DiskChange, Document};
use crate::editor::Editor;
use crate::fonts;
use crate::preview::Preview;
use crate::recovery::{Draft, Drafts};
use crate::scroll_sync::ScrollSync;
use crate::settings::{self, Settings, Theme, ViewMode};

const WELCOME: &str = r#"# Markdowntown

A simple **Markdown** editor with a live _preview_.

- Open a file with **Ctrl+O** (or drop it on the window), save with **Ctrl+S**
- Format with **Ctrl+B**, *Ctrl+I*, `Ctrl+E`, links with Ctrl+K
- Find with Ctrl+F, replace with Ctrl+H
- Zoom the whole UI with Ctrl+Plus / Ctrl+Minus
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
/// Как часто проверять, не изменили ли файл другие программы.
const DISK_CHECK_INTERVAL: Duration = Duration::from_secs(1);
/// Как часто (не чаще) записывать черновик несохранённых правок.
const DRAFT_INTERVAL: Duration = Duration::from_secs(5);

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

/// Действие, при котором текущий документ заменяется или закрывается.
#[derive(Debug, Clone)]
enum Action {
    New,
    /// `None` — выбрать файл в диалоге.
    Open(Option<PathBuf>),
    Exit,
}

enum Dialog {
    /// Несохранённые правки перед действием.
    Unsaved(Action),
    /// Файл изменён другой программой, а в редакторе есть свои правки.
    ExternalChange,
    /// Найден черновик после аварийного завершения.
    Recover(Draft),
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

    settings: Settings,
    /// Шрифт редактора, установленный в контекст (сравнивается с настройками).
    installed_editor_font: Option<String>,
    dialog: Option<Dialog>,
    /// Черновики, найденные при старте и ещё не показанные пользователю.
    recovered: Vec<Draft>,
    /// Закрытие окна подтверждено — больше не перехватывать.
    allow_close: bool,

    drafts: Option<Drafts>,
    /// Версия документа и путь, для которых записан последний черновик.
    draft_written: Option<((u64, u64), Option<PathBuf>)>,
    last_draft_time: Instant,
    last_disk_check: Instant,
}

impl MarkdowntownApp {
    pub fn new(cc: &eframe::CreationContext<'_>, data_dir: Option<PathBuf>) -> Self {
        let ctx = &cc.egui_ctx;
        crate::style::apply_global(ctx);

        let settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, settings::STORAGE_KEY))
            .unwrap_or_default();
        ctx.set_zoom_factor(settings.zoom);
        ctx.set_theme(settings.theme.preference());
        fonts::install(ctx, settings.editor_font.as_deref());

        let mut editor = Editor::default();
        editor.font_size = settings.editor_font_size;
        let mut scroll_sync = ScrollSync::default();
        scroll_sync.enabled = settings.sync_scroll;

        let drafts = data_dir.as_deref().map(Drafts::new);
        let recovered = drafts.as_ref().map(Drafts::list).unwrap_or_default();

        let mut app = Self {
            doc: Document::new_untitled(WELCOME),
            editor,
            preview: Preview::default(),
            scroll_sync,
            pending_scroll: (None, None),
            editor_ratio: settings.editor_ratio,
            last_window_width: 0.0,
            status: String::new(),
            installed_editor_font: settings.editor_font.clone(),
            settings,
            dialog: None,
            recovered,
            allow_close: false,
            drafts,
            draft_written: None,
            last_draft_time: Instant::now(),
            last_disk_check: Instant::now(),
        };
        if let Some(path) = std::env::args_os().nth(1) {
            app.load(Path::new(&path));
        }
        if data_dir.is_none() {
            app.set_status("No writable data folder: settings and recovery are disabled".into());
        }
        app
    }

    // ----- Документ и файлы -----

    /// Выполнить действие; если есть несохранённые правки — сначала спросить.
    fn request(&mut self, action: Action) {
        if self.doc.is_dirty() {
            self.dialog = Some(Dialog::Unsaved(action));
        } else {
            self.perform(action);
        }
    }

    fn perform(&mut self, action: Action) {
        match action {
            Action::New => {
                self.discard_draft();
                self.doc = Document::new_untitled("");
                self.set_status("New document".into());
            }
            Action::Open(None) => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Markdown", &["md", "markdown"])
                    .add_filter("All files", &["*"])
                    .pick_file()
                {
                    self.discard_draft();
                    self.load(&path);
                }
            }
            Action::Open(Some(path)) => {
                self.discard_draft();
                self.load(&path);
            }
            Action::Exit => {
                self.discard_draft();
                self.allow_close = true;
            }
        }
    }

    fn load(&mut self, path: &Path) {
        match Document::load(path) {
            Ok(doc) => {
                self.set_status(format!("Opened {}", path.display()));
                self.doc = doc;
                self.settings.add_recent(path);
            }
            Err(e) => {
                self.set_status(format!("Cannot open {}: {e}", path.display()));
                if !path.exists() {
                    self.settings.remove_recent(path);
                }
            }
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
        let status = match (result, self.doc.path()) {
            (Ok(()), Some(path)) => {
                let path = path.to_owned();
                self.settings.add_recent(&path);
                self.discard_draft();
                format!("Saved {}", path.display())
            }
            (Ok(()), None) => "Saved".to_owned(),
            (Err(e), _) => format!("Cannot save: {e}"),
        };
        self.set_status(status);
    }

    /// Сообщение в строке статуса; оно же — запись в журнал (ошибки — предупреждением).
    fn set_status(&mut self, message: String) {
        if message.starts_with("Cannot") {
            log::warn!("{message}");
        } else {
            log::info!("{message}");
        }
        self.status = message;
    }

    // ----- Черновики восстановления -----

    /// Пишет черновик, если есть несохранённые правки, которых ещё нет в черновике.
    fn write_draft(&mut self, ctx: &egui::Context) {
        let Some(drafts) = &self.drafts else {
            return;
        };
        if !self.doc.is_dirty() {
            return;
        }
        let path = self.doc.path().map(Path::to_owned);
        let current = (self.doc.version(), path.clone());
        if self.draft_written.as_ref() == Some(&current) {
            return;
        }
        let elapsed = self.last_draft_time.elapsed();
        if elapsed < DRAFT_INTERVAL {
            // Записать, даже если пользователь перестанет печатать.
            ctx.request_repaint_after(DRAFT_INTERVAL - elapsed);
            return;
        }
        // Сохранили под другим именем — старый черновик больше не нужен.
        if let Some((_, old_path)) = &self.draft_written
            && *old_path != path
        {
            drafts.remove(old_path.as_deref());
        }
        match drafts.save(&Draft::new(path, self.doc.text.clone())) {
            Ok(()) => self.draft_written = Some(current),
            Err(e) => log::warn!("cannot write recovery draft: {e}"),
        }
        self.last_draft_time = Instant::now();
    }

    /// Удаляет черновик текущего документа (сохранён или правки отброшены).
    fn discard_draft(&mut self) {
        if let Some(drafts) = &self.drafts {
            if let Some((_, path)) = self.draft_written.take() {
                drafts.remove(path.as_deref());
            }
            drafts.remove(self.doc.path());
        }
    }

    // ----- Изменения файла на диске -----

    fn check_disk(&mut self, ctx: &egui::Context) {
        ctx.request_repaint_after(DISK_CHECK_INTERVAL);
        if self.dialog.is_some() || self.last_disk_check.elapsed() < DISK_CHECK_INTERVAL {
            return;
        }
        self.last_disk_check = Instant::now();
        match self.doc.disk_change() {
            None => {}
            Some(DiskChange::Modified) if !self.doc.is_dirty() => match self.doc.reload() {
                Ok(()) => self.set_status("Reloaded: the file was changed on disk".into()),
                Err(e) => self.set_status(format!("Cannot reload: {e}")),
            },
            Some(DiskChange::Modified) => self.dialog = Some(Dialog::ExternalChange),
            Some(DiskChange::Removed) => {
                self.doc.accept_disk_state();
                self.set_status("The file was deleted on disk; save to recreate it".into());
            }
        }
    }

    // ----- Диалоги -----

    fn show_dialog(&mut self, ctx: &egui::Context) {
        if self.dialog.is_none()
            && let Some(draft) = self.recovered.pop()
        {
            self.dialog = Some(Dialog::Recover(draft));
        }
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        let name = self.doc.display_name();
        let mut keep_open = true;

        let response = egui::Modal::new(egui::Id::new("dialog")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            match &dialog {
                Dialog::Unsaved(action) => {
                    ui.heading("Unsaved changes");
                    ui.label(format!("Save changes to “{name}” before continuing?"));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            self.save(false);
                            // Сохранение отменили в диалоге выбора файла — остаёмся.
                            if !self.doc.is_dirty() {
                                self.perform(action.clone());
                            }
                            keep_open = false;
                        }
                        if ui.button("Don't Save").clicked() {
                            self.perform(action.clone());
                            keep_open = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep_open = false;
                        }
                    });
                }
                Dialog::ExternalChange => {
                    ui.heading("File changed on disk");
                    ui.label(format!(
                        "“{name}” was changed by another program, and you have unsaved changes."
                    ));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Reload (discard my changes)").clicked() {
                            match self.doc.reload() {
                                Ok(()) => {
                                    self.discard_draft();
                                    self.set_status("Reloaded from disk".into());
                                }
                                Err(e) => self.set_status(format!("Cannot reload: {e}")),
                            }
                            keep_open = false;
                        }
                        if ui.button("Keep my version").clicked() {
                            self.doc.accept_disk_state();
                            keep_open = false;
                        }
                    });
                }
                Dialog::Recover(draft) => {
                    let what = draft.path.as_deref().map_or_else(
                        || "an untitled document".to_owned(),
                        |p| format!("“{}”", p.display()),
                    );
                    ui.heading("Recover unsaved changes?");
                    ui.label(format!(
                        "Markdowntown did not close properly. Unsaved changes to {what} \
                         from {} were found.",
                        draft.age_description()
                    ));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Recover").clicked() {
                            self.doc = Document::recovered(draft.path.clone(), draft.text.clone());
                            // Черновик остаётся до сохранения: следующие записи его обновят.
                            self.draft_written = Some((self.doc.version(), draft.path.clone()));
                            self.set_status("Recovered unsaved changes".into());
                            keep_open = false;
                        }
                        if ui.button("Discard").clicked() {
                            if let Some(drafts) = &self.drafts {
                                drafts.remove(draft.path.as_deref());
                            }
                            keep_open = false;
                        }
                    });
                }
            }
        });
        // Esc / клик мимо окна — как «Cancel» (у восстановления черновик сохраняется).
        let dismissed = response.should_close() && keep_open;
        if keep_open && !dismissed {
            self.dialog = Some(dialog);
        } else if dismissed && let Dialog::ExternalChange = dialog {
            self.doc.accept_disk_state();
        }
    }

    // ----- Окно, меню, ввод -----

    fn window_title(&self) -> String {
        let dirty = if self.doc.is_dirty() { "• " } else { "" };
        format!("{dirty}{} — Markdowntown", self.doc.display_name())
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let new = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);
        let open = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
        let save = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
        let save_as = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::S);

        // save_as проверяем первым: Ctrl+Shift+S иначе съест Ctrl+S
        if ctx.input_mut(|i| i.consume_shortcut(&save_as)) {
            self.save(true);
        } else if ctx.input_mut(|i| i.consume_shortcut(&save)) {
            self.save(false);
        } else if ctx.input_mut(|i| i.consume_shortcut(&open)) {
            self.request(Action::Open(None));
        } else if ctx.input_mut(|i| i.consume_shortcut(&new)) {
            self.request(Action::New);
        }
    }

    /// Закрытие окна (крестик, Alt+F4, File → Exit) с подтверждением.
    fn handle_close(&mut self, ctx: &egui::Context) {
        if self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.doc.is_dirty() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.dialog = Some(Dialog::Unsaved(Action::Exit));
            } else {
                self.discard_draft();
            }
        }
    }

    /// Файл, перетащенный в окно, открывается; пока его несут — подсказка поверх окна.
    fn handle_drop(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_owned()));
        if let Some(path) = dropped {
            self.request(Action::Open(Some(path)));
        }
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("drop_overlay"),
            ));
            let rect = ctx.content_rect();
            painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(160));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Drop to open",
                egui::FontId::proportional(28.0),
                egui::Color32::WHITE,
            );
        }
    }

    fn menu(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New (Ctrl+N)").clicked() {
                    self.request(Action::New);
                }
                if ui.button("Open… (Ctrl+O)").clicked() {
                    self.request(Action::Open(None));
                }
                ui.menu_button("Open Recent", |ui| {
                    if self.settings.recent_files.is_empty() {
                        ui.weak("No recent files");
                    }
                    let mut open = None;
                    for path in &self.settings.recent_files {
                        let label = path.file_name().map_or_else(
                            || path.display().to_string(),
                            |n| n.to_string_lossy().into_owned(),
                        );
                        if ui
                            .button(label)
                            .on_hover_text(path.display().to_string())
                            .clicked()
                        {
                            open = Some(path.clone());
                        }
                    }
                    if let Some(path) = open {
                        self.request(Action::Open(Some(path)));
                    }
                    if !self.settings.recent_files.is_empty() {
                        ui.separator();
                        if ui.button("Clear Recent").clicked() {
                            self.settings.recent_files.clear();
                        }
                    }
                });
                ui.separator();
                if ui.button("Save (Ctrl+S)").clicked() {
                    self.save(false);
                }
                if ui.button("Save As… (Ctrl+Shift+S)").clicked() {
                    self.save(true);
                }
                ui.separator();
                if ui.button("Exit").clicked() {
                    self.request(Action::Exit);
                }
            });
            ui.menu_button("View", |ui| self.view_menu(ui));
        });
    }

    fn view_menu(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.radio_value(&mut s.view_mode, ViewMode::Both, "Editor and preview");
        ui.radio_value(&mut s.view_mode, ViewMode::Editor, "Editor only");
        ui.radio_value(&mut s.view_mode, ViewMode::Preview, "Preview only");
        ui.separator();
        ui.checkbox(&mut self.scroll_sync.enabled, "Synchronized scrolling");
        ui.separator();

        ui.menu_button("Theme", |ui| {
            let before = s.theme;
            ui.radio_value(&mut s.theme, Theme::System, "System");
            ui.radio_value(&mut s.theme, Theme::Light, "Light");
            ui.radio_value(&mut s.theme, Theme::Dark, "Dark");
            if s.theme != before {
                ui.ctx().set_theme(s.theme.preference());
            }
        });

        // Подменю, а не ComboBox: выпадающий список внутри всплывающего меню egui
        // обрезается и показывает только первый пункт.
        ui.menu_button("Editor font", |ui| {
            ui.radio_value(&mut s.editor_font, None, fonts::BUILTIN_EDITOR_FONT);
            for font in fonts::available_editor_fonts() {
                ui.radio_value(&mut s.editor_font, Some(font.to_owned()), font);
            }
        });
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
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(&self.status);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let format = self.doc.format();
                ui.label(format.line_ending.label());
                ui.separator();
                ui.label(if format.bom { "UTF-8 BOM" } else { "UTF-8" });
            });
        });
    }

    fn panels(&mut self, ui: &mut egui::Ui) {
        let (editor_scroll, preview_scroll) = std::mem::take(&mut self.pending_scroll);
        match self.settings.view_mode {
            ViewMode::Editor => {
                egui::CentralPanel::default()
                    .show(ui, |ui| self.editor.show(ui, &mut self.doc, None));
                return;
            }
            ViewMode::Preview => {
                egui::CentralPanel::default()
                    .show(ui, |ui| self.preview.show(ui, &mut self.doc, None));
                return;
            }
            ViewMode::Both => {}
        }

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
            ui.ctx().request_repaint();
        }
    }

    /// Переносит текущее состояние в `settings` перед сохранением.
    fn sync_settings(&mut self, ctx: &egui::Context) {
        let s = &mut self.settings;
        s.editor_font_size = self.editor.font_size;
        s.zoom = ctx.zoom_factor();
        s.sync_scroll = self.scroll_sync.enabled;
        s.editor_ratio = self.editor_ratio;
    }
}

impl eframe::App for MarkdowntownApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.dialog.is_none() {
            self.handle_shortcuts(&ctx);
        }
        self.handle_drop(&ctx);

        if self.installed_editor_font != self.settings.editor_font {
            fonts::install(&ctx, self.settings.editor_font.as_deref());
            self.installed_editor_font = self.settings.editor_font.clone();
        }

        egui::Panel::top("menu").show(ui, |ui| self.menu(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        self.panels(ui);
        self.show_dialog(&ctx);

        self.check_disk(&ctx);
        self.write_draft(&ctx);
        self.sync_settings(&ctx);
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.window_title()));
        // В конце кадра: Exit из меню или диалога закрывает окно в этом же кадре.
        self.handle_close(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, settings::STORAGE_KEY, &self.settings);
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
    /// Проверяются встроенные шрифты: системные запасные (`fonts.rs`) могут
    /// отсутствовать.
    const UI_SYMBOLS: &[char] = &['⏶', '⏷', '×', '…', '−', '“', '”'];

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
