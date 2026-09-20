//! Terminal events → actions.

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Size;

use crate::ui::app::{Column, Mode};
use crate::ui::columns;

const WHEEL_LINES: isize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Line(isize),
    HalfPage(isize),
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyTarget {
    Message,
    Session,
    Resume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Move(Motion),
    Focus { forward: bool },
    Descend,
    Ascend,
    ToggleFocusMode,
    NextCall { forward: bool },
    NextTurn { forward: bool },
    ToggleCall,
    ToggleAllCalls,
    CycleBranch,
    ToggleInjections,
    ToggleDiagnostics,
    ToggleSearch,
    ToggleFilter,
    Copy(CopyTarget),
    ToggleExport,
    CycleExportFormat,
    Type(char),
    Untype,
    Resize(Size),
    Scroll { column: Column, delta: isize },
    Click { column: Column, row: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub area: Size,
    pub mode: Mode,
    pub text_entry: bool,
}

pub fn action(event: &Event, viewport: Viewport) -> Option<Action> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => key_action(*key, viewport.text_entry),
        Event::Resize(columns, rows) => Some(Action::Resize(Size::new(*columns, *rows))),
        Event::Mouse(mouse) => mouse_action(*mouse, viewport),
        _ => None,
    }
}

fn mouse_action(mouse: MouseEvent, viewport: Viewport) -> Option<Action> {
    let hit = columns::hit(viewport.area, viewport.mode, mouse.column, mouse.row);
    match mouse.kind {
        MouseEventKind::ScrollDown => hit.map(|hit| Action::Scroll { column: hit.column, delta: WHEEL_LINES }),
        MouseEventKind::ScrollUp => hit.map(|hit| Action::Scroll { column: hit.column, delta: -WHEEL_LINES }),
        MouseEventKind::Down(MouseButton::Left) => {
            hit.and_then(|hit| Some(Action::Click { column: hit.column, row: u16::try_from(hit.row).ok()? }))
        }
        _ => None,
    }
}

fn key_action(key: KeyEvent, text_entry: bool) -> Option<Action> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('d') => Some(Action::Move(Motion::HalfPage(1))),
            KeyCode::Char('u') => Some(Action::Move(Motion::HalfPage(-1))),
            KeyCode::Char('c') => Some(Action::Quit),
            _ => None,
        };
    }
    if key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SUPER) {
        return None;
    }

    if text_entry {
        return text_entry_action(key);
    }

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => Some(Action::Move(Motion::Line(1))),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::Move(Motion::Line(-1))),
        KeyCode::Char('d') => Some(Action::Move(Motion::HalfPage(1))),
        KeyCode::Char('u') => Some(Action::Move(Motion::HalfPage(-1))),
        KeyCode::Char('g') | KeyCode::Home => Some(Action::Move(Motion::Top)),
        KeyCode::Char('G') | KeyCode::End => Some(Action::Move(Motion::Bottom)),
        KeyCode::Char('h') | KeyCode::Left | KeyCode::BackTab => Some(Action::Focus { forward: false }),
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Tab => Some(Action::Focus { forward: true }),
        KeyCode::Enter => Some(Action::Descend),
        KeyCode::Esc => Some(Action::Ascend),
        KeyCode::Char('f') => Some(Action::ToggleFocusMode),
        KeyCode::Char('n') => Some(Action::NextCall { forward: true }),
        KeyCode::Char('p' | 'N') => Some(Action::NextCall { forward: false }),
        KeyCode::Char(']') => Some(Action::NextTurn { forward: true }),
        KeyCode::Char('[') => Some(Action::NextTurn { forward: false }),
        KeyCode::Char(' ') => Some(Action::ToggleCall),
        KeyCode::Char('t') => Some(Action::ToggleAllCalls),
        KeyCode::Char('b') => Some(Action::CycleBranch),
        KeyCode::Char('i') => Some(Action::ToggleInjections),
        KeyCode::Char('D') => Some(Action::ToggleDiagnostics),
        KeyCode::Char('?') => Some(Action::ToggleSearch),
        KeyCode::Char('/') => Some(Action::ToggleFilter),
        KeyCode::Char('y') => Some(Action::Copy(CopyTarget::Message)),
        KeyCode::Char('Y') => Some(Action::Copy(CopyTarget::Session)),
        KeyCode::Char('c') => Some(Action::Copy(CopyTarget::Resume)),
        KeyCode::Char('e') => Some(Action::ToggleExport),
        KeyCode::Char('q') => Some(Action::Quit),
        _ => None,
    }
}

