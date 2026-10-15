//! Синхронная прокрутка редактора и превью.
//!
//! Ведущей считается панель, которую прокрутил пользователь (её смещение изменилось
//! не нашей командой); ведомая каждый кадр подстраивается под неё. Сопоставление —
//! через байт документа в верхней строке: в редакторе по раскладке текста, в превью
//! по якорям блоков с линейной интерполяцией между ними.

use crate::editor::EditorGeometry;

/// Якоря превью: (байт документа, Y в координатах содержимого), по возрастанию.
pub struct PreviewGeometry {
    pub anchors: Vec<(usize, f32)>,
    pub offset: f32,
    pub max_offset: f32,
    pub content_height: f32,
}

impl PreviewGeometry {
    pub fn y_for_byte(&self, byte: usize, text_len: usize) -> f32 {
        let (a, b) = self.bracket(|(pos, _)| *pos <= byte, text_len);
        lerp_by(byte as f32, (a.0 as f32, a.1), (b.0 as f32, b.1))
    }

    pub fn byte_for_y(&self, y: f32, text_len: usize) -> usize {
        let (a, b) = self.bracket(|(_, top)| *top <= y, text_len);
        lerp_by(y, (a.1, a.0 as f32), (b.1, b.0 as f32))
            .round()
            .max(0.0) as usize
    }

    /// Соседние якоря вокруг точки: последний, для которого `before` истинно, и следующий.
    /// Края — начало документа и конец содержимого.
    fn bracket(
        &self,
        before: impl Fn(&(usize, f32)) -> bool,
        text_len: usize,
    ) -> ((usize, f32), (usize, f32)) {
        let start = (0, 0.0);
        let end = (text_len, self.content_height);
        let i = self.anchors.partition_point(&before);
        let a = if i == 0 { start } else { self.anchors[i - 1] };
        let b = self.anchors.get(i).copied().unwrap_or(end);
        (a, b)
    }
}

/// Линейная интерполяция `x` из отрезка `a.0..b.0` в `a.1..b.1`.
fn lerp_by(x: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    if b.0 <= a.0 {
        return a.1;
    }
    let t = ((x - a.0) / (b.0 - a.0)).clamp(0.0, 1.0);
    a.1 + (b.1 - a.1) * t
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pane {
    Editor,
    Preview,
}

pub struct ScrollSync {
    pub enabled: bool,
    driver: Pane,
    /// Смещения, которые панели должны иметь после нашей последней команды.
    expected_editor: f32,
    expected_preview: f32,
}

impl Default for ScrollSync {
    fn default() -> Self {
        Self {
            enabled: true,
            driver: Pane::Editor,
            expected_editor: 0.0,
            expected_preview: 0.0,
        }
    }
}

/// Разница смещений, которую считаем движением (а не погрешностью округления).
const EPS: f32 = 1.0;

impl ScrollSync {
    /// Возвращает новые смещения (редактор, превью) для следующего кадра.
    pub fn update(
        &mut self,
        text: &str,
        editor: &EditorGeometry,
        preview: &PreviewGeometry,
    ) -> (Option<f32>, Option<f32>) {
        if !self.enabled {
            self.expected_editor = editor.offset;
            self.expected_preview = preview.offset;
            return (None, None);
        }
        if (editor.offset - self.expected_editor).abs() > EPS {
            self.driver = Pane::Editor;
        } else if (preview.offset - self.expected_preview).abs() > EPS {
            self.driver = Pane::Preview;
        }
        self.expected_editor = editor.offset;
        self.expected_preview = preview.offset;

        match self.driver {
            Pane::Editor => {
                let target = if editor.offset >= editor.max_offset - EPS && editor.max_offset > 0.0
                {
                    preview.max_offset
                } else {
                    let byte = editor.byte_at_offset(text, editor.offset);
                    preview.y_for_byte(byte, text.len())
                }
                .clamp(0.0, preview.max_offset);
                if (target - preview.offset).abs() > EPS {
                    self.expected_preview = target;
                    return (None, Some(target));
                }
            }
            Pane::Preview => {
                let target =
                    if preview.offset >= preview.max_offset - EPS && preview.max_offset > 0.0 {
                        editor.max_offset
                    } else {
                        let byte = preview.byte_for_y(preview.offset, text.len());
                        editor.offset_for_byte(text, byte)
                    }
                    .clamp(0.0, editor.max_offset);
                if (target - editor.offset).abs() > EPS {
                    self.expected_editor = target;
                    return (Some(target), None);
                }
            }
        }
        (None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry() -> PreviewGeometry {
        PreviewGeometry {
            anchors: vec![(0, 0.0), (100, 50.0), (200, 400.0)],
            offset: 0.0,
            max_offset: 300.0,
            content_height: 500.0,
        }
    }

    #[test]
    fn interpolates_between_anchors() {
        let g = geometry();
        assert_eq!(g.y_for_byte(0, 300), 0.0);
        assert_eq!(g.y_for_byte(50, 300), 25.0);
        assert_eq!(g.y_for_byte(150, 300), 225.0);
        // После последнего якоря — к концу содержимого.
        assert_eq!(g.y_for_byte(250, 300), 450.0);
    }

    #[test]
    fn inverse_mapping() {
        let g = geometry();
        assert_eq!(g.byte_for_y(25.0, 300), 50);
        assert_eq!(g.byte_for_y(225.0, 300), 150);
        assert_eq!(g.byte_for_y(500.0, 300), 300);
    }

    #[test]
    fn empty_anchors_map_linearly() {
        let g = PreviewGeometry {
            anchors: vec![],
            offset: 0.0,
            max_offset: 0.0,
            content_height: 100.0,
        };
        assert_eq!(g.y_for_byte(50, 100), 50.0);
    }
}
