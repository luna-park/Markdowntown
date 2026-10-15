//! Фоновый рендер диаграмм и кэш готовых текстур.
//!
//! UI-поток запрашивает диаграмму по ключу; если её нет в кэше, задание уходит
//! в рабочий поток. Готовые картинки забираются в начале кадра и превращаются
//! в текстуры. Давно не показанные записи вытесняются.
//!
//! Задания, которые в очередном кадре никто не запросил (блок отредактирован или
//! удалён, окно увеличения закрыто), отменяются — в том числе уже начатые.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use super::merman_backend::MermanRenderer;
use super::raster::{Rasterized, Rasterizer};
use super::{DiagramRenderer, DiagramTheme, RenderControl};

/// Сколько записей держим, прежде чем вытеснять неиспользуемые.
const CACHE_SOFT_LIMIT: usize = 32;
/// Запись, не показанная дольше этого, может быть вытеснена.
const EVICT_AFTER: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiagramKey {
    source_hash: u64,
    theme: DiagramTheme,
    /// Пикселей на точку в битах f32 — чтобы ключ был `Eq + Hash`.
    scale_bits: u32,
}

impl DiagramKey {
    pub fn new(source: &str, theme: DiagramTheme, scale: f32) -> Self {
        let mut h = DefaultHasher::new();
        source.hash(&mut h);
        Self {
            source_hash: h.finish(),
            theme,
            scale_bits: scale.to_bits(),
        }
    }

    /// Тот же исходник и тема, другой масштаб растра.
    pub fn with_scale(self, scale: f32) -> Self {
        Self {
            scale_bits: scale.to_bits(),
            ..self
        }
    }

    fn scale(&self) -> f32 {
        f32::from_bits(self.scale_bits)
    }
}

pub enum DiagramState<'a> {
    Ready {
        texture: &'a egui::TextureHandle,
        size: egui::Vec2,
    },
    Failed(&'a str),
    /// Рендерится или ещё не запрошена (идёт debounce).
    Pending,
}

enum Entry {
    InFlight(RenderControl),
    Ready {
        texture: egui::TextureHandle,
        size: egui::Vec2,
    },
    Failed(String),
}

struct Job {
    key: DiagramKey,
    source: String,
    control: RenderControl,
}

enum Outcome {
    Done(Result<Rasterized, String>),
    Cancelled,
}

struct Done {
    key: DiagramKey,
    outcome: Outcome,
}

pub struct DiagramService {
    jobs: Sender<Job>,
    done: Receiver<Done>,
    cache: HashMap<DiagramKey, (Entry, Instant)>,
    /// Ключи, запрошенные через [`Self::get`] в текущем кадре.
    wanted: HashSet<DiagramKey>,
}

