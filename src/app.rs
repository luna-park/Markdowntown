use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use crate::diagram::DiagramService;
use crate::document::Document;
use crate::fonts;
use crate::links::{self, Target};
use crate::recovery::{Draft, Drafts};
use crate::settings::{self, Settings, Theme, ViewMode};
use crate::single_instance::Inbox;
use crate::tab::{DiskEvent, Tab};

const WELCOME: &str = r#"# Markdowntown

A simple **Markdown** editor with a live _preview_.

- Open a file with **Ctrl+O** (or drop it on the window), save with **Ctrl+S**
- Each file opens in its own tab: close with **Ctrl+W**, switch with **Ctrl+Tab**
- Format with **Ctrl+B**, *Ctrl+I*, `Ctrl+E`, links with Ctrl+K
- Find with Ctrl+F, replace with Ctrl+H
- Zoom the whole UI with Ctrl+Plus / Ctrl+Minus or Ctrl+mouse wheel
- [x] GitHub-flavored Markdown
- [x] Mermaid diagrams

```mermaid
graph TD
    A[Editor] --> B[Preview]
```
"#;

/// Название и версия в заголовке окна — по ним проще разбирать сообщения об ошибках.
pub const APP_TITLE: &str = concat!("Markdowntown v", env!("CARGO_PKG_VERSION"));

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

/// Пределы масштаба интерфейса — как у Ctrl+Plus / Ctrl+Minus в egui (`gui_zoom`).
const ZOOM_RANGE: std::ops::RangeInclusive<f32> = 0.2..=5.0;
/// Шаг масштаба на щелчок колеса — как у Ctrl+Plus / Ctrl+Minus.
const ZOOM_STEP: f32 = 0.1;
/// Чувствительность точной прокрутки (тачпад, дельта в точках) — как `scroll_zoom_speed` egui.
const ZOOM_PER_POINT: f32 = 1.0 / 200.0;

/// Ctrl+колесо (и жест «щипок») над основными панелями — масштаб всего интерфейса,
/// как Ctrl+Plus / Ctrl+Minus. Над окнами (просмотр диаграммы, диалоги, меню) — нет:
/// там колесо с Ctrl обрабатывает само окно (диаграмма масштабируется вокруг курсора).
fn handle_wheel_zoom(ctx: &egui::Context) {
    // Масштаб — до `input`: вызов `ctx` внутри замыкания `input` — повторная блокировка
    // контекста (взаимоблокировка).
    let current = ctx.zoom_factor();
    let (zoom, pointer) =
        ctx.input(|i| (wheel_zoom(current, &i.raw.events), i.pointer.hover_pos()));
    let Some(zoom) = zoom else {
        return;
    };
    let over_window = pointer
        .and_then(|p| ctx.layer_id_at(p))
        .is_some_and(|layer| layer.order != egui::Order::Background);
    if !over_window {
        ctx.set_zoom_factor(zoom);
    }
}

/// Новый масштаб по событиям кадра или `None`, если масштаб не меняли.
///
/// По сырым событиям, а не по `zoom_delta`: egui сглаживает колесо и раздаёт щелчок по
/// нескольким кадрам — масштаб менялся бы ступеньками с перераскладкой на каждой.
/// Щелчок колеса (дельта в строках) — ровно шаг `ZOOM_STEP` с округлением, как у
/// Ctrl+Plus; точная прокрутка тачпада (в точках) и «щипок» — плавно.
fn wheel_zoom(zoom: f32, events: &[egui::Event]) -> Option<f32> {
    let mut steps = 0.0;
    let mut factor = 1.0;
    for event in events {
        match event {
            egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
                ..
            } if modifiers.ctrl || modifiers.command => match unit {
                egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => steps += delta.y,
                egui::MouseWheelUnit::Point => factor *= (delta.y * ZOOM_PER_POINT).exp(),
            },
            egui::Event::Zoom(f) => factor *= f,
            _ => {}
        }
    }
    if steps == 0.0 && factor == 1.0 {
        return None;
    }
    let mut zoom = zoom * factor;
    if steps != 0.0 {
        zoom = ((zoom + steps * ZOOM_STEP) / ZOOM_STEP).round() * ZOOM_STEP;
    }
    Some(zoom.clamp(*ZOOM_RANGE.start(), *ZOOM_RANGE.end()))
}

/// Действие с вкладками; закрытие при несохранённых правках сначала спрашивает.
#[derive(Debug, Clone)]
enum Action {
    New,
    /// `None` — выбрать файл в диалоге.
    Open(Option<PathBuf>),
    /// Закрыть вкладки с этими id; про вкладки с правками — по одному вопросу.
    Close(Vec<u64>),
    Exit,
}

enum Dialog {
    /// Несохранённые правки в активной вкладке перед действием.
    Unsaved(Action),
    /// Файл активной вкладки изменён другой программой, а в ней есть свои правки.
    ExternalChange,
    /// Найден черновик после аварийного завершения.
    Recover(Draft),
    /// Help → About: название, версия, автор, ссылка на релизы.
    About,
}

/// Строка не длиннее `max_chars` символов: середина заменяется на «…», чтобы у пути
/// остались видны и начало, и имя файла.
fn elide_middle(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars || max_chars < 3 {
        return text.to_owned();
    }
    let keep = max_chars - 1;
    let head = keep / 2;
    let tail = keep - head;
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(count - tail).collect();
    format!("{start}…{end}")
}

