//! Замеры времени кадра без окна: `cargo test --release perf -- --ignored --nocapture`.

use std::time::Instant;

use crate::document::Document;
use crate::editor::Editor;
use crate::preview::Preview;

/// Превью со своим сервисом диаграмм (в приложении сервис общий на все вкладки).
struct TestPreview {
    preview: Preview,
    diagrams: crate::diagram::DiagramService,
}

impl TestPreview {
    fn new(ctx: &egui::Context) -> Self {
        Self {
            preview: Preview::default(),
            diagrams: crate::diagram::DiagramService::new(ctx),
        }
    }

    fn show(
        &mut self,
        ui: &mut egui::Ui,
        doc: &mut Document,
        scroll_to: Option<f32>,
    ) -> crate::scroll_sync::PreviewGeometry {
        self.preview.show(ui, doc, scroll_to, &mut self.diagrams)
    }
}

/// Документ ~200 КБ из повторённого showcase (без картинок по сети).
fn big_document() -> String {
    let showcase = include_str!("../examples/showcase.md").replace(
        "![Rust logo](https://www.rust-lang.org/logos/rust-logo-128x128.png)",
        "",
    );
    let mut text = String::new();
    while text.len() < 200 * 1024 {
        text.push_str(&showcase);
        text.push('\n');
    }
    text
}

#[derive(Clone, Copy)]
enum Panes {
    Both,
    Editor,
    Preview,
}

fn frame(ctx: &egui::Context, editor: &mut Editor, preview: &mut TestPreview, doc: &mut Document) {
    frame_with(ctx, editor, preview, doc, Panes::Both);
}

fn frame_with(
    ctx: &egui::Context,
    editor: &mut Editor,
    preview: &mut TestPreview,
    doc: &mut Document,
    panes: Panes,
) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1600.0, 1000.0),
        )),
        ..Default::default()
    };
    let mut out = ctx.run_ui(input, |ui| match panes {
        Panes::Both => {
            egui::Panel::left("editor")
                .exact_size(800.0)
                .show(ui, |ui| editor.show(ui, doc, None));
            egui::CentralPanel::default().show(ui, |ui| preview.show(ui, doc, None));
        }
        Panes::Editor => {
            egui::CentralPanel::default().show(ui, |ui| editor.show(ui, doc, None));
        }
        Panes::Preview => {
            egui::CentralPanel::default().show(ui, |ui| preview.show(ui, doc, None));
        }
    });
    out.textures_delta.clear();
}

#[test]
#[ignore = "замер, запускать вручную"]
fn perf_idle_and_typing_frames() {
    let ctx = egui::Context::default();
    crate::style::apply_global(&ctx);
    crate::fonts::install(&ctx, None);
    let mut doc = Document::new_untitled(big_document());
    let mut editor = Editor::default();
    let mut preview = TestPreview::new(&ctx);
    println!("document: {} KB", doc.text.len() / 1024);

    // Прогрев: шрифты, кэши.
    for _ in 0..3 {
        frame(&ctx, &mut editor, &mut preview, &mut doc);
    }

    let n = 20;
    let t = Instant::now();
    for _ in 0..n {
        frame(&ctx, &mut editor, &mut preview, &mut doc);
    }
    println!(
        "idle frame:   {:.1} ms",
        t.elapsed().as_secs_f64() * 1e3 / n as f64
    );

    let t = Instant::now();
    for i in 0..n {
        doc.text.insert(10, if i % 2 == 0 { 'x' } else { 'y' });
        doc.mark_edited();
        frame(&ctx, &mut editor, &mut preview, &mut doc);
    }
    println!(
        "typing frame: {:.1} ms",
        t.elapsed().as_secs_f64() * 1e3 / n as f64
    );
}

