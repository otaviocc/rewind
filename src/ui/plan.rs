//! The window behind `Enter` on a plan: the whole document, unfolded.

use ratatui::layout::Size;

use crate::markdown;
use crate::render::line::RenderedLine;
use crate::render::prose;
use crate::theme::Theme;
use crate::ui::FRAME;

pub const TITLE: &str = " Plan ";
const MARGIN: u16 = 8;
const MAX_WIDTH: u16 = 100;
const MARGIN_Y: u16 = 2;

pub fn outer(area: Size) -> Size {
    let width = area.width.saturating_sub(MARGIN).min(MAX_WIDTH);
    let height = area.height.saturating_sub(MARGIN_Y);
    Size::new(width, height)
}

pub fn inner(area: Size) -> Size {
    let outer = outer(area);
    Size::new(outer.width.saturating_sub(FRAME), outer.height.saturating_sub(FRAME))
}

pub fn lines(text: &str, width: usize, theme: &Theme) -> Vec<RenderedLine> {
    prose::render(&markdown::parse(text), width.max(1), theme)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_leaves_a_margin_rather_than_filling_the_screen() {
        let outer = outer(Size::new(120, 40));
        assert!(outer.width < 120 && outer.height < 40, "{outer:?}");
    }

    #[test]
    fn a_wide_terminal_stops_the_window_growing_past_a_readable_column() {
        assert_eq!(outer(Size::new(400, 40)).width, MAX_WIDTH);
    }

    #[test]
    fn the_inner_column_is_the_outer_one_less_the_frame() {
        let area = Size::new(120, 40);
        assert_eq!(inner(area).width, outer(area).width.saturating_sub(FRAME));
    }

    #[test]
    fn a_plan_renders_whole_rather_than_folding_the_way_the_transcript_does() {
        let text = "# Retune the array\n\n".to_owned() + &"a line of the plan\n\n".repeat(40);
        let rows = lines(&text, 60, &Theme::default());
        assert!(rows.len() > 40, "nothing is held back: {} rows", rows.len());
        assert!(!rows.iter().any(|row| row.text().contains("more lines")), "no fold tail");
    }

    #[test]
    fn a_terminal_too_small_for_a_frame_still_reports_a_size() {
        let outer = outer(Size::new(4, 1));
        assert_eq!(outer.width, 0);
        assert_eq!(inner(Size::new(4, 1)).width, 0);
    }
}
