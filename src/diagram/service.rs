//! Фоновый рендер диаграмм и кэш готовых текстур.
//!
//! UI-поток запрашивает диаграмму по ключу; если её нет в кэше, задание уходит
//! в рабочий поток. Готовые картинки забираются в начале кадра и превращаются
//! в текстуры. Давно не показанные записи вытесняются.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use super::merman_backend::MermanRenderer;
use super::raster::{Rasterized, Rasterizer};
use super::{DiagramRenderer, DiagramTheme};

/// Сколько записей держим, прежде чем вытеснять неиспользуемые.
const CACHE_SOFT_LIMIT: usize = 32;
/// Запись, не показанная дольше этого, может быть вытеснена.
const EVICT_AFTER: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiagramKey {
    source_hash: u64,
    theme: DiagramTheme,
    /// `pixels_per_point` в битах f32 — чтобы ключ был `Eq + Hash`.
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
    InFlight,
    Ready {
        texture: egui::TextureHandle,
        size: egui::Vec2,
    },
    Failed(String),
}

struct Job {
    key: DiagramKey,
    source: String,
}

struct Done {
    key: DiagramKey,
    result: Result<Rasterized, String>,
}

pub struct DiagramService {
    jobs: Sender<Job>,
    done: Receiver<Done>,
    cache: HashMap<DiagramKey, (Entry, Instant)>,
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
                    let result = renderer
                        .render_svg(&job.source, job.key.theme)
                        .and_then(|svg| rasterizer.rasterize(&svg, job.key.scale()));
                    if done_tx
                        .send(Done {
                            key: job.key,
                            result,
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
        }
    }

    /// Забирает готовые результаты и вытесняет старые записи. Вызывать раз в кадр.
    pub fn begin_frame(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        for done in self.done.try_iter() {
            let entry = match done.result {
                Ok(r) => Entry::Ready {
                    texture: ctx.load_texture(
                        format!("mermaid-{:x}", done.key.source_hash),
                        r.image,
                        egui::TextureOptions::LINEAR,
                    ),
                    size: r.size,
                },
                Err(e) => Entry::Failed(e),
            };
            self.cache.insert(done.key, (entry, now));
        }
        if self.cache.len() > CACHE_SOFT_LIMIT {
            self.cache.retain(|_, (entry, used)| {
                matches!(entry, Entry::InFlight) || now.duration_since(*used) < EVICT_AFTER
            });
        }
    }

    /// Состояние диаграммы. Если её нет в кэше и `may_request` — ставит задание.
    pub fn get(&mut self, key: DiagramKey, source: &str, may_request: bool) -> DiagramState<'_> {
        if !self.cache.contains_key(&key) {
            if !may_request {
                return DiagramState::Pending;
            }
            let job = Job {
                key,
                source: source.to_owned(),
            };
            if self.jobs.send(job).is_err() {
                return DiagramState::Failed("diagram renderer thread has stopped");
            }
            self.cache.insert(key, (Entry::InFlight, Instant::now()));
        }
        self.peek(key)
    }

    /// Состояние без постановки задания.
    pub fn peek(&mut self, key: DiagramKey) -> DiagramState<'_> {
        match self.cache.get_mut(&key) {
            None => DiagramState::Pending,
            Some((entry, used)) => {
                *used = Instant::now();
                match entry {
                    Entry::InFlight => DiagramState::Pending,
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