#[test]
#[ignore = "замер, запускать вручную"]
fn perf_breakdown() {
    for (name, panes) in [("editor", Panes::Editor), ("preview", Panes::Preview)] {
        let ctx = egui::Context::default();
        crate::style::apply_global(&ctx);
        crate::fonts::install(&ctx, None);
        let mut doc = Document::new_untitled(big_document());
        let mut editor = Editor::default();
        let mut preview = TestPreview::new(&ctx);
        for _ in 0..3 {
            frame_with(&ctx, &mut editor, &mut preview, &mut doc, panes);
        }
        let n = 20;
        let t = Instant::now();
        for _ in 0..n {
            frame_with(&ctx, &mut editor, &mut preview, &mut doc, panes);
        }
        let idle = t.elapsed().as_secs_f64() * 1e3 / n as f64;
        let t = Instant::now();
        for i in 0..n {
            doc.text.insert(10, if i % 2 == 0 { 'x' } else { 'y' });
            doc.mark_edited();
            frame_with(&ctx, &mut editor, &mut preview, &mut doc, panes);
        }
        let typing = t.elapsed().as_secs_f64() * 1e3 / n as f64;
        println!("{name:8} idle {idle:5.1} ms, typing {typing:5.1} ms");
    }
}

fn repeat_to_200k(chunk: &str) -> String {
    let mut text = String::new();
    while text.len() < 200 * 1024 {
        text.push_str(chunk);
        text.push('\n');
    }
    text
}

fn preview_idle_ms(text: String) -> f64 {
    let ctx = egui::Context::default();
    crate::style::apply_global(&ctx);
    crate::fonts::install(&ctx, None);
    let mut doc = Document::new_untitled(text);
    let mut editor = Editor::default();
    let mut preview = TestPreview::new(&ctx);
    for _ in 0..3 {
        frame_with(&ctx, &mut editor, &mut preview, &mut doc, Panes::Preview);
    }
    let n = 10;
    let t = Instant::now();
    for _ in 0..n {
        frame_with(&ctx, &mut editor, &mut preview, &mut doc, Panes::Preview);
    }
    t.elapsed().as_secs_f64() * 1e3 / n as f64
}

#[test]
#[ignore = "замер, запускать вручную"]
fn perf_content_kinds() {
    let showcase = big_document();
    // Без блоков кода: убрать всё между ``` (включая mermaid).
    let mut no_code = String::new();
    let mut in_code = false;
    for line in showcase.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if !in_code {
            no_code.push_str(line);
            no_code.push('\n');
        }
    }
    let paragraphs = repeat_to_200k(
        "Lorem ipsum dolor sit amet, **consectetur** adipiscing elit, sed do eiusmod tempor \
         incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud.\n",
    );
    let code = repeat_to_200k("```rust\nfn main() {\n    println!(\"hello {}\", 1 + 2);\n}\n```\n");
    println!("showcase     {:5.1} ms", preview_idle_ms(showcase));
    println!("no code      {:5.1} ms", preview_idle_ms(no_code));
    println!("paragraphs   {:5.1} ms", preview_idle_ms(paragraphs));
    println!("code only    {:5.1} ms", preview_idle_ms(code));
}

/// Якоря превью после нескольких кадров в окне `height` с прокруткой `offset`.
fn preview_anchors(text: &str, height: f32, offset: f32) -> (Vec<(usize, f32)>, f64) {
    let ctx = egui::Context::default();
    crate::style::apply_global(&ctx);
    crate::fonts::install(&ctx, None);
    let mut doc = Document::new_untitled(text);
    let mut preview = TestPreview::new(&ctx);
    let mut geometry = None;
    let mut last_ms = 0.0;
    for i in 0..6 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, height),
            )),
            ..Default::default()
        };
        let t = Instant::now();
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                geometry = Some(preview.show(ui, &mut doc, (i > 0).then_some(offset)));
            });
        });
        last_ms = t.elapsed().as_secs_f64() * 1e3;
        out.textures_delta.clear();
    }
    (geometry.unwrap().anchors, last_ms)
}