const fn text_entry_action(key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Char(character) => Some(Action::Type(character)),
        KeyCode::Backspace => Some(Action::Untype),
        KeyCode::Down => Some(Action::Move(Motion::Line(1))),
        KeyCode::Up => Some(Action::Move(Motion::Line(-1))),
        KeyCode::Tab => Some(Action::CycleExportFormat),
        KeyCode::Enter => Some(Action::Descend),
        KeyCode::Esc => Some(Action::Ascend),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn control(code: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(code), KeyModifiers::CONTROL))
    }

    fn viewport() -> Viewport {
        Viewport { area: Size::new(120, 24), mode: Mode::Browse, text_entry: false }
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE })
    }

    #[test]
    fn every_documented_key_maps_to_its_action() {
        let table = [
            (press(KeyCode::Char('j')), Action::Move(Motion::Line(1))),
            (press(KeyCode::Down), Action::Move(Motion::Line(1))),
            (press(KeyCode::Char('k')), Action::Move(Motion::Line(-1))),
            (press(KeyCode::Up), Action::Move(Motion::Line(-1))),
            (press(KeyCode::Char('g')), Action::Move(Motion::Top)),
            (press(KeyCode::Home), Action::Move(Motion::Top)),
            (press(KeyCode::Char('G')), Action::Move(Motion::Bottom)),
            (press(KeyCode::End), Action::Move(Motion::Bottom)),
            (press(KeyCode::Char('d')), Action::Move(Motion::HalfPage(1))),
            (press(KeyCode::Char('u')), Action::Move(Motion::HalfPage(-1))),
            (control('d'), Action::Move(Motion::HalfPage(1))),
            (control('u'), Action::Move(Motion::HalfPage(-1))),
            (press(KeyCode::Char('h')), Action::Focus { forward: false }),
            (press(KeyCode::Left), Action::Focus { forward: false }),
            (press(KeyCode::BackTab), Action::Focus { forward: false }),
            (press(KeyCode::Char('l')), Action::Focus { forward: true }),
            (press(KeyCode::Right), Action::Focus { forward: true }),
            (press(KeyCode::Tab), Action::Focus { forward: true }),
            (press(KeyCode::Enter), Action::Descend),
            (press(KeyCode::Esc), Action::Ascend),
            (press(KeyCode::Char('f')), Action::ToggleFocusMode),
            (press(KeyCode::Char('n')), Action::NextCall { forward: true }),
            (press(KeyCode::Char('p')), Action::NextCall { forward: false }),
            (press(KeyCode::Char('N')), Action::NextCall { forward: false }),
            (press(KeyCode::Char(']')), Action::NextTurn { forward: true }),
            (press(KeyCode::Char('[')), Action::NextTurn { forward: false }),
            (press(KeyCode::Char(' ')), Action::ToggleCall),
            (press(KeyCode::Char('t')), Action::ToggleAllCalls),
            (press(KeyCode::Char('b')), Action::CycleBranch),
            (press(KeyCode::Char('i')), Action::ToggleInjections),
            (press(KeyCode::Char('D')), Action::ToggleDiagnostics),
            (press(KeyCode::Char('?')), Action::ToggleSearch),
            (press(KeyCode::Char('/')), Action::ToggleFilter),
            (press(KeyCode::Char('y')), Action::Copy(CopyTarget::Message)),
            (press(KeyCode::Char('Y')), Action::Copy(CopyTarget::Session)),
            (press(KeyCode::Char('c')), Action::Copy(CopyTarget::Resume)),
            (press(KeyCode::Char('e')), Action::ToggleExport),
            (press(KeyCode::Char('q')), Action::Quit),
            (control('c'), Action::Quit),
        ];
        for (event, expected) in table {
            assert_eq!(action(&event, viewport()), Some(expected), "{event:?}");
        }
    }

    fn text_viewport() -> Viewport {
        Viewport { area: Size::new(120, 24), mode: Mode::Browse, text_entry: true }
    }

    #[test]
    fn in_text_entry_mode_a_letter_that_is_normally_bound_types_instead() {
        assert_eq!(action(&press(KeyCode::Char('n')), text_viewport()), Some(Action::Type('n')));
        assert_eq!(action(&press(KeyCode::Char('j')), text_viewport()), Some(Action::Type('j')));
        assert_eq!(action(&press(KeyCode::Char('?')), text_viewport()), Some(Action::Type('?')));
        assert_eq!(action(&press(KeyCode::Char('y')), text_viewport()), Some(Action::Type('y')));
        assert_eq!(action(&press(KeyCode::Char('c')), text_viewport()), Some(Action::Type('c')));
    }

    #[test]
    fn ctrl_c_still_quits_even_though_c_alone_now_copies() {
        assert_eq!(action(&control('c'), viewport()), Some(Action::Quit));
        assert_eq!(action(&press(KeyCode::Char('c')), viewport()), Some(Action::Copy(CopyTarget::Resume)));
    }

    #[test]
    fn in_text_entry_mode_backspace_untypes_and_arrows_still_move() {
        assert_eq!(action(&press(KeyCode::Backspace), text_viewport()), Some(Action::Untype));
        assert_eq!(action(&press(KeyCode::Down), text_viewport()), Some(Action::Move(Motion::Line(1))));
        assert_eq!(action(&press(KeyCode::Up), text_viewport()), Some(Action::Move(Motion::Line(-1))));
    }

    #[test]
    fn in_text_entry_mode_tab_cycles_the_export_format_rather_than_moving_focus() {
        assert_eq!(action(&press(KeyCode::Tab), text_viewport()), Some(Action::CycleExportFormat));
    }

    #[test]
    fn in_text_entry_mode_enter_and_escape_keep_their_meaning() {
        assert_eq!(action(&press(KeyCode::Enter), text_viewport()), Some(Action::Descend));
        assert_eq!(action(&press(KeyCode::Esc), text_viewport()), Some(Action::Ascend));
    }

    #[test]
    fn in_text_entry_mode_a_control_binding_still_reaches_the_shell() {
        assert_eq!(action(&control('c'), text_viewport()), Some(Action::Quit));
    }

    #[test]
    fn a_shifted_capital_still_reaches_its_binding() {
        let shifted = Event::Key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT));
        assert_eq!(action(&shifted, viewport()), Some(Action::Move(Motion::Bottom)));
    }

    #[test]
    fn a_release_is_not_a_second_press() {
        let mut key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        assert_eq!(action(&Event::Key(key), viewport()), None);
    }

    #[test]
    fn a_control_binding_does_not_answer_to_its_bare_letter_twice_over() {
        assert_eq!(action(&control('q'), viewport()), None);
        assert_eq!(action(&control('f'), viewport()), None);
    }

    #[test]
    fn a_half_page_answers_to_both_of_its_spellings() {
        assert_eq!(action(&press(KeyCode::Char('d')), viewport()), action(&control('d'), viewport()));
        assert_eq!(action(&press(KeyCode::Char('u')), viewport()), action(&control('u'), viewport()));
    }

    #[test]
    fn the_diagnostics_answer_to_the_capital_and_never_to_the_letter_that_now_pages() {
        assert_eq!(action(&press(KeyCode::Char('D')), viewport()), Some(Action::ToggleDiagnostics));
        assert_eq!(action(&press(KeyCode::Char('d')), viewport()), Some(Action::Move(Motion::HalfPage(1))));
    }

    #[test]
    fn alt_disqualifies_a_key() {
        let alt = Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
        assert_eq!(action(&alt, viewport()), None);
    }

    #[test]
    fn a_resize_reaches_the_shell() {
        assert_eq!(action(&Event::Resize(80, 24), viewport()), Some(Action::Resize(Size::new(80, 24))));
    }

    #[test]
    fn an_unbound_key_is_ignored_rather_than_guessed_at() {
        assert_eq!(action(&press(KeyCode::Char('z')), viewport()), None);
        assert_eq!(action(&press(KeyCode::Insert), viewport()), None);
    }

    #[test]
    fn capital_n_steps_backward_the_same_as_p() {
        assert_eq!(action(&press(KeyCode::Char('N')), viewport()), action(&press(KeyCode::Char('p')), viewport()));
    }

    #[test]
    fn the_wheel_scrolls_the_column_under_the_pointer() {
        assert_eq!(
            action(&mouse(MouseEventKind::ScrollDown, 5, 5), viewport()),
            Some(Action::Scroll { column: Column::Projects, delta: WHEEL_LINES })
        );
        assert_eq!(
            action(&mouse(MouseEventKind::ScrollUp, 5, 5), viewport()),
            Some(Action::Scroll { column: Column::Projects, delta: -WHEEL_LINES })
        );
    }

    #[test]
    fn a_left_click_resolves_to_the_column_and_row_under_the_pointer() {
        assert_eq!(
            action(&mouse(MouseEventKind::Down(MouseButton::Left), 5, 5), viewport()),
            Some(Action::Click { column: Column::Projects, row: 2 })
        );
    }

    #[test]
    fn a_click_or_scroll_outside_any_column_does_nothing() {
        assert_eq!(action(&mouse(MouseEventKind::Down(MouseButton::Left), 5, 0), viewport()), None);
        assert_eq!(action(&mouse(MouseEventKind::ScrollDown, 5, 0), viewport()), None);
    }

    #[test]
    fn the_right_and_middle_buttons_are_ignored() {
        assert_eq!(action(&mouse(MouseEventKind::Down(MouseButton::Right), 5, 5), viewport()), None);
        assert_eq!(action(&mouse(MouseEventKind::Down(MouseButton::Middle), 5, 5), viewport()), None);
        assert_eq!(action(&mouse(MouseEventKind::Up(MouseButton::Left), 5, 5), viewport()), None);
        assert_eq!(action(&mouse(MouseEventKind::Drag(MouseButton::Left), 5, 5), viewport()), None);
        assert_eq!(action(&mouse(MouseEventKind::Moved, 5, 5), viewport()), None);
    }

    #[test]
    fn a_click_in_focus_mode_always_hits_the_conversation() {
        let focused = Viewport { area: Size::new(120, 24), mode: Mode::Focus, text_entry: false };
        assert_eq!(
            action(&mouse(MouseEventKind::Down(MouseButton::Left), 5, 5), focused),
            Some(Action::Click { column: Column::Conversation, row: 2 })
        );
    }
}
