# Mermaid diagram types

Every diagram type that is checked manually. Open with `cargo run -- examples/mermaid.md`.

## flowchart

```mermaid
flowchart TD
    A[Начало] --> B{Файл открыт?}
    B -->|Да| C[Показать превью]
    B -->|Нет| D[Диалог открытия]
    D --> C
    C --> E((Конец))
    subgraph Рендер
        C
        E
    end
```

## sequence

```mermaid
sequenceDiagram
    participant U as Пользователь
    participant E as Редактор
    participant R as Renderer
    U->>E: Ввод текста
    E->>R: render(source)
    activate R
    R-->>E: SVG
    deactivate R
    Note over E,R: кэш по хэшу
    alt ошибка
        R-->>E: Err
    end
```

## class

```mermaid
classDiagram
    class Document {
        +String text
        +Option~PathBuf~ path
        +bool dirty
        +save() Result
    }
    class Segment {
        <<enumeration>>
        Markdown
        Mermaid
    }
    Document "1" --> "*" Segment : содержит
    Renderer <|-- MmdcRenderer
    Renderer <|-- NativeRenderer
```

## state

```mermaid
stateDiagram-v2
    [*] --> Чисто
    Чисто --> Изменён : ввод
    Изменён --> Сохранение : Ctrl+S
    Сохранение --> Чисто : ok
    Сохранение --> Ошибка : err
    Ошибка --> Изменён
    state Сохранение {
        [*] --> Запись
        Запись --> [*]
    }
```

## er

```mermaid
erDiagram
    USER ||--o{ DOCUMENT : владеет
    DOCUMENT ||--|{ SEGMENT : состоит
    DOCUMENT {
        string path PK
        int size
        bool dirty
    }
    SEGMENT {
        int line_start
        string kind
    }
```

## gantt

```mermaid
gantt
    title План mdview
    dateFormat YYYY-MM-DD
    section Основа
    Каркас          :done, a1, 2026-10-01, 5d
    Markdown GFM    :active, a2, after a1, 10d
    section Mermaid
    Исследование    :b1, after a1, 4d
    Интеграция      :b2, after b1, 8d
    Релиз 1.0       :milestone, m1, 2026-12-01, 0d
```

## pie

```mermaid
pie title Время на задачи
    "Редактор" : 35
    "Превью" : 25
    "Mermaid" : 30
    "Релиз" : 10
```

## mindmap

```mermaid
mindmap
  root((mdview))
    Editor
      Highlighting
      Search
    Preview
      Markdown
      Mermaid
    Release
      Portable exe
```

## timeline

```mermaid
timeline
    title mdview history
    2026-10 : Skeleton : Research
    2026-11 : Mermaid : Editor
    2026-12 : Release 1.0
```

## gitGraph

```mermaid
gitGraph
    commit id: "init"
    branch feature
    commit id: "mermaid"
    checkout main
    commit id: "fix"
    merge feature
    commit id: "release"
```

## journey

```mermaid
journey
    title Writing a document
    section Edit
      Open file: 5: User
      Type text: 4: User
    section Review
      Check preview: 5: User
      Fix diagram: 2: User
```

## Syntax error (must show a message, not crash)

```mermaid
flowchart TD
    A[Unclosed --> B
```
