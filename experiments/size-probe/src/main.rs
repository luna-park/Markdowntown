#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;


fn probe() {
    use merman::{OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest};
    let src = std::env::args().nth(1).unwrap_or_default();
    if let Ok(RenderOutput::Svg(Some(svg))) = Renderer::new().render(RenderRequest::svg(&src, OperationControl::new(), SvgRequest::default())) {
        let tree = resvg::usvg::Tree::from_str(svg.svg(), &Default::default()).unwrap();
        let mut p = resvg::tiny_skia::Pixmap::new(10, 10).unwrap();
        resvg::render(&tree, Default::default(), &mut p.as_mut());
        println!("{}", p.data().len());
    }
}

fn main() -> eframe::Result {
    if std::env::args().len() > 1 { probe(); }
    
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("mdview")
            .with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };

    eframe::run_native(
        "mdview",
        options,
        Box::new(|cc| Ok(Box::new(app::MdViewApp::new(cc)))),
    )
}
