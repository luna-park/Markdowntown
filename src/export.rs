//! Экспорт документа в самодостаточный HTML-файл: стили, подсветка кода, диаграммы
//! Mermaid (SVG) и локальные картинки встроены в страницу (data URI).
//!
//! Светлая и тёмная тема — по `prefers-color-scheme` браузера; у каждой диаграммы два
//! варианта SVG, браузер показывает подходящий (`<picture>`). При печати — светлая.
//! Raw HTML из документа, как и в превью, не исполняется, а выводится текстом.

use std::path::Path;

use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd};
use syntect::highlighting::ThemeSet;
use syntect::html::{ClassStyle, ClassedHTMLGenerator, css_for_theme_with_class_style};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::diagram::{DiagramRenderer, DiagramTheme, RenderControl};
use crate::segment;

/// Префикс CSS-классов подсветки — чтобы не пересекались с классами страницы.
const CLASS_STYLE: ClassStyle = ClassStyle::SpacedPrefixed { prefix: "hl-" };
const LIGHT_CODE_THEME: &str = "InspiredGitHub";
const DARK_CODE_THEME: &str = "base16-ocean.dark";

/// Тема страницы — по настройке темы приложения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTheme {
    /// По теме системы/браузера читателя (`prefers-color-scheme`).
    Auto,
    Light,
    /// Тёмная на экране, светлая при печати.
    Dark,
}

