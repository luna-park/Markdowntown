//! Углублённая проверка merman: холодный старт, тёмная тема, ошибки, большие диаграммы.

use std::time::Instant;

use merman::svg::SvgPipeline;
use merman::{
    Engine, MermaidConfig, OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest,
};

fn render(r: &Renderer, src: &str) -> Result<String, String> {
    let req = SvgRequest {
        pipeline: Some(SvgPipeline::resvg_safe()),
        ..Default::default()
    };
    match r
        .render(RenderRequest::svg(src, OperationControl::new(), req))
        .map_err(|e| e.to_string())?
    {
        RenderOutput::Svg(Some(svg)) => Ok(svg.svg().to_string()),
        _ => Err("no diagram".into()),
    }
}

fn png(svg: &str, bg: resvg::tiny_skia::Color, out: &str) {
    let mut db = resvg::usvg::fontdb::Database::new();
    db.load_system_fonts();
    let opt = resvg::usvg::Options {
        fontdb: std::sync::Arc::new(db),
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_str(svg, &opt).unwrap();
    let s = tree.size().to_int_size();
    let mut p = resvg::tiny_skia::Pixmap::new(s.width(), s.height()).unwrap();
    p.fill(bg);
    resvg::render(&tree, Default::default(), &mut p.as_mut());
    p.save_png(out).unwrap();
}

fn assert_send_sync<T: Send + Sync>() {}

fn main() {
    assert_send_sync::<Renderer>();

    // 1. Холодный старт: создание Renderer + первый рендер.
    let t = Instant::now();
    let r = Renderer::new();
    let flow = std::fs::read_to_string("diagrams/01_flowchart.mmd").unwrap();
    render(&r, &flow).unwrap();
    println!(
        "cold start (new + first render): {:.1} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    let t = Instant::now();
    render(&r, &flow).unwrap();
    println!("warm render: {:.2} ms", t.elapsed().as_secs_f64() * 1e3);

    // 2. Тёмная тема через конфиг хоста.
    let dark = Renderer::new().with_engine(Engine::new().with_site_config(
        MermaidConfig::from_value(serde_json::json!({"theme": "dark"})),
    ));
    std::fs::create_dir_all("out/probe").unwrap();
    for name in ["01_flowchart", "02_sequence", "04_state", "07_pie"] {
        let src = std::fs::read_to_string(format!("diagrams/{name}.mmd")).unwrap();
        let svg = render(&dark, &src).unwrap();
        png(
            &svg,
            resvg::tiny_skia::Color::from_rgba8(27, 27, 27, 255),
            &format!("out/probe/dark_{name}.png"),
        );
    }
    println!("dark theme: out/probe/dark_*.png");

    // 3. Ошибки синтаксиса: не должно быть паники, нужно внятное сообщение.
    let bad = [
        ("empty", ""),
        ("unknown type", "notadiagram\n  a --> b"),
        ("dangling edge", "flowchart TD\n  A -->"),
        ("broken sequence", "sequenceDiagram\n  A->>"),
        ("unclosed bracket", "flowchart TD\n  A[Начало --> B"),
        ("garbage", "flowchart TD\n  }{}{}{ ][ ][ <<<>>> ;;;"),
        ("half-typed class", "classDiagram\n  class A {\n    +int x"),
    ];
    for (name, src) in bad {
        let res = std::panic::catch_unwind(|| render(&r, src));
        let msg = match res {
            Err(_) => "PANIC".to_string(),
            Ok(Ok(_)) => "Ok (rendered)".to_string(),
            Ok(Err(e)) => format!(
                "Err: {}",
                e.lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(110)
                    .collect::<String>()
            ),
        };
        println!("error case [{name}]: {msg}");
    }

    // 4. Большие диаграммы.
    let mut big = String::from("flowchart TD\n");
    for i in 0..200 {
        big.push_str(&format!(
            "  n{i}[Узел {i}] --> n{}[Узел {}]\n",
            (i * 7 + 3) % 200,
            (i * 7 + 3) % 200
        ));
        if i % 3 == 0 {
            big.push_str(&format!("  n{i} --> n{}\n", (i + 1) % 200));
        }
    }
    let t = Instant::now();
    let ok = render(&r, &big).is_ok();
    println!(
        "flowchart 200 nodes / ~267 edges: {:.1} ms (ok={ok})",
        t.elapsed().as_secs_f64() * 1e3
    );

    let mut seq = String::from("sequenceDiagram\n");
    for i in 0..150 {
        seq.push_str(&format!("  P{}->>P{}: сообщение {i}\n", i % 6, (i + 1) % 6));
    }
    let t = Instant::now();
    let ok = render(&r, &seq).is_ok();
    println!(
        "sequence 150 messages: {:.1} ms (ok={ok})",
        t.elapsed().as_secs_f64() * 1e3
    );
}