/// Автор — в строке меню и в окне About.
const AUTHOR: &str = "luna-park discoteque";
/// Страница релизов: новые версии скачиваются отсюда.
const RELEASES_URL: &str = "https://github.com/luna-park/Markdowntown/releases";

pub struct MarkdowntownApp {
    /// Всегда хотя бы одна вкладка.
    tabs: Vec<Tab>,
    active: usize,
    next_tab_id: u64,
    /// Общий для всех вкладок: фоновый поток рендера и шрифты.
    diagrams: DiagramService,

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
    /// Идёт выход: по очереди спрашиваем про каждую вкладку с правками.
    exiting: bool,
    /// Вкладки, правки которых при выходе решили не сохранять.
    exit_discarded: Vec<u64>,
    /// Заголовок окна, уже отправленный в окно. Каждая команда окна — это ещё и запрос
    /// перерисовки: отправка заголовка в каждом кадре рисовала кадры без остановки.
    window_title: String,
    /// Вкладка, к которой строка вкладок уже прокручена (см. `tab_bar`).
    tab_bar_revealed: Option<u64>,

    drafts: Option<Drafts>,
    /// Идущий экспорт в HTML: итог (сообщение для статуса) придёт из фонового потока.
    export: Option<mpsc::Receiver<String>>,
    /// Файлы, открытые из Проводника, пока программа уже запущена.
    inbox: Inbox,
    /// Ссылка под курсором в превью (адрес) и куда она ведёт — для строки статуса.
    /// Описание считается один раз на ссылку, а не каждый кадр.
    link_hover: Option<(String, (String, bool))>,
}

