//! Вкладка: документ и всё состояние вокруг него — редактор (курсор, отмена, поиск),
//! превью и его кэши, синхронная прокрутка, оглавление, черновик восстановления,
//! проверка файла на диске.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::diagram::DiagramService;
use crate::document::{DiskChange, Document};
use crate::editor::{Editor, EditorGeometry};
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
    Status(String),
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
            pending_scroll: (None, None),
            outline: Outline::default(),
            top_byte: 0,
            outline_jump: None,
            outline_jump_frames: 0,
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
                    Ok(()) => "Reloaded: the file was changed on disk".into(),
                    Err(e) => format!("Cannot reload: {e}"),
                }))
            }
            DiskChange::Modified => Some(DiskEvent::Conflict),
            DiskChange::Removed => {
                self.doc.accept_disk_state();
                Some(DiskEvent::Status(
                    "The file was deleted on disk; save to recreate it".into(),
                ))
            }
        }
    }

    // ----- Панели -----

    /// Редактор и/или превью по режиму вида; синхронная прокрутка и переход
    /// по оглавлению.
    pub fn show_editor(&mut self, ui: &mut egui::Ui) -> EditorGeometry {
        let (editor_scroll, _) = self.pending_scroll;
        self.pending_scroll.0 = None;
        let geometry = self.editor.show(ui, &mut self.doc, editor_scroll);
        if self.outline_jump.is_none() {
            self.top_byte =
                geometry.byte_at_offset(&self.doc.text, geometry.offset + SECTION_PROBE);
        }
        geometry
    }

    pub fn show_preview(
        &mut self,
        ui: &mut egui::Ui,
        diagrams: &mut DiagramService,
    ) -> PreviewGeometry {
        let (_, preview_scroll) = self.pending_scroll;
        self.pending_scroll.1 = None;
        self.preview
            .show(ui, &mut self.doc, preview_scroll, diagrams)
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
        // Превью тоже переходит к заголовку — и при выключенной синхронной прокрутке.
        self.jump_preview(preview);
        if self.pending_scroll != (None, None) {
            ui.ctx().request_repaint();
        }
    }

    /// После показа одного превью (режим Preview only).
    pub fn after_preview_only(&mut self, preview: &PreviewGeometry) {
        self.jump_preview(preview);
        if self.outline_jump.is_none() {
            self.top_byte = preview.byte_for_y(preview.offset + SECTION_PROBE, self.doc.text.len());
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

    /// Только редактор: превью не прокручивается.
    pub fn cancel_preview_jump(&mut self) {
        self.outline_jump = None;
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
