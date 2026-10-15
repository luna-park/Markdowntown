//! Сравнение нативных Rust-рендереров Mermaid.
//! Для каждого движка и каждой диаграммы: время рендера, SVG, PNG через resvg.

use std::fs;
use std::path::Path;
use std::time::Instant;

type Engine = (&'static str, fn(&str) -> Result<String, String>);

fn merman_render(src: &str) -> Result<String, String> {
    use merman::svg::SvgPipeline;
    use merman::{OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest};
    let req = SvgRequest {
        pipeline: Some(SvgPipeline::resvg_safe()),
        ..Default::default()
    };
    match Renderer::new()
        .render(RenderRequest::svg(src, OperationControl::new(), req))
        .map_err(|e| e.to_string())?
    {
        RenderOutput::Svg(Some(svg)) => Ok(svg.svg().to_string()),
        _ => Err("no diagram".into()),
    }
}

fn mmdr_render(src: &str) -> Result<String, String> {
    mermaid_rs_renderer::render(src).map_err(|e| e.to_string())
}

fn little_render(src: &str) -> Result<String, String> {
    mermaid_little::convert(src).map_err(|e| e.to_string())
}

fn mermaid_svg_render(src: &str) -> Result<String, String> {
    mermaid_svg::render(src).map_err(|e| e.to_string())
}

fn mermaid_render_render(src: &str) -> Result<String, String> {
    mermaid_render::render_diagram(src, &Default::default(), &mut mermaid_render::EstimatedMeasure)
        // Крейт отдаёт содержимое без корневого <svg>, размеры — отдельно.
        .map(|(body, w, h)| {
            format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">{body}</svg>"#)
        })
        .map_err(|e| format!("{e:?}"))
}

const ENGINES: &[Engine] = &[
    ("merman", merman_render),
    ("mmdr", mmdr_render),
    ("mermaid-little", little_render),
    ("mermaid-svg", mermaid_svg_render),
    ("mermaid-render", mermaid_render_render),
];

fn rasterize(
    svg: &str,
    fontdb: &std::sync::Arc<resvg::usvg::fontdb::Database>,
    out: &Path,
) -> Result<(), String> {
    let opt = resvg::usvg::Options {
        fontdb: fontdb.clone(),
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_str(svg, &opt).map_err(|e| e.to_string())?;
    let size = tree.size().to_int_size();
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(size.width().max(1), size.height().max(1)).ok_or("pixmap")?;
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::default(),
        &mut pixmap.as_mut(),
    );
    pixmap.save_png(out).map_err(|e| e.to_string())
}

/// Сводный лист: эталон и все движки рядом, с подписями.
fn contact_sheet(stem: &str, fontdb: &std::sync::Arc<resvg::usvg::fontdb::Database>) {
    const CELL_W: f32 = 520.0;
    const CELL_H: f32 = 560.0;
    let cols: Vec<&str> = std::iter::once("reference")
        .chain(ENGINES.iter().map(|e| e.0))
        .collect();
    let mut body = String::new();
    for (i, col) in cols.iter().enumerate() {
        let x = i as f32 * CELL_W;
        body.push_str(&format!(
            r#"<text x="{}" y="24" font-family="Arial" font-size="20" font-weight="bold">{col}</text>"#,
            x + 10.0
        ));
        let png = Path::new("out").join(col).join(format!("{stem}.png"));
        if png.exists() {
            body.push_str(&format!(
                r#"<image x="{}" y="36" width="{}" height="{}" preserveAspectRatio="xMidYMin meet" href="{}"/>"#,
                x + 5.0, CELL_W - 10.0, CELL_H - 46.0,
                png.canonicalize().unwrap().display().to_string().replace('\\', "/").trim_start_matches("//?/")
            ));
        } else {
            body.push_str(&format!(
                r#"<text x="{}" y="80" font-family="Arial" font-size="18" fill="red">нет результата</text>"#,
                x + 10.0
            ));
        }
        body.push_str(&format!(
            r##"<rect x="{x}" y="0" width="{CELL_W}" height="{CELL_H}" fill="none" stroke="#999"/>"##
        ));
    }
    let w = CELL_W * cols.len() as f32;
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{CELL_H}"><rect width="100%" height="100%" fill="white"/>{body}</svg>"#
    );
    fs::create_dir_all("out/sheets").unwrap();
    if let Err(e) = rasterize(
        &svg,
        fontdb,
        &Path::new("out/sheets").join(format!("{stem}.png")),
    ) {
        eprintln!("sheet {stem}: {e}");
    }
}

fn main() {
    let mut db = resvg::usvg::fontdb::Database::new();
    db.load_system_fonts();
    let fontdb = std::sync::Arc::new(db);

    let mut diagrams: Vec<_> = fs::read_dir("diagrams")
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "mmd"))
        .collect();
    diagrams.sort();

    println!("| engine | diagram | status | time, ms | foreignObject |");
    println!("|---|---|---|---|---|");
    for (name, render) in ENGINES {
        let dir = Path::new("out").join(name);
        fs::create_dir_all(&dir).unwrap();
        for d in &diagrams {
            let stem = d.file_stem().unwrap().to_string_lossy();
            let src = fs::read_to_string(d).unwrap();
            // Паника внутри движка — тоже результат.
            let first = std::panic::catch_unwind(|| render(&src));
            let res = match first {
                Ok(r) => r,
                Err(_) => Err("PANIC".into()),
            };
            match res {
                Ok(svg) => {
                    const N: u32 = 10;
                    let t = Instant::now();
                    for _ in 0..N {
                        let _ = render(&src);
                    }
                    let ms = t.elapsed().as_secs_f64() * 1000.0 / N as f64;
                    fs::write(dir.join(format!("{stem}.svg")), &svg).unwrap();
                    let fo = svg.contains("<foreignObject");
                    let status = match rasterize(&svg, &fontdb, &dir.join(format!("{stem}.png"))) {
                        Ok(()) => "ok".to_string(),
                        Err(e) => format!("resvg: {e}"),
                    };
                    println!("| {name} | {stem} | {status} | {ms:.2} | {fo} |");
                }
                Err(e) => {
                    let e: String = e.chars().take(80).collect();
                    println!("| {name} | {stem} | ERR {} | - | - |", e.replace('\n', " "));
                }
            }
        }
    }
    for d in &diagrams {
        contact_sheet(&d.file_stem().unwrap().to_string_lossy(), &fontdb);
    }
}
