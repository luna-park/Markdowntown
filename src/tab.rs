//! Вкладка: документ и всё состояние вокруг него — редактор (курсор, отмена, поиск),
//! превью и его кэши, синхронная прокрутка, оглавление, черновик восстановления,
//! проверка файла на диске.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::diagram::DiagramService;
use crate::document::{DiskChange, Document};
use crate::editor::{Editor, EditorGeometry};
use crate::i18n::{Message, msg};
use crate::outline::Outline;
use crate::preview::Preview;
use crate::recovery::{Draft, Drafts};
use crate::scroll_sync::{PreviewGeometry, ScrollSync};

/// Как часто проверять, не изменили ли файл другие программы.
const DISK_CHECK_INTERVAL: Duration = Duration::from_secs(1);
/// Как часто (не чаще) записывать черновик несохранённых правок.
const DRAFT_INTERVAL: Duration = Duration::from_secs(5);

/// Текущий раздел оглавления определяется на такой глубине от верха панели, px —
/// больше зазора над заголовком после перехода (8 px), чтобы раздел был «своим».
pub const SECTION_PROBE: f32 = 16.0;

/// Что показать пользователю после проверки файла на диске.
pub enum DiskEvent {
    Status(Message),
    /// Файл изменён другой программой, а во вкладке есть свои правки — спросить.
    Conflict,
}

pub struct Tab {
    /// Уникален в пределах запуска: id элементов egui вкладки.
    pub id: u64,
    pub doc: Document,
    pub editor: Editor,
    pub preview: Preview,
    pub scroll_sync: ScrollSync,
    /// Прокрутка панелей от синхронизации, вычисленная в прошлом кадре.
    pub pending_scroll: (Option<f32>, Option<f32>),
    pub outline: Outline,
    /// Байт документа в верхней строке ведущей панели (прошлый кадр) — текущий раздел.
    pub top_byte: usize,
    /// Переход по оглавлению: превью прокручивается к этому байту после раскладки.
    pub outline_jump: Option<usize>,
    /// Сколько кадров уже длится переход (страховка от бесконечной подстройки).
    outline_jump_frames: u32,
    /// Слов в документе: (версия документа, число) — для строки статуса.
    words: Option<((u64, u64), usize)>,

    /// Ключ черновика, пока у документа нет пути (свой у каждой безымянной вкладки).
    pub untitled_key: Option<u64>,
    /// Версия документа и путь, для которых записан последний черновик.
    pub draft_written: Option<((u64, u64), Option<PathBuf>)>,
    last_draft_time: Instant,
    last_disk_check: Instant,
}

impl Tab {
    pub fn new(id: u64, doc: Document) -> Self {
        Self {
            id,
            doc,
            editor: Editor::for_tab(id),
            preview: Preview::default(),
            scroll_sync: ScrollSync::default(),
            // Память egui (с прокруткой) сохраняется между запусками, а id вкладок
            // каждый раз начинаются заново: без этого новая вкладка открылась бы там,
            // где была прокручена прежняя вкладка с тем же id.
            pending_scroll: (Some(0.0), Some(0.0)),
            outline: Outline::default(),
            top_byte: 0,
            outline_jump: None,
            outline_jump_frames: 0,
            words: None,
            untitled_key: Some(unique_key(id)),
            draft_written: None,
            last_draft_time: Instant::now(),
            last_disk_check: Instant::now(),
        }
    }

    /// Безымянная и без правок — её можно заменить открываемым файлом.
    pub fn is_pristine(&self) -> bool {
        self.doc.path().is_none() && !self.doc.is_dirty()
    }

    pub fn has_path(&self, path: &Path) -> bool {
        self.doc
            .path()
            .is_some_and(|p| crate::settings::same_path(p, path))
    }

    /// Проверить файл на диске при следующей возможности (вкладку только что открыли).
    pub fn check_disk_soon(&mut self) {
        self.last_disk_check = Instant::now()
            .checked_sub(DISK_CHECK_INTERVAL)
            .unwrap_or_else(Instant::now);
    }

