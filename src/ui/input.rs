//! Terminal events → actions.

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Size;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Line(isize),
    HalfPage(isize),
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Move(Motion),
    Focus { forward: bool },
    Descend,
    Ascend,
    ToggleFocusMode,
    Resize(Size),
}

pub fn action(event: &Event) -> Option<Action> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => key_action(*key),
        Event::Resize(columns, rows) => Some(Action::Resize(Size::new(*columns, *rows))),
        _ => None,
    }
}

fn key_action(key: KeyEvent) -> Option<Action> {
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

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => Some(Action::Move(Motion::Line(1))),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::Move(Motion::Line(-1))),
        KeyCode::Char('g') | KeyCode::Home => Some(Action::Move(Motion::Top)),
        KeyCode::Char('G') | KeyCode::End => Some(Action::Move(Motion::Bottom)),
        KeyCode::Char('h') | KeyCode::Left | KeyCode::BackTab => Some(Action::Focus { forward: false }),
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Tab => Some(Action::Focus { forward: true }),
        KeyCode::Enter => Some(Action::Descend),
        KeyCode::Esc => Some(Action::Ascend),
        KeyCode::Char('f') => Some(Action::ToggleFocusMode),
        KeyCode::Char('q') => Some(Action::Quit),
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
            (press(KeyCode::Char('q')), Action::Quit),
            (control('c'), Action::Quit),
        ];
        for (event, expected) in table {
            assert_eq!(action(&event), Some(expected), "{event:?}");
        }
    }

    #[test]
    fn a_shifted_capital_still_reaches_its_binding() {
        let shifted = Event::Key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT));
        assert_eq!(action(&shifted), Some(Action::Move(Motion::Bottom)));
    }

    #[test]
    fn a_release_is_not_a_second_press() {
        let mut key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        assert_eq!(action(&Event::Key(key)), None);
    }

    #[test]
    fn a_control_binding_does_not_answer_to_its_bare_letter_twice_over() {
        assert_eq!(action(&control('q')), None);
        assert_eq!(action(&control('f')), None);
    }

    #[test]
    fn alt_disqualifies_a_key() {
        let alt = Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
        assert_eq!(action(&alt), None);
    }

    #[test]
    fn a_resize_reaches_the_shell() {
        assert_eq!(action(&Event::Resize(80, 24)), Some(Action::Resize(Size::new(80, 24))));
    }

    #[test]
    fn an_unbound_key_is_ignored_rather_than_guessed_at() {
        assert_eq!(action(&press(KeyCode::Char('z'))), None);
        assert_eq!(action(&press(KeyCode::Insert)), None);
    }
}
