//! Генерация файлов иконки из `assets/icon.svg`:
//! `cargo test generate_icon_files -- --ignored`. Результат коммитится.

use std::path::Path;

const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

fn render(svg: &resvg::usvg::Tree, size: u32) -> resvg::tiny_skia::Pixmap {
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).expect("pixmap");
    let scale = size as f32 / svg.size().width();
    resvg::render(
        svg,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap
}

#[test]
#[ignore = "генерация ресурсов, запускать вручную"]
fn generate_icon_files() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let svg = std::fs::read_to_string(assets.join("icon.svg")).unwrap();
    let tree = resvg::usvg::Tree::from_str(&svg, &Default::default()).unwrap();

    // PNG 256 — иконка окна во время работы.
    render(&tree, 256)
        .save_png(assets.join("icon-256.png"))
        .unwrap();

    // ICO со всеми размерами — иконка exe (Проводник, панель задач).
    let frames: Vec<image::codecs::ico::IcoFrame> = SIZES
        .iter()
        .map(|&size| {
            let pixmap = render(&tree, size);
            // tiny-skia хранит премультиплицированную альфу, ICO ждёт обычную.
            let rgba: Vec<u8> = pixmap
                .pixels()
                .iter()
                .flat_map(|p| {
                    let c = p.demultiply();
                    [c.red(), c.green(), c.blue(), c.alpha()]
                })
                .collect();
            image::codecs::ico::IcoFrame::as_png(&rgba, size, size, image::ExtendedColorType::Rgba8)
                .unwrap()
        })
        .collect();
    let file = std::fs::File::create(assets.join("icon.ico")).unwrap();
    image::codecs::ico::IcoEncoder::new(file)
        .encode_images(&frames)
        .unwrap();
}
