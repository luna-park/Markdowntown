//! Меню правой кнопки в превью: «Копировать» выделенный текст.
//!
//! egui сбрасывает выделение текста в превью при нажатии любой кнопки мыши, и выбрать
//! пункт меню, не потеряв выделение, нельзя. Поэтому правый клик и клик по меню
//! перехватываются до egui (`filter_input` в `raw_input_hook`): выделение остаётся,
//! а «Копировать» — это событие `Event::Copy`, которое egui и так обрабатывает.
//! Состояние — во временных данных контекста: хуку и панели превью не нужно знать друг
//! о друге.

use egui::{Context, Event, Id, PointerButton, Pos2, Rect};

use crate::i18n::tr;

const STATE_ID: &str = "preview_menu_state";

#[derive(Clone)]
struct MenuState {
    /// Область превью в последнем показанном кадре и номер прохода, когда её запомнили.
    area: Option<(Rect, u64)>,
    /// Где открыто меню (`None` — закрыто).
    open: Option<Pos2>,
    /// Прямоугольник меню в последнем кадре — для клика.
    menu_rect: Rect,
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            area: None,
            open: None,
            menu_rect: Rect::NOTHING,
        }
    }
}

fn load(ctx: &Context) -> MenuState {
    ctx.data(|d| d.get_temp(Id::new(STATE_ID)))
        .unwrap_or_default()
}

fn store(ctx: &Context, state: MenuState) {
    ctx.data_mut(|d| d.insert_temp(Id::new(STATE_ID), state));
}

/// Превью показало себя в `area` (панель, без полосы прокрутки не обязательно).
pub fn note_area(ctx: &Context, area: Rect) {
    let mut state = load(ctx);
    state.area = Some((area, ctx.cumulative_pass_nr()));
    store(ctx, state);
}

fn has_selection(ctx: &Context) -> bool {
    ctx.with_plugin(|state: &mut egui::text_selection::LabelSelectionState| state.has_selection())
        .unwrap_or(false)
}

/// Для `eframe::App::raw_input_hook`: забирает правый клик в превью (открывает меню)
/// и клики по открытому меню, не пуская их к egui.
pub fn filter_input(ctx: &Context, input: &mut egui::RawInput) {
    let mut state = load(ctx);
    let was_open = state.open;
    // Превью, которого не было в последних кадрах (другой режим вида), не считается.
    let area = state
        .area
        .filter(|&(_, pass)| pass + 2 >= ctx.cumulative_pass_nr())
        .map(|(rect, _)| rect);
    if filter_events(&mut input.events, area, &mut state, has_selection(ctx))
        || was_open != state.open
    {
        ctx.request_repaint();
    }
    store(ctx, state);
}

/// Чистая часть `filter_input`. `true` — события изменились.
fn filter_events(
    events: &mut Vec<Event>,
    area: Option<Rect>,
    state: &mut MenuState,
    can_copy: bool,
) -> bool {
    let mut copy = false;
    let before = events.len();
    events.retain(|event| {
        let Event::PointerButton {
            pos,
            button,
            pressed,
            ..
        } = event
        else {
            return true;
        };
        match button {
            PointerButton::Secondary if area.is_some_and(|a| a.contains(*pos)) => {
                if *pressed {
                    state.open = Some(*pos);
                    state.menu_rect = Rect::from_min_size(*pos, egui::Vec2::ZERO);
                }
                false
            }
            PointerButton::Primary if state.open.is_some() && state.menu_rect.contains(*pos) => {
                // Нажатие и отпускание на меню egui не видит: иначе оно сбросило бы выделение.
                if *pressed && can_copy {
                    copy = true;
                    state.open = None;
                }
                false
            }
            _ if *pressed => {
                state.open = None; // клик в другом месте закрывает меню и идёт дальше
                true
            }
            _ => true,
        }
    });
    if copy {
        events.push(Event::Copy);
    }
    events.len() != before
}

/// Меню, если открыто. Вызывать из панели превью каждый кадр.
pub fn show(ctx: &Context) {
    let mut state = load(ctx);
    let Some(pos) = state.open else {
        return;
    };
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.open = None;
        store(ctx, state);
        return;
    }
    let can_copy = has_selection(ctx);
    let response = egui::Area::new(Id::new("preview_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_min_width(150.0);
                ui.add_enabled(
                    can_copy,
                    egui::Button::new(tr!("Copy", "Копировать")).shortcut_text("Ctrl+C"),
                );
            });
        });
    state.menu_rect = response.response.rect;
    store(ctx, state);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn button(pos: Pos2, button: PointerButton, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn area() -> Option<Rect> {
        Some(Rect::from_min_max(
            Pos2::new(100.0, 0.0),
            Pos2::new(500.0, 400.0),
        ))
    }

    #[test]
    fn right_click_in_preview_opens_menu_and_is_hidden_from_egui() {
        let mut state = MenuState::default();
        let at = Pos2::new(200.0, 50.0);
        let mut events = vec![
            button(at, PointerButton::Secondary, true),
            button(at, PointerButton::Secondary, false),
        ];
        assert!(filter_events(&mut events, area(), &mut state, true));
        assert!(events.is_empty());
        assert_eq!(state.open, Some(at));
    }

    #[test]
    fn right_click_outside_preview_passes_through() {
        let mut state = MenuState::default();
        let mut events = vec![button(
            Pos2::new(10.0, 10.0),
            PointerButton::Secondary,
            true,
        )];
        filter_events(&mut events, area(), &mut state, true);
        assert_eq!(events.len(), 1);
        assert_eq!(state.open, None);
    }

    #[test]
    fn click_on_menu_copies_and_closes() {
        let at = Pos2::new(200.0, 50.0);
        let mut state = MenuState {
            open: Some(at),
            menu_rect: Rect::from_min_size(at, egui::vec2(150.0, 30.0)),
            ..Default::default()
        };
        let inside = Pos2::new(210.0, 60.0);
        let mut events = vec![button(inside, PointerButton::Primary, true)];
        filter_events(&mut events, area(), &mut state, true);
        assert!(matches!(events.as_slice(), [Event::Copy]));
        assert_eq!(state.open, None);
    }

    #[test]
    fn click_on_disabled_menu_keeps_it_open() {
        let at = Pos2::new(200.0, 50.0);
        let mut state = MenuState {
            open: Some(at),
            menu_rect: Rect::from_min_size(at, egui::vec2(150.0, 30.0)),
            ..Default::default()
        };
        let mut events = vec![button(Pos2::new(210.0, 60.0), PointerButton::Primary, true)];
        filter_events(&mut events, area(), &mut state, false);
        assert!(events.is_empty());
        assert_eq!(state.open, Some(at));
    }

    #[test]
    fn click_elsewhere_closes_menu_and_passes_through() {
        let at = Pos2::new(200.0, 50.0);
        let mut state = MenuState {
            open: Some(at),
            menu_rect: Rect::from_min_size(at, egui::vec2(150.0, 30.0)),
            ..Default::default()
        };
        let mut events = vec![button(
            Pos2::new(400.0, 300.0),
            PointerButton::Primary,
            true,
        )];
        filter_events(&mut events, area(), &mut state, true);
        assert_eq!(events.len(), 1);
        assert_eq!(state.open, None);
    }
}