    /// Слов в документе (пересчёт — только после правок).
    pub fn word_count(&mut self) -> usize {
        let version = self.doc.version();
        match self.words {
            Some((v, words)) if v == version => words,
            _ => {
                let words = crate::editor::stats::word_count(&self.doc.text);
                self.words = Some((version, words));
                words
            }
        }
    }

    // ----- Черновик восстановления -----

    /// Пишет черновик, если есть несохранённые правки, которых ещё нет в черновике.
    pub fn write_draft(&mut self, drafts: &Drafts, ctx: &egui::Context) {
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
            drafts.remove(old_path.as_deref(), self.untitled_key);
        }
        let draft = Draft::new(path, self.untitled_key, self.doc.text.clone());
        match drafts.save(&draft) {
            Ok(()) => self.draft_written = Some(current),
            Err(e) => log::warn!("cannot write recovery draft: {e}"),
        }
        self.last_draft_time = Instant::now();
    }

    /// Удаляет черновик документа (сохранён или правки отброшены).
    pub fn discard_draft(&mut self, drafts: &Drafts) {
        if let Some((_, path)) = self.draft_written.take() {
            drafts.remove(path.as_deref(), self.untitled_key);
        }
        drafts.remove(self.doc.path(), self.untitled_key);
    }

    // ----- Файл на диске -----

    /// Раз в секунду: файл изменён другой программой? Без своих правок — перечитать.
    pub fn check_disk(&mut self, ctx: &egui::Context) -> Option<DiskEvent> {
        ctx.request_repaint_after(DISK_CHECK_INTERVAL);
        if self.last_disk_check.elapsed() < DISK_CHECK_INTERVAL {
            return None;
        }
        self.last_disk_check = Instant::now();
        match self.doc.disk_change()? {
            DiskChange::Modified if !self.doc.is_dirty() => {
                Some(DiskEvent::Status(match self.doc.reload() {
                    Ok(()) => msg!(
                        "Reloaded: the file was changed on disk",
                        "Перечитан: файл изменён на диске"
                    ),
                    Err(e) => msg!("Cannot reload: {e}", "Не удалось перечитать: {e}"),
                }))
            }
            DiskChange::Modified => Some(DiskEvent::Conflict),
            DiskChange::Removed => {
                self.doc.accept_disk_state();
                Some(DiskEvent::Status(msg!(
                    "The file was deleted on disk; save to recreate it",
                    "Файл удалён с диска; сохраните, чтобы создать его заново"
                )))
            }
        }
    }

    // ----- Панели -----

    /// Редактор и/или превью по режиму вида; синхронная прокрутка и переход
    /// по оглавлению.
    pub fn show_editor(&mut self, ui: &mut egui::Ui) -> EditorGeometry {
        let (editor_scroll, _) = self.pending_scroll;
        self.pending_scroll.0 = None;
        self.editor.show(ui, &mut self.doc, editor_scroll)
    }

    pub fn after_editor_only(&mut self, ctx: &egui::Context, editor: &EditorGeometry) {
        self.outline_jump = None;
        self.update_top_byte(
            ctx,
            editor.byte_at_offset(&self.doc.text, editor.offset + SECTION_PROBE),
        );
    }

    pub fn show_preview(
        &mut self,
        ui: &mut egui::Ui,
        diagrams: &mut DiagramService,
        preview_only: bool,
    ) -> PreviewGeometry {
        let (_, preview_scroll) = self.pending_scroll;
        self.pending_scroll.1 = None;
        self.preview
            .show(ui, &mut self.doc, preview_scroll, preview_only, diagrams)
    }

    /// После показа обеих панелей: синхронная прокрутка и переход по оглавлению.
    pub fn after_both(
        &mut self,
        ui: &egui::Ui,
        editor: &EditorGeometry,
        preview: &PreviewGeometry,
    ) {
        self.pending_scroll = if self.outline_jump.is_some() {
            self.scroll_sync.observe(editor, preview);
            (None, None)
        } else {
            self.scroll_sync.update(&self.doc.text, editor, preview)
        };
        if self.outline_jump.is_none() {
            let top = if self.scroll_sync.preview_leads() {
                preview.byte_for_y(preview.offset + SECTION_PROBE, self.doc.text.len())
            } else {
                editor.byte_at_offset(&self.doc.text, editor.offset + SECTION_PROBE)
            };
            self.update_top_byte(ui.ctx(), top);
        }
        // Превью переходит к заголовку и при выключенной синхронной прокрутке.
        self.jump_preview(preview);
        if self.pending_scroll != (None, None) {
            ui.ctx().request_repaint();
        }
    }

    /// После показа одного превью (режим Preview only).
    pub fn after_preview_only(&mut self, ctx: &egui::Context, preview: &PreviewGeometry) {
        self.jump_preview(preview);
        if self.outline_jump.is_none() {
            self.update_top_byte(
                ctx,
                preview.byte_for_y(preview.offset + SECTION_PROBE, self.doc.text.len()),
            );
        }
    }

    fn update_top_byte(&mut self, ctx: &egui::Context, byte: usize) {
        if self.top_byte != byte {
            self.top_byte = byte;
            // Оглавление уже было отрисовано в этом кадре.
            ctx.request_repaint();
        }
    }

    /// Переход по оглавлению: курсор на заголовок (если редактор виден) и прокрутка.
    pub fn jump_to(&mut self, ctx: &egui::Context, byte: usize, editor_visible: bool) {
        if editor_visible {
            self.editor.jump_to(ctx, &self.doc.text, byte);
        }
        self.outline_jump = Some(byte);
        // Сразу подсветить выбранный раздел, не дожидаясь прокрутки.
        self.top_byte = byte;
    }

    /// Прокрутка превью к заголовку из оглавления. Высоты блоков, которые ещё не
    /// рисовались, известны приблизительно — поэтому повторяем, пока позиция не устоится.
    fn jump_preview(&mut self, preview: &PreviewGeometry) {
        let Some(byte) = self.outline_jump else {
            return;
        };
        let target =
            (preview.y_for_target(byte, self.doc.text.len()) - 8.0).clamp(0.0, preview.max_offset);
        if (target - preview.offset).abs() > 1.0 {
            self.pending_scroll.1 = Some(target);
            self.scroll_sync.expect_preview(target);
            self.outline_jump_frames += 1;
            if self.outline_jump_frames > 10 {
                self.outline_jump = None;
            }
        } else {
            self.outline_jump = None;
        }
        if self.outline_jump.is_none() {
            self.outline_jump_frames = 0;
        }
    }
}

