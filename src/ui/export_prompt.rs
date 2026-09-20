//! The overlay behind `e`: an editable destination path and a Markdown/JSONL toggle.

use ratatui::layout::Size;
use unicode_width::UnicodeWidthStr;

use crate::render::line::{RenderedLine, StyledSpan, truncate};
use crate::theme::{Element, Theme};
use crate::ui::FRAME;
use crate::ui::app::ExportFormat;

pub const TITLE: &str = " Export ";
const MAX_WIDTH: u16 = 72;
const MARGIN: u16 = 4;
pub const CONTENT_ROWS: u16 = 4;
const PATH_PREFIX: &str = "path:";
const FORMAT_PREFIX: &str = "format:";
const HINT: &str = "Enter to write · Tab for the other format · Esc to cancel";
const CONFIRM_HINT: &str = "that file exists — overwrite? y/n";

pub fn outer(area: Size) -> Size {
    let width = area.width.saturating_sub(MARGIN).min(MAX_WIDTH);
    let height = CONTENT_ROWS.saturating_add(FRAME).min(area.height);
    Size::new(width, height)
}

const fn format_label(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Markdown => "Markdown",
        ExportFormat::Jsonl => "JSONL (raw transcript)",
    }
}

pub fn format_line(format: ExportFormat, width: usize, theme: &Theme) -> RenderedLine {
    labelled_line(FORMAT_PREFIX, format_label(format), width, theme)
}

pub fn path_line(path: &str, width: usize, theme: &Theme) -> RenderedLine {
    labelled_line(PATH_PREFIX, path, width, theme)
}

fn labelled_line(label: &str, value: &str, width: usize, theme: &Theme) -> RenderedLine {
    let mut line = RenderedLine::default();
    let label_text = truncate(label, width);
    let used = label_text.width();
    line.push(StyledSpan::new(label_text, theme.style(Element::Label)));
    if value.is_empty() {
        return line;
    }
    let room = width.saturating_sub(used);
    if room == 0 {
        return line;
    }
    let spaced = format!(" {value}");
    let value_text = truncate(&spaced, room);
    if value_text.trim().is_empty() {
        return line;
    }
    line.push(StyledSpan::new(value_text, theme.style(Element::Body)));
    line
}

pub fn hint_line(confirm: bool, width: usize, theme: &Theme) -> RenderedLine {
    let text = if confirm { CONFIRM_HINT } else { HINT };
    let mut line = RenderedLine::default();
    let element = if confirm { Element::StatusNotice } else { Element::Status };
    line.push(StyledSpan::new(truncate(text, width), theme.style(element)));
    line
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    use super::*;

    fn theme() -> Theme {
        Theme::default()
    }

    #[test]
    fn the_overlay_never_outgrows_the_area_it_floats_over() {
        for width in 1..200u16 {
            for height in 1..40u16 {
                let area = Size::new(width, height);
                assert!(outer(area).width <= width, "{area:?}");
                assert!(outer(area).height <= height, "{area:?}");
            }
        }
    }

    #[test]
    fn the_format_line_names_the_current_format() {
        assert!(format_line(ExportFormat::Markdown, 80, &theme()).text().contains("Markdown"));
        assert!(format_line(ExportFormat::Jsonl, 80, &theme()).text().contains("JSONL"));
    }

    #[test]
    fn the_path_line_carries_the_prompts_path() {
        let line = path_line("./out.md", 80, &theme());
        assert!(line.text().contains("./out.md"), "{}", line.text());
    }

    #[test]
    fn confirming_an_overwrite_replaces_the_hint_with_the_question() {
        let hint = hint_line(false, 80, &theme());
        assert!(hint.text().contains("Enter to write"), "{}", hint.text());
        let confirm = hint_line(true, 80, &theme());
        assert!(confirm.text().contains("overwrite"), "{}", confirm.text());
    }

    #[test]
    fn the_hint_reads_at_the_status_bars_colour_rather_than_the_dimmer_muted_one() {
        let theme = theme();
        let hint = hint_line(false, 80, &theme);
        assert_eq!(hint.spans[0].style, theme.style(Element::Status));
        assert_ne!(hint.spans[0].style, theme.style(Element::Muted));

        let confirm = hint_line(true, 80, &theme);
        assert_eq!(confirm.spans[0].style, theme.style(Element::StatusNotice), "the overwrite question must stay louder");
    }

    #[test]
    fn no_line_overflows_or_ends_in_whitespace_at_any_width() {
        for width in 1..=100_usize {
            for line in [
                format_line(ExportFormat::Markdown, width, &theme()),
                path_line("./a-fairly-long-destination-path-for-the-export.jsonl", width, &theme()),
                hint_line(false, width, &theme()),
                hint_line(true, width, &theme()),
            ] {
                let text = line.text();
                assert!(text.width() <= width, "width {width} overflowed to {}", text.width());
                assert_eq!(text.trim_end(), text, "width {width} left trailing whitespace: {text:?}");
            }
        }
    }
}