impl MarkdowntownApp {
    /// `files` — из командной строки; `inbox` — файлы от следующих запусков программы.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        data_dir: Option<PathBuf>,
        files: &[PathBuf],
        inbox: Inbox,
    ) -> Self {
        let ctx = &cc.egui_ctx;
        inbox.attach(cc);
        // Графика инициализирована — дальше сбои уже не в драйвере при запуске.
        log::info!("graphics initialized, starting UI");
        crate::style::apply_global(ctx);

        let settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, settings::STORAGE_KEY))
            .unwrap_or_default();
        ctx.set_zoom_factor(settings.zoom);
        ctx.set_theme(settings.theme.preference());
        fonts::install(ctx, settings.editor_font.as_deref());

        let drafts = data_dir.as_deref().map(Drafts::new);
        let recovered = drafts.as_ref().map(Drafts::list).unwrap_or_default();

        let mut app = Self {
            tabs: Vec::new(),
            active: 0,
            next_tab_id: 1,
            diagrams: DiagramService::new(ctx),
            editor_ratio: settings.editor_ratio,
            last_window_width: 0.0,
            status: String::new(),
            installed_editor_font: settings.editor_font.clone(),
            settings,
            dialog: None,
            recovered,
            allow_close: false,
            exiting: false,
            exit_discarded: Vec::new(),
            window_title: String::new(),
            tab_bar_revealed: None,
            drafts,
            export: None,
            inbox,
            link_hover: None,
        };
        app.restore_session();
        for path in files {
            app.open_path(path);
        }
        if app.tabs.is_empty() {
            // Первый запуск или пустой сеанс — приветствие.
            app.push_tab(Document::new_untitled(WELCOME));
        }
        if data_dir.is_none() {
            app.set_status("No writable data folder: settings and recovery are disabled".into());
        }
        app
    }

    // ----- Вкладки -----

    fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    fn new_tab_id(&mut self) -> u64 {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        id
    }

    /// Новая вкладка с документом становится активной.
    fn push_tab(&mut self, doc: Document) {
        let id = self.new_tab_id();
        self.tabs.push(Tab::new(id, doc));
        self.activate(self.tabs.len() - 1);
    }

    /// Открыть документ: в активной вкладке, если она пустая безымянная, иначе в новой.
    fn put_document(&mut self, doc: Document) {
        if !self.tabs.is_empty() && self.tab().is_pristine() {
            let id = self.new_tab_id();
            self.tabs[self.active] = Tab::new(id, doc);
        } else {
            self.push_tab(doc);
        }
    }

    fn activate(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
            // Пока вкладка была в фоне, файл могли изменить — проверить сразу.
            self.tabs[index].check_disk_soon();
        }
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// Закрывает вкладку без вопросов (правки сохранены или отброшены).
    fn close_tab(&mut self, id: u64) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let mut tab = self.tabs.remove(index);
        if let Some(drafts) = &self.drafts {
            tab.discard_draft(drafts);
        }
        if self.tabs.is_empty() {
            self.push_tab(Document::new_untitled(""));
        } else if index < self.active || self.active >= self.tabs.len() {
            self.activate(self.active.saturating_sub(1));
        } else {
            self.activate(self.active);
        }
    }

    /// Соседняя вкладка (`step` = 1 / −1), по кругу.
    fn cycle_tab(&mut self, step: isize) {
        let n = self.tabs.len() as isize;
        let next = (self.active as isize + step).rem_euclid(n);
        self.activate(next as usize);
    }

    // ----- Сеанс -----

    /// Открывает файлы прошлого сеанса; отсутствующие пропускаются.
    fn restore_session(&mut self) {
        let session = self.settings.session.clone();
        let mut active = None;
        for (i, path) in session.iter().enumerate() {
            match Document::load(path) {
                Ok(doc) => {
                    self.push_tab(doc);
                    if i == self.settings.session_active {
                        active = Some(self.tabs.len() - 1);
                    }
                }
                Err(e) => log::info!("session: skipping {}: {e}", path.display()),
            }
        }
        if let Some(index) = active {
            self.activate(index);
        } else if !self.tabs.is_empty() {
            self.activate(0);
        }
        if !self.tabs.is_empty() {
            log::info!("session: restored {} tab(s)", self.tabs.len());
        }
    }

    /// Файлы открытых вкладок — в настройки (сохраняются eframe периодически и при выходе).
    fn save_session(&mut self) {
        let mut session = Vec::new();
        let mut active = 0;
        for (i, tab) in self.tabs.iter().enumerate() {
            if let Some(path) = tab.doc.path() {
                if i == self.active {
                    active = session.len();
                }
                session.push(path.to_owned());
            }
        }
        self.settings.session = session;
        self.settings.session_active = active;
    }

    // ----- Документ и файлы -----

    fn request(&mut self, action: Action) {
        match action {
            Action::New => {
                self.push_tab(Document::new_untitled(""));
                self.set_status("New document".into());
            }
            Action::Open(None) => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Markdown", &["md", "markdown"])
                    .add_filter("All files", &["*"])
                    .pick_file()
                {
                    self.open_path(&path);
                }
            }
            Action::Open(Some(path)) => self.open_path(&path),
            Action::Close(mut ids) => {
                // Без правок — закрыть сразу; с правками — спросить про первую,
                // остальные ждут ответа (Cancel оставляет их открытыми).
                ids.retain(|&id| self.index_of(id).is_some());
                let (dirty, clean): (Vec<u64>, Vec<u64>) = ids.into_iter().partition(|&id| {
                    self.index_of(id)
                        .is_some_and(|i| self.tabs[i].doc.is_dirty())
                });
                for id in clean {
                    self.close_tab(id);
                }
                if let Some(index) = dirty.first().and_then(|&id| self.index_of(id)) {
                    self.activate(index);
                    self.dialog = Some(Dialog::Unsaved(Action::Close(dirty)));
                }
            }
            Action::Exit => self.continue_exit(),
        }
    }

    /// Выход: спросить про следующую вкладку с правками или закрыть окно.
    fn continue_exit(&mut self) {
        self.exiting = true;
        let next = self
            .tabs
            .iter()
            .position(|t| t.doc.is_dirty() && !self.exit_discarded.contains(&t.id));
        if let Some(index) = next {
            self.activate(index);
            self.dialog = Some(Dialog::Unsaved(Action::Exit));
            return;
        }
        if let Some(drafts) = &self.drafts {
            for tab in &mut self.tabs {
                tab.discard_draft(drafts);
            }
        }
        self.allow_close = true;
    }

    fn cancel_exit(&mut self) {
        self.exiting = false;
        self.exit_discarded.clear();
    }

    /// Вопрос о несохранённых правках решён: `saved` — сохранили, иначе отбросили.
    fn resolve_unsaved(&mut self, action: Action, saved: bool) {
        match action {
            Action::Close(ids) => {
                // Первая в списке — та, о которой спрашивали; дальше — остальные.
                if let Some((&id, rest)) = ids.split_first() {
                    self.close_tab(id);
                    self.request(Action::Close(rest.to_vec()));
                }
            }
            Action::Exit => {
                if !saved {
                    let id = self.tab().id;
                    if let Some(drafts) = &self.drafts {
                        self.tabs[self.active].discard_draft(drafts);
                    }
                    self.exit_discarded.push(id);
                }
                self.continue_exit();
            }
            other => self.request(other),
        }
    }

    /// Открывает файл; если он уже открыт — переключается на его вкладку.
    fn open_path(&mut self, path: &Path) {
        if let Some(index) = self.tabs.iter().position(|t| t.has_path(path)) {
            self.activate(index);
            self.settings.add_recent(path);
            return;
        }
        match Document::load(path) {
            Ok(doc) => {
                self.set_status(format!("Opened {}", path.display()));
                self.put_document(doc);
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
        let doc = &mut self.tabs[self.active].doc;
        let result = match (doc.path(), save_as) {
            (Some(_), false) => doc.save(),
            _ => {
                let Some(path) = rfd::FileDialog::new()
                    .add_filter("Markdown", &["md"])
                    .set_file_name(doc.display_name())
                    .save_file()
                else {
                    return;
                };
                // Тот же файл уже открыт в другой вкладке — две вкладки не должны
                // писать в один файл.
                if self
                    .tabs
                    .iter()
                    .enumerate()
                    .any(|(i, t)| i != self.active && t.has_path(&path))
                {
                    self.set_status(format!(
                        "Cannot save: {} is open in another tab",
                        path.display()
                    ));
                    return;
                }
                self.tabs[self.active].doc.save_as(&path)
            }
        };
        let status = match (result, self.tab().doc.path()) {
            (Ok(()), Some(path)) => {
                let path = path.to_owned();
                self.settings.add_recent(&path);
                if let Some(drafts) = &self.drafts {
                    self.tabs[self.active].discard_draft(drafts);
                }
                format!("Saved {}", path.display())
            }
            (Ok(()), None) => "Saved".to_owned(),
            (Err(e), _) => format!("Cannot save: {e}"),
        };
        self.set_status(status);
    }

    /// Экспорт в HTML: выбрать файл и собрать страницу в фоновом потоке
    /// (каждая диаграмма рендерится дважды — для светлой и тёмной темы).
    fn export_html(&mut self) {
        if self.export.is_some() {
            return;
        }
        let doc = &self.tab().doc;
        let stem = Path::new(&doc.display_name()).file_stem().map_or_else(
            || "document".to_owned(),
            |s| s.to_string_lossy().into_owned(),
        );
        let mut dialog = rfd::FileDialog::new()
            .add_filter("HTML", &["html", "htm"])
            .set_file_name(format!("{stem}.html"));
        if let Some(dir) = doc.base_dir() {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.save_file() else {
            return;
        };

        let theme = match self.settings.theme {
            Theme::System => crate::export::PageTheme::Auto,
            Theme::Light => crate::export::PageTheme::Light,
            Theme::Dark => crate::export::PageTheme::Dark,
        };
        let text = doc.text.clone();
        let base_dir = doc.base_dir().map(Path::to_owned);
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("html-export".into())
            .spawn(move || {
                let renderer = crate::diagram::MermanRenderer::new();
                let out =
                    crate::export::to_html(&text, base_dir.as_deref(), &stem, theme, &renderer);
                let message = match crate::document::write_atomic(&path, out.html.as_bytes()) {
                    Ok(()) => match out.warnings() {
                        Some(w) => format!("Exported to {} ({w})", path.display()),
                        None => format!("Exported to {}", path.display()),
                    },
                    Err(e) => format!("Cannot export to {}: {e}", path.display()),
                };
                let _ = tx.send(message);
            });
        match spawned {
            Ok(_) => {
                self.export = Some(rx);
                self.set_status("Exporting to HTML…".into());
            }
            Err(e) => self.set_status(format!("Cannot export: {e}")),
        }
    }

    /// Проверяет, не закончился ли экспорт.
    fn poll_export(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.export else {
            return;
        };
        match rx.try_recv() {
            Ok(message) => {
                self.export = None;
                self.set_status(message);
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
            Err(mpsc::TryRecvError::Disconnected) => {
                // Поток завершился без ответа — паника, она уже в журнале.
                self.export = None;
                self.set_status("Cannot export: internal error (see log)".into());
            }
        }
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

    // ----- Черновики и диск -----

    /// Черновики несохранённых правок всех вкладок.
    fn write_drafts(&mut self, ctx: &egui::Context) {
        // Выход подтверждён (правки сохранены или отброшены, черновики удалены):
        // не записывать их заново в последнем кадре — иначе при следующем запуске
        // прошлое закрытие выглядело бы аварийным.
        let Some(drafts) = &self.drafts else {
            return;
        };
        if self.allow_close {
            return;
        }
        for tab in &mut self.tabs {
            if !self.exit_discarded.contains(&tab.id) {
                tab.write_draft(drafts, ctx);
            }
        }
    }

    /// Файл активной вкладки на диске (фоновые проверяются при переключении на них).
    fn check_disk(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() {
            return;
        }
        match self.tab_mut().check_disk(ctx) {
            None => {}
            Some(DiskEvent::Status(message)) => self.set_status(message),
            Some(DiskEvent::Conflict) => self.dialog = Some(Dialog::ExternalChange),
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
        let name = self.tab().doc.display_name();
        let mut keep_open = true;

        let response = egui::Modal::new(egui::Id::new("dialog")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            match &dialog {
                Dialog::Unsaved(action) => {
                    ui.heading("Unsaved changes");
                    ui.label(format!("Save changes to “{name}” before closing?"));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            self.save(false);
                            // Сохранение отменили в диалоге выбора файла — остаёмся.
                            if !self.tab().doc.is_dirty() {
                                self.resolve_unsaved(action.clone(), true);
                            } else if matches!(action, Action::Exit) {
                                self.cancel_exit();
                            }
                            keep_open = false;
                        }
                        if ui.button("Don't Save").clicked() {
                            self.resolve_unsaved(action.clone(), false);
                            keep_open = false;
                        }
                        if ui.button("Cancel").clicked() {
                            if matches!(action, Action::Exit) {
                                self.cancel_exit();
                            }
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
                            match self.tab_mut().doc.reload() {
                                Ok(()) => {
                                    if let Some(drafts) = &self.drafts {
                                        self.tabs[self.active].discard_draft(drafts);
                                    }
                                    self.set_status("Reloaded from disk".into());
                                }
                                Err(e) => self.set_status(format!("Cannot reload: {e}")),
                            }
                            keep_open = false;
                        }
                        if ui.button("Keep my version").clicked() {
                            self.tab_mut().doc.accept_disk_state();
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
                            self.recover(draft);
                            keep_open = false;
                        }
                        if ui.button("Discard").clicked() {
                            if let Some(drafts) = &self.drafts {
                                drafts.remove(draft.path.as_deref(), draft.untitled);
                            }
                            keep_open = false;
                        }
                    });
                }
                Dialog::About => {
                    ui.vertical_centered(|ui| {
                        ui.add(
                            egui::Image::new(egui::include_image!("../assets/icon-256.png"))
                                .fit_to_exact_size(egui::vec2(64.0, 64.0)),
                        );
                        ui.heading("Markdowntown");
                        ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                        ui.add_space(4.0);
                        ui.label(env!("CARGO_PKG_DESCRIPTION"));
                        ui.add_space(8.0);
                        ui.label(format!("© 2026 {AUTHOR}"));
                        ui.weak(format!("License: {}", env!("CARGO_PKG_LICENSE")));
                        ui.add_space(8.0);
                        ui.label("New versions and release notes:");
                        ui.hyperlink(RELEASES_URL);
                        ui.add_space(8.0);
                        if ui.button("OK").clicked() {
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
        } else if dismissed {
            match dialog {
                Dialog::ExternalChange => self.tab_mut().doc.accept_disk_state(),
                Dialog::Unsaved(Action::Exit) => self.cancel_exit(),
                _ => {}
            }
        }
    }

    /// Восстановить черновик: во вкладку этого файла (если открыт сеансом) или в новую.
    fn recover(&mut self, draft: &Draft) {
        let doc = Document::recovered(draft.path.clone(), draft.text.clone());
        let existing = draft
            .path
            .as_deref()
            .and_then(|p| self.tabs.iter().position(|t| t.has_path(p)));
        match existing {
            Some(index) => {
                self.tabs[index].doc = doc;
                self.activate(index);
            }
            None => self.put_document(doc),
        }
        let tab = self.tab_mut();
        // Черновик остаётся до сохранения: следующие записи его обновят.
        tab.untitled_key = draft.untitled;
        tab.draft_written = Some((tab.doc.version(), draft.path.clone()));
        self.set_status("Recovered unsaved changes".into());
    }

    // ----- Окно, меню, ввод -----

    fn window_title(&self) -> String {
        let doc = &self.tab().doc;
        let dirty = if doc.is_dirty() { "• " } else { "" };
        format!("{dirty}{} — {APP_TITLE}", doc.display_name())
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let new = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);
        let open = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
        let save = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
        // F2 — сохранить, как в классических редакторах (Far, Norton).
        let save_f2 = KeyboardShortcut::new(Modifiers::NONE, Key::F2);
        let save_as = KeyboardShortcut::new(shift, Key::S);
        let export = KeyboardShortcut::new(shift, Key::E);
        let outline = KeyboardShortcut::new(shift, Key::O);
        let close = KeyboardShortcut::new(Modifiers::COMMAND, Key::W);
        let next = KeyboardShortcut::new(Modifiers::COMMAND, Key::Tab);
        let prev = KeyboardShortcut::new(shift, Key::Tab);
        let next_page = KeyboardShortcut::new(Modifiers::COMMAND, Key::PageDown);
        let prev_page = KeyboardShortcut::new(Modifiers::COMMAND, Key::PageUp);
        let consume = |s: &KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(s));

        // Сочетания с Shift проверяем первыми: иначе Ctrl+S съест Ctrl+Shift+S и т.п.
        if consume(&save_as) {
            self.save(true);
        } else if consume(&save) || consume(&save_f2) {
            self.save(false);
        } else if consume(&outline) {
            self.settings.show_outline = !self.settings.show_outline;
        } else if consume(&open) {
            self.request(Action::Open(None));
        } else if consume(&new) {
            self.request(Action::New);
        } else if consume(&export) {
            self.export_html();
        } else if consume(&close) {
            self.request(Action::Close(vec![self.tab().id]));
        } else if consume(&prev) || consume(&prev_page) {
            self.cycle_tab(-1);
        } else if consume(&next) || consume(&next_page) {
            self.cycle_tab(1);
        }
    }

    /// Закрытие окна (крестик, Alt+F4, File → Exit) с подтверждением.
    fn handle_close(&mut self, ctx: &egui::Context) {
        if self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.dialog.is_none() {
                self.request(Action::Exit);
            }
        }
    }

    /// Файлы, открытые из Проводника при запущенной программе, — вкладками в этом окне
    /// (окно на передний план выводит `single_instance`).
    fn handle_inbox(&mut self) {
        while let Some(files) = self.inbox.try_recv() {
            for path in files {
                self.request(Action::Open(Some(path)));
            }
        }
    }

    /// Файлы, перетащенные в окно, открываются; пока их несут — подсказка поверх окна.
    fn handle_drop(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_owned())
                .collect()
        });
        for path in dropped {
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
                if ui.button("Save (Ctrl+S, F2)").clicked() {
                    self.save(false);
                }
                if ui.button("Save As… (Ctrl+Shift+S)").clicked() {
                    self.save(true);
                }
                if ui
                    .add_enabled(
                        self.export.is_none(),
                        egui::Button::new("Export HTML… (Ctrl+Shift+E)"),
                    )
                    .clicked()
                {
                    self.export_html();
                }
                #[cfg(windows)]
                ui.menu_button("File association", |ui| self.association_menu(ui));
                ui.separator();
                if ui.button("Close Tab (Ctrl+W)").clicked() {
                    self.request(Action::Close(vec![self.tab().id]));
                }
                if ui
                    .add_enabled(self.tabs.len() > 1, egui::Button::new("Close Other Tabs"))
                    .clicked()
                {
                    self.request(Action::Close(self.other_tabs(self.active)));
                }
                if ui.button("Exit").clicked() {
                    self.request(Action::Exit);
                }
            });
            ui.menu_button("View", |ui| self.view_menu(ui));
            ui.menu_button("Help", |ui| {
                if ui.button("About Markdowntown").clicked() {
                    self.dialog = Some(Dialog::About);
                }
            });
            // Подпись автора справа в строке меню.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.weak(format!("{AUTHOR} 2026"));
            });
        });
    }

    /// Регистрация Markdowntown для `.md` / `.markdown` (HKCU). Приложение по умолчанию
    /// пользователь выбирает сам в Параметрах Windows — программно это запрещено.
    #[cfg(windows)]
    fn association_menu(&mut self, ui: &mut egui::Ui) {
        use crate::file_assoc::{self, Location, Status};

        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                ui.weak(format!("Unknown program path: {e}"));
                return;
            }
        };
        let loc = Location::user();
        let status = file_assoc::status(&loc, &exe);
        match &status {
            Status::NotRegistered => ui.weak("Not registered for .md files"),
            Status::Registered => ui.weak("Registered for .md and .markdown"),
            Status::OtherCopy(path) => ui
                .weak("Registered for another copy")
                .on_hover_text(path.display().to_string()),
        };
        ui.separator();

        let register_label = match status {
            Status::OtherCopy(_) => "Register this copy",
            _ => "Register for .md files",
        };
        if status != Status::Registered && ui.button(register_label).clicked() {
            let result = file_assoc::register(&loc, &exe).and_then(|()| {
                file_assoc::notify_shell();
                file_assoc::open_default_apps_settings()
            });
            self.set_status(match result {
                Ok(()) => {
                    "Registered. Choose Markdowntown as the default app in Windows Settings".into()
                }
                Err(e) => format!("Cannot register file association: {e}"),
            });
        }
        if status != Status::NotRegistered {
            if ui.button("Choose default app…").clicked()
                && let Err(e) = file_assoc::open_default_apps_settings()
            {
                self.set_status(format!("Cannot open Windows Settings: {e}"));
            }
            if ui.button("Unregister").clicked() {
                let result = file_assoc::unregister(&loc);
                file_assoc::notify_shell();
                self.set_status(match result {
                    Ok(()) => "File association removed".into(),
                    Err(e) => format!("Cannot remove file association: {e}"),
                });
            }
        }
    }

    fn view_menu(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.radio_value(&mut s.view_mode, ViewMode::Both, "Editor and preview");
        ui.radio_value(&mut s.view_mode, ViewMode::Editor, "Editor only");
        ui.radio_value(&mut s.view_mode, ViewMode::Preview, "Preview only");
        ui.separator();
        ui.checkbox(&mut s.show_outline, "Outline (Ctrl+Shift+O)");
        ui.checkbox(&mut s.sync_scroll, "Synchronized scrolling");
        ui.checkbox(&mut s.readable_width, "Readable width")
            .on_hover_text("Preview text in a centered column of comfortable line length");
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
                egui::DragValue::new(&mut s.editor_font_size)
                    .range(crate::style::EDITOR_FONT_SIZE_RANGE)
                    .speed(0.2)
                    .suffix(" pt"),
            );
        });
        ui.weak("Whole UI: Ctrl+Plus / Ctrl+Minus / Ctrl+wheel, reset: Ctrl+0");
    }

    /// Id вкладок, кроме вкладки `index` (Close Others).
    fn other_tabs(&self, index: usize) -> Vec<u64> {
        let keep = self.tabs[index].id;
        self.tabs
            .iter()
            .map(|t| t.id)
            .filter(|&id| id != keep)
            .collect()
    }

    /// Id вкладок правее вкладки `index`.
    fn tabs_to_the_right(&self, index: usize) -> Vec<u64> {
        self.tabs[index + 1..].iter().map(|t| t.id).collect()
    }

    /// Строка вкладок: клик — переключиться, «×» или средняя кнопка — закрыть,
    /// правая кнопка — меню (Close, Close Others, Close Tabs to the Right).
    fn tab_bar(&mut self, ui: &mut egui::Ui) {
        /// Что закрыть — считается после цикла, когда `self` уже не заимствован.
        enum Close {
            One(u64),
            Others(usize),
            ToTheRight(usize),
        }
        let mut activate = None;
        let mut close = None;
        let count = self.tabs.len();
        // Активная вкладка сменилась (Ctrl+Tab, открытие файла) — прокрутить к ней.
        let active_id = self.tabs[self.active].id;
        let reveal_active = self.tab_bar_revealed != Some(active_id);
        self.tab_bar_revealed = Some(active_id);
        // Полоса прокрутки под вкладками, а не поверх них (по умолчанию в egui она
        // плавающая и перекрывает вкладки); колесо мыши прокручивает вкладки вбок.
        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
        ui.style_mut().always_scroll_the_only_direction = true;
        egui::ScrollArea::horizontal()
            .id_salt("tab_bar")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (i, tab) in self.tabs.iter().enumerate() {
                        let mut name = tab.doc.display_name();
                        if tab.doc.is_dirty() {
                            name.push_str(" •");
                        }
                        let hover = tab.doc.path().map_or_else(
                            || "Not saved yet".to_owned(),
                            |p| p.display().to_string(),
                        );
                        let response = ui
                            .add(egui::Button::selectable(i == self.active, name))
                            .on_hover_text(hover);
                        if reveal_active && i == self.active {
                            response.scroll_to_me(None);
                        }
                        if response.clicked() {
                            activate = Some(i);
                        }
                        if response.middle_clicked() {
                            close = Some(Close::One(tab.id));
                        }
                        response.context_menu(|ui| {
                            if ui.button("Close").clicked() {
                                close = Some(Close::One(tab.id));
                            }
                            if ui
                                .add_enabled(count > 1, egui::Button::new("Close Others"))
                                .clicked()
                            {
                                close = Some(Close::Others(i));
                            }
                            if ui
                                .add_enabled(
                                    i + 1 < count,
                                    egui::Button::new("Close Tabs to the Right"),
                                )
                                .clicked()
                            {
                                close = Some(Close::ToTheRight(i));
                            }
                        });
                        if ui
                            .add(egui::Button::new("×").frame(false))
                            .on_hover_text("Close (Ctrl+W)")
                            .clicked()
                        {
                            close = Some(Close::One(tab.id));
                        }
                        ui.add_space(6.0);
                    }
                });
            });
        if let Some(i) = activate {
            self.activate(i);
        }
        let ids = match close {
            None => return,
            Some(Close::One(id)) => vec![id],
            Some(Close::Others(i)) => {
                // Оставшаяся вкладка становится активной.
                self.activate(i);
                self.other_tabs(i)
            }
            Some(Close::ToTheRight(i)) => self.tabs_to_the_right(i),
        };
        self.request(Action::Close(ids));
    }

    /// Ссылка под курсором (её сообщает `egui_commonmark` при рисовании превью) —
    /// строка статуса покажет, куда она ведёт. Панель статуса рисуется раньше превью,
    /// поэтому показ — со следующего кадра.
    fn update_link_hover(&mut self, ctx: &egui::Context) {
        let url = egui_commonmark::take_hovered_link(ctx);
        if url.as_deref() == self.link_hover.as_ref().map(|(u, _)| u.as_str()) {
            return;
        }
        let base = self.tab().doc.base_dir().map(Path::to_owned);
        self.link_hover = url.map(|url| {
            let description = links::describe(&url, base.as_deref(), &self.tab().doc.text);
            (url, description)
        });
        ctx.request_repaint();
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            match &self.link_hover {
                // Пока курсор над ссылкой — куда она ведёт (вместо сообщения).
                Some((_, (description, missing))) => {
                    // Место — за вычетом правой части (кодировка, окончания строк).
                    let char_width = ui.text_style_height(&egui::TextStyle::Body) * 0.5;
                    let max_chars = ((ui.available_width() - 160.0) / char_width).max(20.0);
                    let text = elide_middle(description, max_chars as usize);
                    if *missing {
                        ui.colored_label(ui.visuals().error_fg_color, text);
                    } else {
                        ui.label(text);
                    }
                }
                None => {
                    ui.label(&self.status);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let format = self.tab().doc.format();
                ui.label(format.line_ending.label());
                ui.separator();
                ui.label(if format.bom { "UTF-8 BOM" } else { "UTF-8" });
            });
        });
    }

    /// Панель оглавления слева; клик — переход к заголовку в редакторе и превью.
    fn outline_panel(&mut self, ui: &mut egui::Ui) {
        if !self.settings.show_outline {
            return;
        }
        let editor_visible = self.settings.view_mode != ViewMode::Preview;
        let tab = &mut self.tabs[self.active];
        egui::Panel::left("outline")
            .resizable(true)
            .default_size(220.0)
            .size_range(120.0..=480.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.strong("Outline");
                ui.separator();
                let version = tab.doc.version();
                let clicked = ui
                    .push_id(tab.id, |ui| {
                        tab.outline.show(ui, &tab.doc.text, version, tab.top_byte)
                    })
                    .inner;
                if let Some(byte) = clicked {
                    tab.jump_to(ui.ctx(), byte, editor_visible);
                }
            });
    }

    fn panels(&mut self, ui: &mut egui::Ui) {
        let tab = &mut self.tabs[self.active];
        tab.editor.font_size = self.settings.editor_font_size;
        tab.scroll_sync.enabled = self.settings.sync_scroll;
        tab.preview.readable_width = self.settings.readable_width;
        let id = tab.id;
        let diagrams = &mut self.diagrams;
        // Id элементов вкладки (прокрутка, таблицы, окна) — свои у каждой вкладки.
        match self.settings.view_mode {
            ViewMode::Editor => {
                egui::CentralPanel::default()
                    .show(ui, |ui| ui.push_id(id, |ui| tab.show_editor(ui)));
                tab.cancel_preview_jump();
                return;
            }
            ViewMode::Preview => {
                let preview = egui::CentralPanel::default()
                    .show(ui, |ui| {
                        ui.push_id(id, |ui| tab.show_preview(ui, diagrams)).inner
                    })
                    .inner;
                tab.after_preview_only(&preview);
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
        let editor = editor_panel.show(ui, |ui| ui.push_id(id, |ui| tab.show_editor(ui)).inner);
        if window_width > 0.0 {
            self.editor_ratio = (editor.response.rect.width() / window_width).clamp(0.1, 0.9);
        }
        let editor = editor.inner;
        let preview = egui::CentralPanel::default()
            .show(ui, |ui| {
                ui.push_id(id, |ui| tab.show_preview(ui, diagrams)).inner
            })
            .inner;
        tab.after_both(ui, &editor, &preview);
    }

    /// Ссылки, по которым кликнули в превью в этом кадре: локальные забираем у браузера
    /// (Markdown — во вкладке, `#якорь` — переход к заголовку), внешние оставляем.
    fn handle_links(&mut self, ctx: &egui::Context) {
        let base = self.tab().doc.base_dir().map(Path::to_owned);
        let mut targets = Vec::new();
        ctx.output_mut(|o| {
            o.commands.retain(|command| match command {
                egui::OutputCommand::OpenUrl(open) => {
                    match links::classify(&open.url, base.as_deref()) {
                        Target::External => true,
                        target => {
                            targets.push(target);
                            false
                        }
                    }
                }
                _ => true,
            });
        });
        for target in targets {
            match target {
                Target::External => {}
                Target::Anchor(anchor) => self.jump_to_anchor(ctx, &anchor),
                Target::Markdown { path, anchor } => {
                    self.request(Action::Open(Some(path.clone())));
                    if let Some(anchor) = anchor
                        && self.tab().has_path(&path)
                    {
                        self.jump_to_anchor(ctx, &anchor);
                    }
                }
                Target::LocalFile(path) => {
                    let message = links::open_local(&path);
                    self.set_status(message);
                }
                Target::Missing(path) => {
                    self.set_status(format!("Cannot open link: {} not found", path.display()))
                }
            }
        }
    }

    /// Переход к заголовку активной вкладки по якорю ссылки.
    fn jump_to_anchor(&mut self, ctx: &egui::Context, anchor: &str) {
        let editor_visible = self.settings.view_mode != ViewMode::Preview;
        let tab = self.tab_mut();
        match links::find_anchor(&tab.doc.text, anchor) {
            Some(byte) => tab.jump_to(ctx, byte, editor_visible),
            None => self.set_status(format!("No heading for #{anchor}")),
        }
    }

    /// Переносит текущее состояние в `settings` перед сохранением.
    fn sync_settings(&mut self, ctx: &egui::Context) {
        self.settings.zoom = ctx.zoom_factor();
        self.settings.editor_ratio = self.editor_ratio;
        self.save_session();
    }
}

impl eframe::App for MarkdowntownApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.dialog.is_none() {
            self.handle_shortcuts(&ctx);
        }
        self.handle_drop(&ctx);
        self.handle_inbox();

        if self.installed_editor_font != self.settings.editor_font {
            fonts::install(&ctx, self.settings.editor_font.as_deref());
            self.installed_editor_font = self.settings.editor_font.clone();
        }

        egui::Panel::top("menu").show(ui, |ui| self.menu(ui));
        egui::Panel::top("tabs").show(ui, |ui| self.tab_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        self.outline_panel(ui);
        self.panels(ui);
        self.update_link_hover(&ctx);
        self.handle_links(&ctx);
        handle_wheel_zoom(&ctx);
        self.show_dialog(&ctx);

        self.check_disk(&ctx);
        self.poll_export(&ctx);
        self.write_drafts(&ctx);
        self.sync_settings(&ctx);
        let title = self.window_title();
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
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

    fn wheel(unit: egui::MouseWheelUnit, y: f32, ctrl: bool) -> egui::Event {
        egui::Event::MouseWheel {
            unit,
            delta: egui::vec2(0.0, y),
            phase: egui::TouchPhase::Move,
            modifiers: if ctrl {
                egui::Modifiers::CTRL
            } else {
                egui::Modifiers::NONE
            },
        }
    }

    #[test]
    fn wheel_zoom_steps_like_keyboard() {
        use egui::MouseWheelUnit::{Line, Point};
        let approx = |a: Option<f32>, b: f32| a.is_some_and(|a| (a - b).abs() < 1e-4);
        // Щелчок колеса — ровно шаг 0.1, с округлением до шага.
        assert!(approx(wheel_zoom(1.0, &[wheel(Line, 1.0, true)]), 1.1));
        assert!(approx(wheel_zoom(1.0, &[wheel(Line, -1.0, true)]), 0.9));
        assert!(approx(wheel_zoom(1.23, &[wheel(Line, 1.0, true)]), 1.3));
        assert!(approx(
            wheel_zoom(1.0, &[wheel(Line, 1.0, true), wheel(Line, 1.0, true)]),
            1.2
        ));
        // Без Ctrl — обычная прокрутка, масштаб не трогаем.
        assert_eq!(wheel_zoom(1.0, &[wheel(Line, 1.0, false)]), None);
        assert_eq!(wheel_zoom(1.0, &[]), None);
        // Тачпад — плавно; «щипок» — множитель.
        let touchpad = wheel_zoom(1.0, &[wheel(Point, 10.0, true)]).unwrap();
        assert!(touchpad > 1.0 && touchpad < 1.1);
        assert!(approx(wheel_zoom(1.0, &[egui::Event::Zoom(1.5)]), 1.5));
        // Пределы.
        assert!(approx(wheel_zoom(5.0, &[wheel(Line, 3.0, true)]), 5.0));
        assert!(approx(wheel_zoom(0.2, &[wheel(Line, -3.0, true)]), 0.2));
    }

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
    const UI_SYMBOLS: &[char] = &['⏶', '⏷', '×', '…', '−', '“', '”', '•', '©'];

    #[test]
    fn elide_middle_keeps_both_ends() {
        assert_eq!(elide_middle("short", 10), "short");
        let path = r"Open in a tab: F:\Info\_tutorial\Game_Design\guide.md";
        let elided = elide_middle(path, 30);
        assert_eq!(elided.chars().count(), 30);
        assert!(elided.starts_with("Open in a tab:"));
        assert!(elided.ends_with("guide.md"));
        assert!(elided.contains('…'));
    }

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