impl DiagramService {
    pub fn new(ctx: &egui::Context) -> Self {
        let (jobs_tx, jobs_rx) = channel::<Job>();
        let (done_tx, done_rx) = channel::<Done>();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("diagram-renderer".into())
            .spawn(move || {
                let renderer = MermanRenderer::new();
                let rasterizer = Rasterizer::new();
                for job in jobs_rx {
                    let outcome = run_job(&renderer, &rasterizer, &job);
                    if done_tx
                        .send(Done {
                            key: job.key,
                            outcome,
                        })
                        .is_err()
                    {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("failed to spawn diagram renderer thread");
        Self {
            jobs: jobs_tx,
            done: done_rx,
            cache: HashMap::new(),
            wanted: HashSet::new(),
        }
    }

    /// Забирает готовые результаты и вытесняет старые записи. Вызывать в начале кадра.
    pub fn begin_frame(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        for done in self.done.try_iter() {
            let entry = match done.outcome {
                // Отменённое задание: запись уже убрана в `end_frame`.
                Outcome::Cancelled => continue,
                Outcome::Done(Ok(r)) => Entry::Ready {
                    texture: ctx.load_texture(
                        format!("mermaid-{:x}", done.key.source_hash),
                        r.image,
                        egui::TextureOptions::LINEAR,
                    ),
                    size: r.size,
                },
                Outcome::Done(Err(e)) => {
                    log::debug!("mermaid render failed: {e}");
                    Entry::Failed(e)
                }
            };
            self.cache.insert(done.key, (entry, now));
        }
        if self.cache.len() > CACHE_SOFT_LIMIT {
            self.cache.retain(|_, (entry, used)| {
                matches!(entry, Entry::InFlight(_)) || now.duration_since(*used) < EVICT_AFTER
            });
        }
        self.wanted.clear();
    }

    /// Отменяет задания, которые в этом кадре никто не запросил. Вызывать в конце кадра.
    pub fn end_frame(&mut self) {
        let wanted = &self.wanted;
        self.cache.retain(|key, (entry, _)| match entry {
            Entry::InFlight(control) if !wanted.contains(key) => {
                control.cancel();
                false
            }
            _ => true,
        });
    }

    /// Состояние диаграммы. Если её нет в кэше и `may_request` — ставит задание.
    pub fn get(&mut self, key: DiagramKey, source: &str, may_request: bool) -> DiagramState<'_> {
        self.wanted.insert(key);
        if !self.cache.contains_key(&key) {
            if !may_request {
                return DiagramState::Pending;
            }
            let control = RenderControl::new();
            let job = Job {
                key,
                source: source.to_owned(),
                control: control.clone(),
            };
            if self.jobs.send(job).is_err() {
                return DiagramState::Failed("diagram renderer thread has stopped");
            }
            self.cache
                .insert(key, (Entry::InFlight(control), Instant::now()));
        }
        self.peek(key)
    }

    /// Состояние без постановки задания (и без защиты от отмены).
    pub fn peek(&mut self, key: DiagramKey) -> DiagramState<'_> {
        match self.cache.get_mut(&key) {
            None => DiagramState::Pending,
            Some((entry, used)) => {
                *used = Instant::now();
                match entry {
                    Entry::InFlight(_) => DiagramState::Pending,
                    Entry::Ready { texture, size } => DiagramState::Ready {
                        texture,
                        size: *size,
                    },
                    Entry::Failed(e) => DiagramState::Failed(e),
                }
            }
        }
    }
}

fn run_job(renderer: &dyn DiagramRenderer, rasterizer: &Rasterizer, job: &Job) -> Outcome {
    if job.control.is_cancelled() {
        return Outcome::Cancelled;
    }
    let svg = renderer.render_svg(&job.source, job.key.theme, &job.control);
    // Ошибка из-за отмены — не ошибка диаграммы, показывать её не нужно.
    if job.control.is_cancelled() {
        return Outcome::Cancelled;
    }
    Outcome::Done(svg.and_then(|svg| rasterizer.rasterize(&svg, job.key.scale())))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "flowchart TD\n  A --> B";

    fn key() -> DiagramKey {
        DiagramKey::new(SOURCE, DiagramTheme::Light, 1.0)
    }

    fn in_flight(service: &DiagramService) -> Option<RenderControl> {
        match service.cache.get(&key()) {
            Some((Entry::InFlight(c), _)) => Some(c.clone()),
            _ => None,
        }
    }

    #[test]
    fn unrequested_job_is_cancelled_and_can_be_requested_again() {
        let ctx = egui::Context::default();
        let mut service = DiagramService::new(&ctx);

        service.begin_frame(&ctx);
        service.get(key(), SOURCE, true);
        let control = in_flight(&service).expect("job queued");
        service.end_frame();
        assert!(!control.is_cancelled(), "requested in this frame");

        // Следующий кадр: диаграмму никто не запросил.
        service.begin_frame(&ctx);
        if let Some(control) = in_flight(&service) {
            service.end_frame();
            assert!(control.is_cancelled());
            assert!(in_flight(&service).is_none());
        }

        // Повторный запрос ставит задание заново и в итоге даёт картинку.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            service.begin_frame(&ctx);
            let ready = matches!(service.get(key(), SOURCE, true), DiagramState::Ready { .. });
            service.end_frame();
            if ready {
                break;
            }
            assert!(Instant::now() < deadline, "diagram was not rendered");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn debounced_request_does_not_queue() {
        let ctx = egui::Context::default();
        let mut service = DiagramService::new(&ctx);
        service.begin_frame(&ctx);
        assert!(matches!(
            service.get(key(), SOURCE, false),
            DiagramState::Pending
        ));
        assert!(service.cache.is_empty());
    }
}
