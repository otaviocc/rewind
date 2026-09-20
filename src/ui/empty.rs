//! What a column shows in place of rows, when there are none to draw: an explanation
//! rather than a blank pane.

use ratatui::style::Style;

use crate::render::line::{RenderedLine, StyledSpan, truncate};
use crate::theme::{Element, Theme};

const INSET: usize = 2;

pub fn lines(primary: &str, detail: Option<&str>, width: usize, theme: &Theme) -> Vec<RenderedLine> {
    let mut lines = vec![line(primary, theme.style(Element::Muted), width)];
    if let Some(detail) = detail {
        lines.push(line(detail, theme.style(Element::Hint), width));
    }
    lines
}

fn line(text: &str, style: Style, width: usize) -> RenderedLine {
    let mut rendered = RenderedLine::default();
    rendered.push(StyledSpan::new(" ".repeat(INSET.min(width)), Style::new()));
    rendered.push(StyledSpan::new(truncate(text, width.saturating_sub(INSET)), style));
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::default()
    }

    fn texts(lines: &[RenderedLine]) -> Vec<String> {
        lines.iter().map(RenderedLine::text).collect()
    }

    #[test]
    fn a_primary_line_alone_is_one_line() {
        let rendered = lines("No projects here yet.", None, 80, &theme());
        assert_eq!(texts(&rendered), ["  No projects here yet."]);
    }

    #[test]
    fn a_detail_line_follows_the_primary_one() {
        let rendered = lines("No transcripts in this project.", Some("/Users/fixture/Developer/shuttlebay"), 80, &theme());
        assert_eq!(texts(&rendered), ["  No transcripts in this project.", "  /Users/fixture/Developer/shuttlebay"]);
    }

    #[test]
    fn a_narrow_pane_truncates_rather_than_overflows() {
        let rendered = lines("No projects here yet. Claude Code writes one per directory it is run in.", None, 20, &theme());
        assert!(rendered.iter().all(|line| line.width() <= 20), "{:?}", texts(&rendered));
    }

    #[test]
    fn the_primary_line_reads_muted_and_the_detail_reads_as_a_hint() {
        let rendered = lines("primary", Some("detail"), 80, &theme());
        assert_eq!(rendered[0].spans[1].style, theme().style(Element::Muted));
        assert_eq!(rendered[1].spans[1].style, theme().style(Element::Hint));
    }
}
