# Исследование: рендер Mermaid без Node.js

Дата: 2026-10-15. Стенд: [experiments/mermaid/](../../experiments/mermaid/) (сравнение
рендереров, `src/bin/probe.rs` — углублённые проверки merman),
[experiments/size-probe/](../../experiments/size-probe/) (копия mdview (прежнее имя проекта) + merman для замера размера exe).

## Вопрос

Можно ли рендерить Mermaid внутри самодостаточного exe (без Node.js, Chromium и других
внешних программ) с качеством, сопоставимым с официальным `mermaid-cli`?

## Ответ

**Да.** Нативный Rust-крейт [`merman`](https://github.com/Latias94/merman) 0.9.0 даёт результат,
визуально почти совпадающий с эталоном `mmdc` (mermaid 12.2), на всех проверенных типах диаграмм,
в 300–500 раз быстрее и без внешних зависимостей. Его же использует редактор Zed
как Mermaid-бэкенд.

**Следствие для плана:** этап A (`mmdc`) не нужен — сразу делаем встроенный рендер на `merman`.
Вариант «JS-движок + mermaid.js» не понадобился.

## Кандидаты

| Крейт | Версия | Лицензия | Итог |
|---|---|---|---|
| [`merman`](https://github.com/Latias94/merman) | 0.9.0 | MIT OR Apache-2.0 | ✅ **выбран** |
| [`mermaid-svg`](https://github.com/xmiksay/mermaid) | 0.7.0 | MIT | запасной вариант: быстрый и лёгкий, но раскладка заметно хуже |
| [`mermaid-rs-renderer`](https://github.com/1jehuang/mermaid-rs-renderer) (mmdr) | 0.3.1 | MIT | ❌ не разбирает `<<enumeration>>` в class; ломает порядок в sequence |
| [`mermaid-little`](https://github.com/Actrium/supramark) | 11.14.0-6 | MIT | ❌ портит кириллицу (mojibake в pie); подписи через `<foreignObject>` теряются в resvg |
| [`mermaid-render`](https://github.com/alitrack/mermaid-render) | 0.10.0 | MIT | ❌ пустые gantt и pie, слабый sequence; отдаёт SVG без корня |
| `flowmaid` | 0.27.0 | GPL-3.0 | ❌ исключён по лицензии |
| `rquickjs` + `mermaid.js` | — | — | не понадобился (см. ниже) |

## Методика

- 8 тестовых диаграмм с кириллицей: flowchart TD, sequence, class, state (с вложенным состоянием),
  ER, gantt, pie, flowchart LR со всеми основными формами узлов —
  [experiments/mermaid/diagrams/](../../experiments/mermaid/diagrams/).
- Эталон: `mmdc` 12.0.0 (mermaid 12.2.0) → PNG.
- Каждый движок: SVG → `resvg` 0.48 (системные шрифты) → PNG. Время — среднее из 10 прогонов
  в release-сборке.
- Сводные листы «эталон + все движки» — `experiments/mermaid/out/sheets/*.png`
  (генерируются `cargo run --release` в `experiments/mermaid`).

## Результаты

### Качество (визуальное сравнение с эталоном)

| Диаграмма | merman | mermaid-svg | mmdr | mermaid-little | mermaid-render |
|---|---|---|---|---|---|
| flowchart TD + subgraph | ≈ эталон | ок, другая раскладка | ок | ❌ нет подписей | ок, ломаные линии |
| sequence + note + alt | ≈ эталон | ок | ❌ блоки не на месте | ок | ❌ мелко, обрезано |
| class + enumeration | ≈ эталон | ок | ❌ ошибка разбора | ❌ нет подписей | ❌ потеряны связи |
| state + composite | ≈ эталон | ❌ вложенное состояние вынесено | ок | ❌ нет подписей | ❌ спутанные линии |
| ER | ≈ эталон | ≈ эталон | ок, свой стиль | ❌ обрезано | ❌ мелко |
| gantt | ≈ эталон | ок | ок, свой стиль | ок, мелко | ❌ пусто |
| pie | ≈ эталон | ≈ эталон | ок, другие цвета | ❌ кириллица в легенде | ❌ пусто |
| flowchart LR, формы | ≈ эталон | ок, мелко | ❌ перестроено в 2 ряда | ❌ нет подписей | ок |

### Скорость (мс на диаграмму, release)

| Диаграмма | merman | mermaid-svg | mmdr | `mmdc` (эталон) |
|---|---|---|---|---|
| flowchart TD | 3.7 | 0.06 | 28.3 | ~1640 (запуск Chromium) |
| sequence | 1.5 | 0.04 | 0.3 | |
| class | 3.6 | 0.04 | — | |
| state | 4.0 | 0.06 | 1.7 | |
| ER | 2.9 | 0.04 | 1.8 | |
| gantt | 0.6 | 0.04 | 0.1 | |
| pie | 0.3 | 0.01 | 0.05 | |

### merman: углублённые проверки

- **Холодный старт** (`Renderer::new()` + первый рендер): ~8 мс. Тёплый рендер — ~4 мс.
- **Большие диаграммы:** flowchart на 200 узлов / ~267 рёбер — ~130 мс; sequence на 150
  сообщений — ~6 мс. Для живого превью с debounce этого достаточно; рендер всё равно
  выносим в фоновый поток.
- **`Renderer: Send + Sync`** — можно держать один экземпляр в фоновом потоке.
- **Ошибки синтаксиса** (пустой ввод, неизвестный тип, недописанное ребро, незакрытая скобка,
  мусор, недописанный класс): во всех случаях `Err` с понятным сообщением, **без паник**.
  Пример: `Diagram parse error (flowchart-v2): Unterminated node label (missing ']')`.
- **Тёмная тема:** конфиг хоста `{"theme": "dark"}` через `Engine::with_site_config` —
  результат контрастный и читаемый (`out/probe/dark_*.png`).
- **resvg-совместимость:** `SvgPipeline::resvg_safe()` убирает `<foreignObject>` —
  подписи не теряются, флаг `htmlLabels: false` не нужен.
- **Зависимости:** ~215 крейтов; нет OpenSSL, сети, JS-движков. Лицензии — MIT/Apache/Zlib/
  Unicode/BSD; под **MPL-2.0** только `cssparser`, `selectors`, `dtoa-short` (экосистема Servo;
  файловый copyleft — при использовании без изменений достаточно уведомления).
  ELK-раскладку (EPL-2.0) не включаем: она в отдельной feature `layout-elk`.

### Размер exe

Release, LTO + `codegen-units = 1` + `strip`, статический CRT:

| Сборка | Размер | Прирост |
|---|---|---|
| Markdowntown (eframe + egui_commonmark + rfd) | 10.7 МБ | — |
| + merman (все диаграммы, `complete-svg`) + resvg | 28.2 МБ | +17.5 МБ |
| + merman (11 основных диаграмм, только `svg`) + resvg | 21.0 МБ | +10.3 МБ |

Откуда размер (`cargo bloat --crates`, полная сборка без LTO, секция `.text` — 23.5 из 30.5 МБ):

| Крейт | `.text` |
|---|---|
| `merman_render` | 6.1 МБ |
| `merman_core` | 3.8 МБ |
| `naga`, `wgpu_core`, `wgpu_hal` (уже есть в Markdowntown) | 3.2 МБ |
| `dugong` (раскладка), `ratex_parser`/`ratex_layout` (формулы), `vello_cpu`, `manatee` | ~1.4 МБ |
| `usvg`, `resvg`, `tiny_skia`, `skrifa`, `harfrust` (SVG и текст) | ~1.8 МБ |

Выводы:

- Основной прирост — **собственный код `merman`** (~10 МБ, по сути реализация всех
  парсеров и раскладок). Его не убрать, только сократить набор диаграмм.
- Разница «полный набор → 11 диаграмм» (~7 МБ) — это feature `math` (шрифты KaTeX через
  `ratex-katex-fonts` + `ratex_*`), `layout-cytoscape` и редкие типы диаграмм.
  Формулы в диаграммах нам не нужны → `math` не включаем.

### Почему не понадобился JS-движок

Вариант «`rquickjs` + вшитый `mermaid.js`» требует: бандл `mermaid.min.js` 12.2 — 5.2 МБ;
эмуляцию DOM и, главное, измерения текста (`getBBox`), без которых mermaid не может разложить
диаграмму. Это самая рискованная часть, и она решает ту же задачу хуже, чем `merman`,
который уже повторяет раскладку mermaid на Rust. Возвращаться к варианту имеет смысл,
только если `merman` перестанут поддерживать.

## Рекомендация

1. Рендерить Mermaid через **`merman`** с `default-features = false` и features
   `all-diagrams` + `complete-svg` (все типы диаграмм, cytoscape, формулы). Без `layout-elk`.
   _Обновлено после исследования:_ бюджет на размер exe — до ~100 МБ, приоритет — функционал,
   поэтому +17.5 МБ за полный набор приемлемы. Урезанный вариант (`svg` + список `diagram-*`,
   +10.3 МБ) остаётся на случай, если размер станет важен.
2. Растеризация — **`resvg`** (уже в дереве через `egui_extras`/`merman`), SVG только через
   `SvgPipeline::resvg_safe()`.
3. Рендерер прятать за трейтом (`DiagramRenderer`), чтобы `merman` можно было заменить
   (`mermaid-svg` — запасной вариант).
4. **Этап A (`mmdc`) из плана убрать.** Node.js не нужен даже в разработке.
5. Тема: передавать `theme: "dark" | "default"` по текущей теме egui; ключ кэша — хэш
   (исходник + тема + ширина).
6. Риски:
   - `merman` — 0.x, API может меняться между минорными версиями → фиксировать версию,
     обновлять осознанно (у проекта есть upgrade guide).
   - Прирост exe +17.5 МБ (полный набор): в рамках бюджета ~100 МБ.
   - Шрифты: `resvg` берёт системные шрифты; для детерминированности можно вшить один шрифт
     (+0.5–1 МБ).

## Источники

- merman: https://github.com/Latias94/merman, интеграция в Zed: https://github.com/zed-industries/zed/pull/57644
- mermaid-rs-renderer: https://github.com/1jehuang/mermaid-rs-renderer
- mermaid-svg: https://github.com/xmiksay/mermaid
- mermaid-little: https://github.com/Actrium/supramark
- mermaid-render: https://github.com/alitrack/mermaid-render
- mermaid-to-svg (только flowchart, не тестировался): https://github.com/warpdotdev/mermaid-to-svg
