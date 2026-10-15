//! Ресурсы exe для Windows: иконка (Проводник, панель задач) и сведения о версии
//! (Свойства → Подробно). Нужен `rc.exe` из Windows SDK — он ставится вместе с MSVC.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico")
        .set("ProductName", "Markdowntown")
        .set("FileDescription", "Markdowntown — Markdown editor")
        .set(
            "LegalCopyright",
            "© 2026 luna-park discoteque. Licensed under MIT OR Apache-2.0",
        );
    if let Err(e) = res.compile() {
        // Без ресурсов exe работает, просто со стандартной иконкой.
        println!("cargo:warning=cannot embed Windows resources: {e}");
    }
}