/// Ключ черновика безымянной вкладки: не повторяется между запусками, чтобы новая
/// вкладка не затёрла черновик прошлого (ещё не восстановленный) сеанса.
fn unique_key(id: u64) -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    nanos ^ id.rotate_left(48)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_only_find_shortcut_focuses_query() {
        let ctx = egui::Context::default();
        let mut diagrams = DiagramService::new(&ctx);
        let mut tab = Tab::new(1, Document::new_untitled("Первый абзац".to_owned()));
        let command = egui::Modifiers::CTRL | egui::Modifiers::COMMAND;
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::F,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: command,
            }],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            tab.show_preview(ui, &mut diagrams, true);
        });
        output.textures_delta.clear();
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.text() == crate::i18n::tr!("Find", "Найти")
        )));
    }

    #[test]
    fn preview_find_highlights_formatted_text_and_visits_offscreen_matches() {
        let ctx = egui::Context::default();
        let mut diagrams = DiagramService::new(&ctx);
        let filler = "Другой абзац.\n\n".repeat(80);
        let text = format!(
            "СИНЯЯ **птица** у окна.\n\n{filler}Синяя [птица](https://example.org) в саду.\n\n{filler}синяя птица у реки.\n\n{filler}"
        );
        let mut tab = Tab::new(1, Document::new_untitled(text));
        let mut frame = |events, preview_only| {
            let mut scroll = 0.0;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(420.0, 240.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let preview = tab.show_preview(ui, &mut diagrams, preview_only);
                    if preview_only {
                        tab.after_preview_only(ui.ctx(), &preview);
                    }
                    scroll = preview.offset;
                },
            );
            output.textures_delta.clear();
            (scroll, output)
        };
        let key = |key, modifiers| {
            vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers,
                },
            ]
        };
        frame(vec![], true);
        frame(vec![], true);
        frame(key(egui::Key::F, egui::Modifiers::COMMAND), true);
        let (_, first) = frame(
            vec![
                egui::Event::ModifiersChanged(egui::Modifiers::NONE),
                egui::Event::Text("синяя птица".into()),
            ],
            true,
        );
        let has_highlighted = |output: &egui::FullOutput, word: &str| {
            output.shapes.iter().any(|shape| match &shape.shape {
                egui::Shape::Text(text)
                    if text
                        .galley
                        .rect
                        .translate(text.pos.to_vec2())
                        .intersects(shape.clip_rect) =>
                {
                    text.galley.job.sections.iter().any(|section| {
                        section.format.background != egui::Color32::TRANSPARENT
                            && text.galley.text()
                                [section.byte_range.start.0..section.byte_range.end.0]
                                .trim()
                                == word
                    })
                }
                _ => false,
            })
        };
        assert!(has_highlighted(&first, "СИНЯЯ"));
        assert!(has_highlighted(&first, "птица"));

        frame(key(egui::Key::Enter, egui::Modifiers::NONE), true);
        let (offset, second) = frame(vec![], true);
        assert!(offset > 500.0, "{offset}");
        assert!(has_highlighted(&second, "Синяя"));
        assert!(has_highlighted(&second, "птица"));

        let mut previous = key(egui::Key::Enter, egui::Modifiers::SHIFT);
        previous.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        frame(previous, true);
        let (offset, _) = frame(vec![], true);
        assert!(offset < 30.0, "{offset}");

        let (_, both) = frame(key(egui::Key::F, egui::Modifiers::COMMAND), false);
        assert!(!has_highlighted(&both, "СИНЯЯ"));
        assert!(!both.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.text() == "синяя птица"
        )));
    }

    /// Кадр «редактор + превью» без окна; смещения прокрутки панелей.
    fn frame(ctx: &egui::Context, tab: &mut Tab, diagrams: &mut DiagramService) -> (f32, f32) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 800.0),
            )),
            ..Default::default()
        };
        let mut offsets = (0.0, 0.0);
        let id = tab.id;
        let mut out = ctx.run_ui(input, |ui| {
            let editor = egui::Panel::left("editor")
                .exact_size(600.0)
                .show(ui, |ui| ui.push_id(id, |ui| tab.show_editor(ui)).inner)
                .inner;
            let preview = egui::CentralPanel::default()
                .show(ui, |ui| {
                    ui.push_id(id, |ui| tab.show_preview(ui, diagrams, false))
                        .inner
                })
                .inner;
            offsets = (editor.offset, preview.offset);
        });
        out.textures_delta.clear();
        offsets
    }

    /// Память egui (прокрутка) сохраняется между запусками, а id вкладок каждый раз
    /// начинаются с 1: новая вкладка не должна открываться там, где была прокручена
    /// прежняя вкладка с тем же id.
    #[test]
    fn new_tab_starts_at_top() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, None);
        let mut diagrams = DiagramService::new(&ctx);
        let text = "Paragraph of text.\n\n".repeat(300);

        let mut old = Tab::new(1, Document::new_untitled(text.clone()));
        frame(&ctx, &mut old, &mut diagrams);
        old.pending_scroll = (Some(1000.0), Some(1000.0));
        let (editor, preview) = frame(&ctx, &mut old, &mut diagrams);
        assert!(editor > 0.0 && preview > 0.0, "{editor} {preview}");

        let mut new = Tab::new(1, Document::new_untitled(text));
        assert_eq!(frame(&ctx, &mut new, &mut diagrams), (0.0, 0.0));
    }
}
