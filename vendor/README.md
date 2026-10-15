# Пропатченные зависимости

Подключаются через `[patch.crates-io]` в корневом `Cargo.toml`. Версия копии должна
совпадать с той, что требуют зависимости, иначе патч молча не применится
(проверка: `cargo tree -i egui_commonmark` и `cargo tree -i egui_commonmark_backend` должны
показать пути `vendor/...`). Все изменения помечены комментарием `Markdowntown patch`.

## egui_commonmark_backend 0.25.0

Источник: crates.io, лицензия MIT OR Apache-2.0 (файлы лицензий в папке).

Изменения в `src/misc.rs`. `Style::to_richtext`: размеры заголовков H1–H6 считаются
от основного текста с коэффициентами GitHub (2.0 / 1.5 / 1.25 / 1.0 / 0.875 / 0.85),
все уровни выделены, H6 приглушён. В оригинале H1 (18pt, без выделения) выглядел не
крупнее H2 (17.1pt, выделенный).

Также `src/misc.rs`, `CommonMarkCache`: поле `block_positions` и методы
`block_positions()`, `clear_block_positions()`, `push_block_position()` — для патча ниже.

## egui_commonmark 0.25.0

Источник: crates.io, лицензия MIT OR Apache-2.0.

Изменение — только `src/parsers/pulldown.rs`: в `CommonMarkViewerInternal::show` перед
каждым блоком верхнего уровня в кэш записывается (смещение в исходнике, Y экрана) —
на этом строится синхронная прокрутка редактора и превью. Начала блоков считаются
отдельным проходом (`top_level_block_starts`), потому что `process_event` сам забирает
часть событий (таблицы, HTML).

## Обновление

При обновлении `egui_commonmark`: скопировать новые версии обоих крейтов, перенести патчи
(поиск по комментарию `Markdowntown patch`), либо удалить копию, если upstream исправил.
