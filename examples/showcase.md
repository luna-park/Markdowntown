# Showcase: all supported Markdown elements

This document is used to check the preview manually. Open it with
`cargo run -- examples/showcase.md`.

## Headings

# H1
## H2
### H3
#### H4
##### H5
###### H6

## Inline formatting

Plain text, **bold**, *italic*, ***bold italic***, ~~strikethrough~~, `inline code`,
and a [link to CommonMark](https://commonmark.org). Autolink: <https://github.com>.

Кириллица: **жирный**, *курсив*, `код`, ~~зачёркнутый~~.

Line with a hard break at the end  
and the next line.

## Lists

- Unordered item
- Another item
  - Nested item
    - Deeper nested item
- Back to the first level

1. Ordered item
2. Second item
   1. Nested ordered
   2. Another nested
3. Third item

### Task list

- [x] Done task
- [ ] Open task (click the checkbox in the preview — the source must change)
- [ ] Another open task

## Blockquotes

> A simple quote.
>
> > Nested quote.

## Alerts (GFM)

> [!NOTE]
> Useful information.

> [!TIP]
> A helpful hint.

> [!IMPORTANT]
> Key information.

> [!WARNING]
> Needs attention.

> [!CAUTION]
> Possible negative consequences.

## Code

```rust
use std::collections::HashMap;

fn main() {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for word in "a b a c b a".split_whitespace() {
        *counts.entry(word).or_default() += 1;
    }
    println!("{counts:?}");
}
```

```python
def greet(name: str) -> str:
    return f"Привет, {name}!"
```

```
Code block without a language.
```

## Tables

| Left | Center | Right |
|:-----|:------:|------:|
| a    | b      | 1     |
| long cell | **bold** | 22 |
| `code` | кириллица | 333 |

## Horizontal rule

Above the rule.

---

Below the rule.

## Footnotes

Text with a footnote[^1] and another one[^note].

[^1]: The first footnote.
[^note]: A named footnote.

## Definition list

Term
: Definition of the term.

## Images

Local SVG (path relative to this file):

![mdview logo](images/logo.svg)

Remote image:

![Rust logo](https://www.rust-lang.org/logos/rust-logo-128x128.png)

## Mermaid

```mermaid
flowchart TD
    A[Start] --> B{Is it working?}
    B -->|Yes| C[Great]
    B -->|No| D[Debug]
    D --> B
```

Text between two diagrams.

```mermaid
sequenceDiagram
    Editor->>Preview: text changed
    Preview-->>Editor: rendered
```

Mermaid inside a list stays a regular code block:

- Item
  ```mermaid
  pie
      "A" : 1
  ```

The end.