#[test]
fn culling_keeps_block_positions() {
    // Без mermaid: диаграммы рендерятся асинхронно, их высота меняется между кадрами.
    let mut text = String::new();
    for chunk in include_str!("../examples/showcase.md")
        .split("## Mermaid")
        .take(1)
    {
        while text.len() < 40 * 1024 {
            text.push_str(chunk);
            text.push('\n');
        }
    }
    let text = text.replace(
        "![Rust logo](https://www.rust-lang.org/logos/rust-logo-128x128.png)",
        "",
    );
    let (full, _) = preview_anchors(&text, 100_000.0, 0.0);
    let (culled, _) = preview_anchors(&text, 1000.0, 20_000.0);
    assert_eq!(full.len(), culled.len());
    let worst = full
        .iter()
        .zip(&culled)
        .map(|(a, b)| {
            assert_eq!(a.0, b.0);
            (a.1 - b.1).abs()
        })
        .fold(0.0f32, f32::max);
    assert!(worst < 1.0, "block positions drift by {worst} px");
}

#[test]
#[ignore = "замер, запускать вручную"]
fn perf_culled_preview() {
    let text = big_document();
    let (_, ms) = preview_anchors(&text, 1000.0, 100_000.0);
    println!("culled preview frame: {ms:.1} ms");
}

