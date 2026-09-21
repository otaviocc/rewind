//! The window behind `?`/`F1`: every key, grouped by what it is for.

use ratatui::layout::Size;

use crate::render::line::{RenderedLine, StyledSpan, truncate};
use crate::theme::{Element, Theme};
use crate::ui::FRAME;

pub const TITLE: &str = " Keys ";
const WIDTH_FRACTION: u16 = 60;
const MIN_WIDTH: u16 = 44;
const MAX_WIDTH: u16 = 76;
const HEIGHT_FRACTION: u16 = 70;
const MIN_HEIGHT: u16 = 9;
const MAX_HEIGHT: u16 = 30;
const KEYS_COLUMN: usize = 20;

const SECTIONS: &[(&str, &[(&str, &str)])] = &[
    (
        "Moving",
        &[
            ("h l Tab", "move between columns"),
            ("j k", "move within a column"),
            ("g G", "top · bottom of the column"),
            ("d u Ctrl-d Ctrl-u", "half page down · up"),
            ("f", "focus the conversation full-width"),
        ],
    ),
    (
        "Reading",
        &[
            ("Enter", "descend · expand the selected tool call · enter a subagent · read a plan"),
            ("Esc", "leave a subagent · close a plan · back out"),
            ("[ ]", "previous · next message"),
            ("n p N", "next · previous tool call, or step through a locked / filter"),
            ("Space t", "expand one tool call · all of them"),
            ("i", "reveal context injections"),
            ("b", "cycle the alternate branches at the marker"),
            ("D", "what could not be read"),
        ],
    ),
    ("Finding", &[("/", "filter the list"), ("s", "search everything")]),
    (
        "Taking away",
        &[("y Y", "copy the message · the whole session"), ("c", "copy claude --resume <id>"), ("e", "export to a file")],
    ),
    ("Leaving", &[("? F1", "these keys · Esc or q closes this window"), ("q", "quit")]),
];

pub fn outer(area: Size) -> Size {
    let width = area.width.saturating_mul(WIDTH_FRACTION).saturating_div(100).clamp(MIN_WIDTH, MAX_WIDTH).min(area.width);
    let height = area.height.saturating_mul(HEIGHT_FRACTION).saturating_div(100).clamp(MIN_HEIGHT, MAX_HEIGHT).min(area.height);
    Size::new(width, height)
}

pub fn inner(area: Size) -> Size {
    let outer = outer(area);
    Size::new(outer.width.saturating_sub(FRAME), outer.height.saturating_sub(FRAME))
}

pub fn lines(width: usize, theme: &Theme) -> Vec<RenderedLine> {
    let mut lines = Vec::new();
    for (index, (heading, keys)) in SECTIONS.iter().enumerate() {
        if index > 0 {
            lines.push(RenderedLine::blank());
        }
        lines.push(section_heading(heading, width, theme));
        for (keys, meaning) in *keys {
            lines.push(key_line(keys, meaning, width, theme));
        }
    }
    lines
}

fn section_heading(text: &str, width: usize, theme: &Theme) -> RenderedLine {
    let mut rendered = RenderedLine::default();
    rendered.push(StyledSpan::new(truncate(text, width), theme.style(Element::HeaderTitle)));
    rendered
}

fn key_line(keys: &str, meaning: &str, width: usize, theme: &Theme) -> RenderedLine {
    let mut rendered = RenderedLine::default();
    let padded = format!("{keys:<KEYS_COLUMN$}");
    let keys_width = KEYS_COLUMN.min(width);
    rendered.push(StyledSpan::new(truncate(&padded, keys_width), theme.style(Element::Label)));
    let remaining = width.saturating_sub(keys_width);
    if remaining > 0 {
        rendered.push(StyledSpan::new(truncate(meaning, remaining), theme.style(Element::Body)));
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::{Column, Mode};
    use crate::ui::input::{self, Viewport};
    use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    fn theme() -> Theme {
        Theme::default()
    }

    fn texts(lines: &[RenderedLine]) -> Vec<String> {
        lines.iter().map(RenderedLine::text).collect()
    }

    #[test]
    fn the_overlay_never_outgrows_the_area_it_floats_over() {
        for width in 1..200u16 {
            for height in 1..40u16 {
                let area = Size::new(width, height);
                assert!(outer(area).width <= width, "{area:?}");
                assert!(outer(area).height <= height, "{area:?}");
                assert!(inner(area).width <= outer(area).width, "{area:?}");
                assert!(inner(area).height <= outer(area).height, "{area:?}");
            }
        }
    }

    #[test]
    fn every_key_it_advertises_is_actually_bound() {
        let viewport = Viewport {
            area: Size::new(120, 24),
            mode: Mode::Browse,
            focused: Column::Projects,
            text_entry: false,
            overlay: false,
        };
        for (_, keys) in SECTIONS {
            for (keys, _) in *keys {
                for token in keys.split(' ') {
                    let event = to_event(token);
                    let resolved = input::action(&event, viewport);
                    assert!(resolved.is_some(), "{token} in the help table does not resolve to an action");
                }
            }
        }
    }

    fn to_event(token: &str) -> Event {
        match token {
            "Tab" => Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            "Enter" => Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            "Esc" => Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            "Space" => Event::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)),
            "F1" => Event::Key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE)),
            "[" | "]" => Event::Key(KeyEvent::new(KeyCode::Char(token.chars().next().expect("one char")), KeyModifiers::NONE)),
            "Ctrl-d" => Event::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            "Ctrl-u" => Event::Key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)),
            other if other.chars().count() == 1 => {
                let character = other.chars().next().expect("one char");
                Event::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
            }
            other => panic!("no event mapping for token {other:?}"),
        }
    }

    #[test]
    fn a_wide_pane_still_fits_every_line_within_its_width() {
        let rendered = lines(80, &theme());
        assert!(!rendered.is_empty());
        assert!(rendered.iter().all(|line| line.width() <= 80), "{:?}", texts(&rendered));
    }

    #[test]
    fn a_narrow_pane_truncates_rather_than_overflows() {
        let rendered = lines(20, &theme());
        assert!(rendered.iter().all(|line| line.width() <= 20), "{:?}", texts(&rendered));
    }

    #[test]
    fn sections_are_separated_by_a_blank_line() {
        let rendered = texts(&lines(80, &theme()));
        assert!(rendered.contains(&String::new()));
    }
}