impl PageTheme {
    /// Media-запрос тёмного оформления; `None` — тёмного варианта в файле нет.
    fn dark_media(self) -> Option<&'static str> {
        match self {
            Self::Auto => Some("screen and (prefers-color-scheme: dark)"),
            Self::Light => None,
            Self::Dark => Some("screen"),
        }
    }

    /// Дополнение к [`Self::dark_media`]: когда действует светлая подсветка кода.
    fn light_media(self) -> &'static str {
        match self {
            Self::Auto => "print, (prefers-color-scheme: light)",
            Self::Light => "all",
            Self::Dark => "print",
        }
    }

    fn color_scheme(self) -> &'static str {
        match self {
            Self::Auto => "light dark",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// Результат экспорта: страница и то, что не удалось встроить.
pub struct Exported {
    pub html: String,
    /// Диаграммы с ошибкой разбора (на странице — текст ошибки и исходник).
    pub failed_diagrams: usize,
    /// Локальные картинки, которые не нашлись или не читаются (ссылка оставлена как есть).
    pub missing_images: usize,
}

impl Exported {
    /// Предупреждения для строки статуса, например «1 diagram failed».
    pub fn warnings(&self) -> Option<String> {
        let mut parts = Vec::new();
        match self.failed_diagrams {
            0 => {}
            1 => parts.push("1 diagram failed".to_owned()),
            n => parts.push(format!("{n} diagrams failed")),
        }
        match self.missing_images {
            0 => {}
            1 => parts.push("1 image not found".to_owned()),
            n => parts.push(format!("{n} images not found")),
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

/// Собирает HTML-страницу. `base_dir` — папка документа (для относительных путей
/// картинок), `fallback_title` — заголовок, если в документе нет заголовка.
pub fn to_html(
    text: &str,
    base_dir: Option<&Path>,
    fallback_title: &str,
    theme: PageTheme,
    renderer: &dyn DiagramRenderer,
) -> Exported {
    let mut exporter = Exporter {
        base_dir,
        theme,
        renderer,
        syntaxes: SyntaxSet::load_defaults_newlines(),
        failed_diagrams: 0,
        missing_images: 0,
    };
    // GFM — ради alerts (`> [!NOTE]`); в превью их распознаёт egui_commonmark.
    let options = segment::parser_options() | Options::ENABLE_GFM;
    let mut events = exporter.transform(Parser::new_ext(text, options));
    let title = add_heading_ids(&mut events).unwrap_or_else(|| fallback_title.to_owned());

    let mut body = String::with_capacity(text.len() * 2);
    pulldown_cmark::html::push_html(&mut body, events.into_iter());

    let html = format!(
        "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"color-scheme\" content=\"{scheme}\">\n\
         <meta name=\"generator\" content=\"{generator}\">\n\
         <title>{title}</title>\n<style>\n{css}</style>\n</head>\n\
         <body>\n<main class=\"markdown-body\">\n{body}</main>\n</body>\n</html>\n",
        generator = crate::app::APP_TITLE,
        title = escape(&title),
        scheme = theme.color_scheme(),
        css = stylesheet(theme),
    );
    Exported {
        html,
        failed_diagrams: exporter.failed_diagrams,
        missing_images: exporter.missing_images,
    }
}

struct Exporter<'r> {
    base_dir: Option<&'r Path>,
    theme: PageTheme,
    renderer: &'r dyn DiagramRenderer,
    syntaxes: SyntaxSet,
    failed_diagrams: usize,
    missing_images: usize,
}

impl Exporter<'_> {
    /// Подменяет события, которые `push_html` выводит не так, как нужно.
    fn transform<'a>(&mut self, parser: Parser<'a>) -> Vec<Event<'a>> {
        let mut out = Vec::new();
        let mut events = parser;
        while let Some(event) = events.next() {
            match event {
                Event::Start(Tag::CodeBlock(kind)) => {
                    let info = match &kind {
                        CodeBlockKind::Fenced(info) => info.to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    let mut code = String::new();
                    for event in events.by_ref() {
                        match event {
                            Event::End(TagEnd::CodeBlock) => break,
                            Event::Text(t) => code.push_str(&t),
                            _ => {}
                        }
                    }
                    let html = if segment::is_mermaid(&info) {
                        self.diagram(&code)
                    } else {
                        let lang = info.split_whitespace().next().unwrap_or("");
                        self.code_block(lang, &code)
                    };
                    out.push(Event::Html(html.into()));
                }
                // Raw HTML не поддерживаем: показываем исходник, как в превью.
                Event::Start(Tag::HtmlBlock) => {
                    out.push(Event::Html("<pre class=\"raw-html\">".into()));
                }
                Event::End(TagEnd::HtmlBlock) => out.push(Event::Html("</pre>\n".into())),
                Event::Html(t) | Event::InlineHtml(t) => out.push(Event::Text(t)),
                Event::InlineMath(tex) => {
                    out.push(Event::InlineHtml(math_html(&tex, false).into()))
                }
                Event::DisplayMath(tex) => {
                    out.push(Event::InlineHtml(math_html(&tex, true).into()))
                }
                Event::Start(Tag::Image {
                    link_type,
                    dest_url,
                    title,
                    id,
                }) => {
                    let dest_url = match self.embed_image(&dest_url) {
                        Some(data) => CowStr::from(data),
                        None => dest_url,
                    };
                    out.push(Event::Start(Tag::Image {
                        link_type,
                        dest_url,
                        title,
                        id,
                    }));
                }
                event => out.push(event),
            }
        }
        out
    }

    fn code_block(&self, lang: &str, code: &str) -> String {
        let highlighted = (!lang.is_empty())
            .then(|| self.syntaxes.find_syntax_by_token(lang))
            .flatten()
            .and_then(|syntax| {
                let mut generator =
                    ClassedHTMLGenerator::new_with_class_style(syntax, &self.syntaxes, CLASS_STYLE);
                for line in LinesWithEndings::from(code) {
                    generator
                        .parse_html_for_line_which_includes_newline(line)
                        .ok()?;
                }
                Some(generator.finalize())
            });
        match highlighted {
            Some(html) => format!("<pre class=\"hl-code\"><code>{html}</code></pre>\n"),
            None => format!("<pre><code>{}</code></pre>\n", escape(code)),
        }
    }

    fn diagram(&mut self, source: &str) -> String {
        let control = RenderControl::new();
        let light = self
            .renderer
            .render_svg(source, DiagramTheme::Light, &control);
        // Светлый вариант нужен всегда (печать), тёмный — если тема его допускает.
        let dark = self.theme.dark_media().map(|media| {
            self.renderer
                .render_svg(source, DiagramTheme::Dark, &control)
                .map(|svg| (media, svg))
        });
        match (light, dark.transpose()) {
            (Ok(light), Ok(dark)) => {
                let source_tag = dark.map_or_else(String::new, |(media, svg)| {
                    format!(
                        "<source media=\"{media}\" srcset=\"{}\">",
                        svg_data_uri(&svg)
                    )
                });
                format!(
                    "<figure class=\"mermaid\"><picture>{source_tag}\
                     <img alt=\"Mermaid diagram\" src=\"{}\"></picture></figure>\n",
                    svg_data_uri(&light),
                )
            }
            (Err(e), _) | (_, Err(e)) => {
                self.failed_diagrams += 1;
                log::warn!("export: mermaid diagram failed: {e}");
                format!(
                    "<div class=\"mermaid-error\"><p>Mermaid error: {}</p>\
                     <pre><code>{}</code></pre></div>\n",
                    escape(&e),
                    escape(source),
                )
            }
        }
    }

    /// Локальную картинку — в data URI. `None` — оставить ссылку как есть
    /// (URL, якорь или файл не найден).
    fn embed_image(&mut self, url: &str) -> Option<String> {
        if url.is_empty() || url.contains("://") || url.starts_with("data:") {
            return None;
        }
        let decoded = percent_decode(url);
        let path = Path::new(&decoded);
        let full = if path.is_absolute() {
            path.to_owned()
        } else {
            self.base_dir?.join(path)
        };
        let Some(mime) = image_mime(&full) else {
            self.missing_images += 1;
            log::warn!("export: unsupported image type {}", full.display());
            return None;
        };
        match std::fs::read(&full) {
            Ok(bytes) => Some(format!("data:{mime};base64,{}", base64(&bytes))),
            Err(e) => {
                self.missing_images += 1;
                log::warn!("export: cannot read image {}: {e}", full.display());
                None
            }
        }
    }
}

/// Формула — встроенный SVG. Размеры в em и цвет `currentColor`: формула следует размеру
/// и цвету текста вокруг (в том числе теме страницы). Ошибка — исходник с подсказкой.
fn math_html(tex: &str, display: bool) -> String {
    let formula = match crate::math::render(tex, display, crate::math::MATH_SCALE, [0, 0, 0]) {
        Ok(f) => f,
        Err(e) => {
            let source = if display {
                format!("$${tex}$$")
            } else {
                format!("${tex}$")
            };
            return format!(
                "<code class=\"math-error\" title=\"{}\">{}</code>",
                escape(&e),
                escape(&source)
            );
        }
    };
    let svg = formula.svg.replace("rgba(0,0,0,1)", "currentColor");
    let style = format!(
        "width:{:.4}em;height:{:.4}em;vertical-align:-{:.4}em",
        formula.width, formula.height, formula.depth
    );
    let svg = svg.replacen("<svg", &format!("<svg style=\"{style}\" role=\"img\""), 1);
    let label = escape(tex);
    if display {
        format!("<span class=\"math-display\" aria-label=\"{label}\">{svg}</span>")
    } else {
        format!("<span class=\"math-inline\" aria-label=\"{label}\">{svg}</span>")
    }
}

/// Проставляет заголовкам `id` в стиле GitHub (`#my-section`), чтобы работали ссылки
/// внутри документа. Возвращает текст первого заголовка — для `<title>`.
fn add_heading_ids(events: &mut [Event<'_>]) -> Option<String> {
    let mut used = std::collections::HashMap::<String, usize>::new();
    let mut first_title = None;
    let mut i = 0;
    while i < events.len() {
        if !matches!(events[i], Event::Start(Tag::Heading { .. })) {
            i += 1;
            continue;
        }
        let start = i;
        let mut text = String::new();
        i += 1;
        while i < events.len() && !matches!(events[i], Event::End(TagEnd::Heading(_))) {
            if let Event::Text(t) | Event::Code(t) = &events[i] {
                text.push_str(t);
            }
            i += 1;
        }
        let mut slug = slugify(&text);
        let count = used.entry(slug.clone()).or_insert(0);
        if *count > 0 {
            slug = format!("{slug}-{count}");
        }
        *count += 1;
        if let Event::Start(Tag::Heading { id, .. }) = &mut events[start]
            && id.is_none()
        {
            *id = Some(slug.into());
        }
        if first_title.is_none() && !text.trim().is_empty() {
            first_title = Some(text.trim().to_owned());
        }
    }
    first_title
}

/// Якорь заголовка как на GitHub: строчные буквы, пробелы → `-`, пунктуация убирается.
fn slugify(text: &str) -> String {
    text.trim()
        .chars()
        .flat_map(char::to_lowercase)
        .filter_map(|c| match c {
            ' ' => Some('-'),
            '-' | '_' => Some(c),
            c if c.is_alphanumeric() => Some(c),
            _ => None,
        })
        .collect()
}

fn image_mime(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

fn svg_data_uri(svg: &str) -> String {
    format!(
        "data:image/svg+xml;base64,{}",
        base64(with_intrinsic_size(svg).as_bytes())
    )
}

/// merman задаёт корню `width="100%"` (и `max-width` в стиле) — внутри `<img>` у такого
/// SVG нет собственного размера. Ставим `width`/`height` из `viewBox`.
pub(crate) fn with_intrinsic_size(svg: &str) -> String {
    let Some(start) = svg.find("<svg") else {
        return svg.to_owned();
    };
    let Some(end) = svg[start..].find('>').map(|e| start + e) else {
        return svg.to_owned();
    };
    let root = &svg[start..end];
    let size = attr(root, "viewBox").and_then(|v| {
        let n: Vec<f64> = v.split([' ', ',']).filter_map(|x| x.parse().ok()).collect();
        (n.len() == 4).then(|| (n[2], n[3]))
    });
    let Some((width, height)) = size else {
        return svg.to_owned();
    };
    let mut new_root = root.to_owned();
    for name in ["width", "height"] {
        if let Some(value) = attr(&new_root, name) {
            new_root = new_root.replacen(&format!(" {name}=\"{value}\""), "", 1);
        }
    }
    new_root.push_str(&format!(" width=\"{width}\" height=\"{height}\""));
    format!("{}{new_root}{}", &svg[..start], &svg[end..])
}

/// Значение атрибута `name="..."` в теге.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let from = tag.find(&key)? + key.len();
    let len = tag[from..].find('"')?;
    Some(&tag[from..from + len])
}

/// `%20` и т.п. в пути картинки → байты; некорректные последовательности — как есть.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// Стили страницы (как у GitHub) и подсветки кода для обеих тем.
fn stylesheet(theme: PageTheme) -> String {
    let themes = ThemeSet::load_defaults();
    let code_css = |name: &str| {
        themes
            .themes
            .get(name)
            .and_then(|t| css_for_theme_with_class_style(t, CLASS_STYLE).ok())
            .unwrap_or_default()
    };
    // Темы подсветки не должны смешиваться: правила одной темы для областей, которых
    // нет в другой, иначе протекают (тёмный текст на тёмном фоне). Печать — светлая.
    let mut css = format!(
        "{PAGE_CSS}@media {} {{\n{}}}\n",
        theme.light_media(),
        code_css(LIGHT_CODE_THEME)
    );
    if let Some(media) = theme.dark_media() {
        css.push_str(&format!(
            "@media {media} {{\n:root {{{DARK_VARS}}}\n{}}}\n",
            code_css(DARK_CODE_THEME)
        ));
    }
    css.push_str(CODE_BOX_CSS);
    css
}

/// Палитра тёмной темы (как у GitHub) — переопределяет переменные `:root`.
const DARK_VARS: &str = "
  --fg: #e6edf3; --muted: #9198a1; --bg: #0d1117; --border: #3d444d;
  --code-bg: #151b23; --link: #4493f8;
  --note: #4493f8; --tip: #3fb950; --important: #ab7df8; --warning: #d29922; --caution: #f85149;
";

const PAGE_CSS: &str = r#":root {
  --fg: #1f2328; --muted: #59636e; --bg: #ffffff; --border: #d1d9e0;
  --code-bg: #f6f8fa; --link: #0969da;
  --note: #0969da; --tip: #1a7f37; --important: #8250df; --warning: #9a6700; --caution: #d1242f;
}
html { background: var(--bg); color: var(--fg); }
body { margin: 0; }
.markdown-body {
  box-sizing: border-box; max-width: 980px; margin: 0 auto; padding: 32px 24px;
  font: 16px/1.5 -apple-system, "Segoe UI", "Noto Sans", Helvetica, Arial, sans-serif;
  overflow-wrap: break-word;
}
.markdown-body > :first-child { margin-top: 0; }
a { color: var(--link); text-decoration: none; }
a:hover { text-decoration: underline; }
h1, h2, h3, h4, h5, h6 { margin: 24px 0 16px; font-weight: 600; line-height: 1.25; }
h1 { font-size: 2em; padding-bottom: .3em; border-bottom: 1px solid var(--border); }
h2 { font-size: 1.5em; padding-bottom: .3em; border-bottom: 1px solid var(--border); }
h3 { font-size: 1.25em; } h4 { font-size: 1em; } h5 { font-size: .875em; }
h6 { font-size: .85em; color: var(--muted); }
p, blockquote, ul, ol, dl, table, pre, figure, .mermaid-error { margin: 0 0 16px; }
ul, ol { padding-left: 2em; }
li + li { margin-top: .25em; }
li > ul, li > ol { margin: .25em 0 0; }
li:has(> input[type=checkbox]) { list-style: none; }
li > input[type=checkbox] { margin: 0 .3em 0 -1.4em; vertical-align: middle; }
blockquote { padding: 0 1em; color: var(--muted); border-left: .25em solid var(--border); }
hr { height: .25em; margin: 24px 0; padding: 0; border: 0; background: var(--border); }
code, pre, .raw-html {
  font-family: ui-monospace, Consolas, "Cascadia Mono", "Liberation Mono", monospace;
  font-size: 85%;
}
:not(pre) > code { padding: .2em .4em; border-radius: 6px; background: var(--code-bg); }
pre { padding: 16px; overflow: auto; line-height: 1.45; border-radius: 6px; }
pre code { font-size: 100%; }
.raw-html { white-space: pre-wrap; }
table { border-collapse: collapse; display: block; width: max-content; max-width: 100%; overflow: auto; }
th, td { padding: 6px 13px; border: 1px solid var(--border); }
th { font-weight: 600; }
tr:nth-child(2n) { background: var(--code-bg); }
img { max-width: 100%; }
figure.mermaid { text-align: center; }
figure.mermaid img { height: auto; }
.mermaid-error p { color: var(--caution); font-weight: 600; }
.math-display { display: block; margin: 16px 0; text-align: center; overflow-x: auto; }
.math-inline svg, .math-display svg { overflow: visible; }
.math-error { color: var(--caution); }
dt { font-weight: 600; } dd { margin: 0 0 8px 2em; }
.footnote-definition { font-size: .875em; color: var(--muted); }
.footnote-definition p { display: inline; }
.footnote-definition-label { margin-right: .4em; }
.markdown-alert-note, .markdown-alert-tip, .markdown-alert-important,
.markdown-alert-warning, .markdown-alert-caution { color: inherit; border-left-color: var(--alert); }
.markdown-alert-note { --alert: var(--note); } .markdown-alert-tip { --alert: var(--tip); }
.markdown-alert-important { --alert: var(--important); }
.markdown-alert-warning { --alert: var(--warning); } .markdown-alert-caution { --alert: var(--caution); }
[class^=markdown-alert-]::before { display: block; padding-top: 8px; font-weight: 600; color: var(--alert); }
.markdown-alert-note::before { content: "Note"; } .markdown-alert-tip::before { content: "Tip"; }
.markdown-alert-important::before { content: "Important"; }
.markdown-alert-warning::before { content: "Warning"; } .markdown-alert-caution::before { content: "Caution"; }
@media print {
  .markdown-body { max-width: none; padding: 0; }
  pre, figure, table, img { break-inside: avoid; }
}
"#;

/// После CSS подсветки: фон блоков кода — из палитры страницы, а не темы.
const CODE_BOX_CSS: &str = "pre, pre.hl-code { background: var(--code-bg); }\n";

#[cfg(test)]
mod tests {
    use super::*;

    /// Рендерер без merman: «SVG» с исходником внутри, `fail` — ошибка разбора.
    struct FakeRenderer;

    impl DiagramRenderer for FakeRenderer {
        fn render_svg(
            &self,
            source: &str,
            theme: DiagramTheme,
            _control: &RenderControl,
        ) -> Result<String, String> {
            if source.contains("fail") {
                return Err("parse error <here>".into());
            }
            Ok(format!("<svg>{theme:?}</svg>"))
        }
    }

    fn export(text: &str) -> Exported {
        to_html(text, None, "Untitled", PageTheme::Auto, &FakeRenderer)
    }

    #[test]
    fn page_has_title_and_body() {
        let out = export("# Hello <World>\n\nSome *text*.\n");
        assert!(out.html.starts_with("<!DOCTYPE html>"));
        assert!(out.html.contains("<title>Hello &lt;World&gt;</title>"));
        assert!(out.html.contains("<em>text</em>"));
        assert!(export("plain").html.contains("<title>Untitled</title>"));
    }

    #[test]
    fn mermaid_is_embedded_in_both_themes() {
        let out = export("```mermaid\ngraph TD\n  A --> B\n```\n");
        let light = svg_data_uri("<svg>Light</svg>");
        let dark = svg_data_uri("<svg>Dark</svg>");
        assert!(out.html.contains(&format!("src=\"{light}\"")));
        assert!(out.html.contains(&format!("srcset=\"{dark}\"")));
        assert_eq!(out.failed_diagrams, 0);
        assert!(!out.html.contains("graph TD"));
    }

    #[test]
    fn page_theme_follows_setting() {
        let text = "```mermaid\ngraph TD\n```\n";
        let light = to_html(text, None, "t", PageTheme::Light, &FakeRenderer).html;
        assert!(light.contains("content=\"light\""));
        assert!(
            !light.contains("<source"),
            "light page needs no dark diagram"
        );
        assert!(!light.contains("#0d1117"), "light page has no dark palette");

        let dark = to_html(text, None, "t", PageTheme::Dark, &FakeRenderer).html;
        assert!(dark.contains("content=\"dark\""));
        assert!(dark.contains("<source media=\"screen\""));
        assert!(dark.contains("@media screen {"));
        assert!(dark.contains("@media print {"), "printing stays light");

        let auto = export(text).html;
        assert!(auto.contains("prefers-color-scheme: dark"));
    }

    #[test]
    fn mermaid_error_shows_message_and_source() {
        let out = export("```mermaid\nfail <x>\n```\n");
        assert_eq!(out.failed_diagrams, 1);
        assert!(out.html.contains("parse error &lt;here&gt;"));
        assert!(out.html.contains("fail &lt;x&gt;"));
        assert_eq!(out.warnings().as_deref(), Some("1 diagram failed"));
    }

    #[test]
    fn code_is_highlighted_or_escaped() {
        let out = export("```rust\nfn main() {}\n```\n\n```nolang\na < b\n```\n");
        assert!(out.html.contains("<pre class=\"hl-code\">"));
        assert!(out.html.contains("hl-"), "no highlight classes");
        assert!(out.html.contains("<pre><code>a &lt; b\n</code></pre>"));
    }

    #[test]
    fn raw_html_is_shown_as_text() {
        let out = export("<div onclick=\"x()\">hi</div>\n\ntext <b>bold</b>\n");
        assert!(!out.html.contains("<div onclick"));
        assert!(out.html.contains("&lt;div onclick"));
        assert!(out.html.contains("text &lt;b&gt;bold&lt;/b&gt;"));
    }

    #[test]
    fn gfm_elements() {
        let out =
            export("| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done\n\n~~old~~\n\n> [!NOTE]\n> hi\n");
        assert!(out.html.contains("<table>"));
        assert!(out.html.contains("type=\"checkbox\""));
        assert!(out.html.contains("<del>old</del>"));
        assert!(out.html.contains("markdown-alert-note"));
    }

    #[test]
    fn headings_get_unique_ids() {
        let out = export("# Раздел `один`!\n\n## Same\n\n## Same\n");
        assert!(out.html.contains("<h1 id=\"раздел-один\">"));
        assert!(out.html.contains("<h2 id=\"same\">"));
        assert!(out.html.contains("<h2 id=\"same-1\">"));
    }

    #[test]
    fn local_images_are_embedded() {
        let dir = std::env::temp_dir().join(format!("mdt-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("my pic.png"), b"PNG").unwrap();
        let text = "![a](my%20pic.png) ![b](missing.png) ![c](https://example.com/x.png)";
        let out = to_html(text, Some(&dir), "t", PageTheme::Auto, &FakeRenderer);
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(out.html.contains("src=\"data:image/png;base64,UE5H\""));
        assert!(out.html.contains("src=\"missing.png\""));
        assert!(out.html.contains("src=\"https://example.com/x.png\""));
        assert_eq!(out.missing_images, 1);
    }

    #[test]
    fn math_is_embedded_as_svg() {
        let out = export("Inline $x^2$ and\n\n$$\\frac{a}{b}$$\n\nbad $\\frac{1}$ end\n");
        assert!(
            out.html
                .contains("<span class=\"math-inline\" aria-label=\"x^2\"><svg")
        );
        assert!(out.html.contains("<span class=\"math-display\""));
        assert!(out.html.contains("currentColor"));
        assert!(!out.html.contains("rgba(0,0,0,1)"));
        assert!(out.html.contains("class=\"math-error\""));
        // Цены — не формулы.
        let prices = export("It costs $5 and $10.\n");
        assert!(!prices.html.contains("class=\"math-inline\""));
    }

    #[test]
    fn base64_matches_rfc_vectors() {
        let cases = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (input, expected) in cases {
            assert_eq!(base64(input.as_bytes()), expected);
        }
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("a%20b%2Fc"), "a b/c");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%D0%AF"), "Я");
    }

    /// Настоящий merman: SVG-диаграмма получает размер, иначе `<img>` его не знает.
    #[test]
    fn real_diagram_svg_has_size() {
        let renderer = crate::diagram::MermanRenderer::new();
        let svg = renderer
            .render_svg(
                "graph TD\n  A --> B",
                DiagramTheme::Light,
                &RenderControl::new(),
            )
            .unwrap();
        let sized = with_intrinsic_size(&svg);
        let root = &sized[..sized.find('>').unwrap()];
        assert!(!root.contains("width=\"100%\""), "{root}");
        assert!(attr(root, "width").unwrap().parse::<f64>().unwrap() > 0.0);
        assert!(attr(root, "height").unwrap().parse::<f64>().unwrap() > 0.0);
    }

    #[test]
    fn intrinsic_size_from_view_box() {
        let svg = r#"<?xml?><svg id="m" width="100%" viewBox="0 0 168.5 156"><g/></svg>"#;
        assert_eq!(
            with_intrinsic_size(svg),
            r#"<?xml?><svg id="m" viewBox="0 0 168.5 156" width="168.5" height="156"><g/></svg>"#
        );
        // Без viewBox — без изменений.
        assert_eq!(with_intrinsic_size("<svg><g/></svg>"), "<svg><g/></svg>");
    }

    /// Экспорт примеров в `target/export/` для просмотра в браузере:
    /// `cargo test export_examples -- --ignored`.
    #[test]
    #[ignore]
    fn export_examples() {
        let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/export");
        std::fs::create_dir_all(&out_dir).unwrap();
        let renderer = crate::diagram::MermanRenderer::new();
        let themes = [
            ("", PageTheme::Auto),
            ("-light", PageTheme::Light),
            ("-dark", PageTheme::Dark),
        ];
        for name in ["showcase", "mermaid"] {
            let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("examples/{name}.md"));
            let text = std::fs::read_to_string(&src).unwrap();
            for (suffix, theme) in themes {
                let out = to_html(&text, src.parent(), name, theme, &renderer);
                let file = format!("{name}{suffix}.html");
                println!("{file}: {} KB, {:?}", out.html.len() / 1024, out.warnings());
                std::fs::write(out_dir.join(file), out.html).unwrap();
            }
        }
    }
}