/// Первое переключение на вкладку: создание вкладки и первые кадры её панелей
/// (кэша высот блоков ещё нет). `cargo test --release perf_first_tab_frames -- --ignored --nocapture`.
#[test]
#[ignore = "замер, запускать вручную"]
fn perf_first_tab_frames() {
    for (name, text) in [
        (
            "showcase",
            include_str!("../examples/showcase.md").to_owned(),
        ),
        ("200 KB", big_document()),
    ] {
        let ctx = egui::Context::default();
        crate::style::apply_global(&ctx);
        crate::fonts::install(&ctx, None);
        let mut doc = Document::new_untitled(text);
        // Прогрев контекста (шрифты), как в работающем приложении.
        let mut warm = TestPreview::new(&ctx);
        let mut warm_editor = Editor::default();
        let mut warm_doc = Document::new_untitled("# warm\n\ntext\n");
        for _ in 0..3 {
            frame(&ctx, &mut warm_editor, &mut warm, &mut warm_doc);
        }

        let t = Instant::now();
        let mut editor = Editor::for_tab(7);
        let mut preview = TestPreview::new(&ctx);
        let create = t.elapsed().as_secs_f64() * 1e3;
        let mut frames = Vec::new();
        for _ in 0..4 {
            let t = Instant::now();
            frame(&ctx, &mut editor, &mut preview, &mut doc);
            frames.push(t.elapsed().as_secs_f64() * 1e3);
        }
        let frames: Vec<String> = frames.iter().map(|ms| format!("{ms:.1}")).collect();
        println!(
            "{name}: new tab {create:.1} ms, first frames {} ms",
            frames.join(" / ")
        );

        // Отдельно: первый кадр только редактора и только превью новой вкладки.
        for (pane, panes) in [("editor", Panes::Editor), ("preview", Panes::Preview)] {
            let mut doc = Document::new_untitled(doc.text.clone());
            let mut editor = Editor::for_tab(8);
            let mut preview = TestPreview::new(&ctx);
            let t = Instant::now();
            frame_with(&ctx, &mut editor, &mut preview, &mut doc, panes);
            println!("  {pane} only: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
        }
    }
}

/// Документ, где много блоков кода: раздел с описанием и блоком Rust в `lines` строк,
/// повторённый до ~200 КБ.
fn code_heavy_document(lines: usize) -> String {
    let code = include_str!("editor/commands.rs")
        .lines()
        .cycle()
        .take(lines)
        .collect::<Vec<_>>()
        .join("\n");
    let mut text = String::new();
    let mut i = 0;
    while text.len() < 200 * 1024 {
        i += 1;
        text.push_str(&format!(
            "## Section {i}\n\nSome text before the code block.\n\n```rust\n// block {i}\n{code}\n```\n\n"
        ));
    }
    text
}

/// Прокрутка по 120 px за кадр: превью вниз и обратно вверх (блоки, ушедшие за край,
/// появляются снова), затем редактор + превью вместе (синхронная прокрутка). Время кадра
/// UI и тесселяции: среднее, 95-й перцентиль, худший; «пики» — кадры дольше 3 средних
/// (первый кадр прохода — смена раскладки, не прокрутка).
/// `cargo test --release perf_scroll -- --ignored --nocapture`.
#[test]
#[ignore = "замер, запускать вручную"]
fn perf_scroll() {
    for (name, text) in [
        ("showcase x N", big_document()),
        ("code blocks of 30 lines", code_heavy_document(30)),
        ("code blocks of 300 lines", code_heavy_document(300)),
    ] {
        let ctx = egui::Context::default();
        crate::style::apply_global(&ctx);
        crate::fonts::install(&ctx, None);
        let mut doc = Document::new_untitled(text);
        let mut preview = TestPreview::new(&ctx);
        let mut editor = Editor::default();
        // Кадр: превью (и редактор — при синхронной прокрутке) на заданных долях прокрутки;
        // время — вместе с тесселяцией (её делает eframe после `run_ui`).
        let mut frame = |both: bool, preview_offset: f32, editor_offset: f32| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600.0, 1000.0),
                )),
                ..Default::default()
            };
            let (mut preview_max, mut editor_max) = (0.0, 0.0);
            let t = Instant::now();
            let mut out = ctx.run_ui(input, |ui| {
                if both {
                    egui::Panel::left("editor")
                        .exact_size(800.0)
                        .show(ui, |ui| {
                            editor_max = editor.show(ui, &mut doc, Some(editor_offset)).max_offset;
                        });
                }
                egui::CentralPanel::default().show(ui, |ui| {
                    preview_max = preview.show(ui, &mut doc, Some(preview_offset)).max_offset;
                });
            });
            let ui_ms = t.elapsed().as_secs_f64() * 1e3;
            out.textures_delta.clear();
            let t = Instant::now();
            let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
            let tess_ms = t.elapsed().as_secs_f64() * 1e3;
            (ui_ms, tess_ms, preview_max, editor_max)
        };
        let (_, _, preview_max, editor_max) = frame(true, 0.0, 0.0);
        // Прокрутка вниз и обратно вверх: блоки, ушедшие за край, появляются снова.
        for (panes, both) in [
            ("preview, down", false),
            ("preview, up", false),
            ("editor + preview, down", true),
        ] {
            let mut ui_times = Vec::new();
            let mut tess_times = Vec::new();
            let up = panes.ends_with("up");
            let mut step = 0.0;
            while step <= preview_max {
                let offset = if up { preview_max - step } else { step };
                step += 120.0;
                let editor_offset = offset / preview_max.max(1.0) * editor_max;
                let (ui_ms, tess_ms, _, _) = frame(both, offset, editor_offset);
                ui_times.push(ui_ms);
                tess_times.push(tess_ms);
            }
            // Пики: кадры дольше 3 средних — сколько их и на каких кадрах.
            let avg = ui_times.iter().sum::<f64>() / ui_times.len() as f64;
            let spikes: Vec<String> = ui_times
                .iter()
                .enumerate()
                .filter(|(_, ms)| **ms > 3.0 * avg.max(0.5))
                .map(|(i, ms)| format!("#{i}: {ms:.0}"))
                .collect();
            println!("  spikes ({}): {}", spikes.len(), spikes.join(", "));
            let stats = |times: &mut Vec<f64>| {
                times.sort_by(f64::total_cmp);
                let avg = times.iter().sum::<f64>() / times.len() as f64;
                let p95 = times[times.len() * 95 / 100];
                let max = times[times.len() - 1];
                format!("avg {avg:.1}, p95 {p95:.1}, max {max:.1} ms")
            };
            println!(
                "{name}, {panes}: {} frames; ui {}; tessellation {}",
                ui_times.len(),
                stats(&mut ui_times),
                stats(&mut tess_times)
            );
        }
    }
}
