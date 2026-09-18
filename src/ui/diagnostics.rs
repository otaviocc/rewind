//! The list behind `d`: every defect the reader has found so far, one group per file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ratatui::layout::Size;
use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthStr;

use crate::domain::diagnostics::{Defect, Diagnostics};
use crate::render::line::{RenderedLine, StyledSpan, truncate};

pub const TITLE: &str = " Diagnostics ";
const MAX_WIDTH: u16 = 78;
const MARGIN: u16 = 4;
const BORDER: u16 = 2;
const NOTHING: &str = "Nothing unreadable.";
const INSET: usize = 2;

const fn file_style() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

const fn count_style() -> Style {
    Style::new().fg(Color::Yellow)
}

const fn defect_style() -> Style {
    Style::new()
}

const fn muted_style() -> Style {
    Style::new().fg(Color::DarkGray)
}

pub fn outer(area: Size) -> Size {
    let width = area.width.saturating_sub(MARGIN).min(MAX_WIDTH);
    let height = area.height.saturating_sub(BORDER);
    Size::new(width, height)
}

pub fn inner(area: Size) -> Size {
    let outer = outer(area);
    Size::new(outer.width.saturating_sub(BORDER), outer.height.saturating_sub(BORDER))
}

pub fn lines(drift: &BTreeMap<PathBuf, Diagnostics>, width: usize) -> Vec<RenderedLine> {
    if drift.is_empty() {
        return vec![line(NOTHING, muted_style(), 0, width)];
    }

    let mut lines = Vec::new();
    for (path, diagnostics) in drift {
        if !lines.is_empty() {
            lines.push(RenderedLine::blank());
        }
        lines.push(heading(path, diagnostics, width));
        for defect in diagnostics.defects() {
            lines.push(line(&describe(defect), defect_style(), INSET, width));
        }
        let held = diagnostics.defects().len();
        if let Some(beyond) = diagnostics.count().checked_sub(held).filter(|beyond| *beyond > 0) {
            lines.push(line(&format!("… and {beyond} more"), muted_style(), INSET, width));
        }
    }
    lines
}

fn heading(path: &Path, diagnostics: &Diagnostics, width: usize) -> RenderedLine {
    let name = path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned());
    let count = diagnostics.count();
    let plural = if count == 1 { "defect" } else { "defects" };
    let tally = format!(" · {count} {plural}");
    let mut rendered = RenderedLine::default();
    rendered.push(StyledSpan::new(truncate(&name, width.saturating_sub(tally.width())), file_style()));
    rendered.push(StyledSpan::new(truncate(&tally, width), count_style()));
    rendered
}

fn line(text: &str, style: Style, inset: usize, width: usize) -> RenderedLine {
    let mut rendered = RenderedLine::default();
    rendered.push(StyledSpan::new(" ".repeat(inset.min(width)), Style::new()));
    rendered.push(StyledSpan::new(truncate(text, width.saturating_sub(inset)), style));
    rendered
}

fn describe(defect: &Defect) -> String {
    match defect {
        Defect::Oversized { line, bytes } => format!("line {line} · oversized · {bytes} bytes"),
        Defect::Truncated { line } => format!("line {line} · truncated mid-record"),
        Defect::Unparseable { line, message } => format!("line {line} · unparseable · {message}"),
        Defect::UnknownRecord { line, kind } => format!("line {line} · unknown record type \"{kind}\""),
        Defect::UnknownBlock { line, kind } => format!("line {line} · unknown content block \"{kind}\""),
        Defect::OrphanedParent { line, uuid } => format!("line {line} · parent {uuid} is not in the file"),
        Defect::SeveredCycle { line, uuid } => format!("line {line} · cycle severed at {uuid}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drift(defects: Vec<Defect>) -> BTreeMap<PathBuf, Diagnostics> {
        let path = PathBuf::from("/tmp/44444444.jsonl");
        let mut diagnostics = Diagnostics::new(&path);
        for defect in defects {
            diagnostics.push(defect);
        }
        BTreeMap::from([(path, diagnostics)])
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
            }
        }
    }

    #[test]
    fn an_empty_store_still_says_something() {
        let rendered = lines(&BTreeMap::new(), 80);
        assert_eq!(texts(&rendered), [NOTHING]);
    }

    #[test]
    fn every_defect_is_named_under_the_file_it_was_found_in() {
        let rendered = lines(
            &drift(vec![
                Defect::UnknownBlock { line: 12, kind: "server_tool_use".to_owned() },
                Defect::UnknownRecord { line: 7, kind: "telemetry-latch".to_owned() },
                Defect::Truncated { line: 31 },
            ]),
            80,
        );
        assert_eq!(
            texts(&rendered),
            [
                "44444444.jsonl · 3 defects",
                "  line 12 · unknown content block \"server_tool_use\"",
                "  line 7 · unknown record type \"telemetry-latch\"",
                "  line 31 · truncated mid-record",
            ]
        );
    }

    #[test]
    fn one_defect_is_not_pluralized() {
        let rendered = lines(&drift(vec![Defect::Truncated { line: 2 }]), 80);
        assert!(texts(&rendered).first().is_some_and(|heading| heading.ends_with("1 defect")), "{:?}", texts(&rendered));
    }

    #[test]
    fn the_defects_beyond_the_stored_limit_are_announced_rather_than_dropped_silently() {
        let mut diagnostics = Diagnostics::new(Path::new("/tmp/noisy.jsonl"));
        for line in 0..200u64 {
            diagnostics.push(Defect::Truncated { line });
        }
        let held = diagnostics.defects().len();
        let store = BTreeMap::from([(PathBuf::from("/tmp/noisy.jsonl"), diagnostics)]);
        let rendered = texts(&lines(&store, 80));
        assert_eq!(rendered.last().map(String::as_str), Some(format!("  … and {} more", 200 - held).as_str()));
    }

    #[test]
    fn two_files_are_separated_by_a_blank_line() {
        let mut store = drift(vec![Defect::Truncated { line: 1 }]);
        let other = PathBuf::from("/tmp/11111111.jsonl");
        let mut diagnostics = Diagnostics::new(&other);
        diagnostics.push(Defect::Truncated { line: 9 });
        store.insert(other, diagnostics);
        let rendered = texts(&lines(&store, 80));
        assert_eq!(rendered.len(), 5, "{rendered:?}");
        assert_eq!(rendered.get(2).map(String::as_str), Some(""));
    }

    #[test]
    fn a_narrow_pane_truncates_rather_than_overflows() {
        let rendered = lines(&drift(vec![Defect::UnknownBlock { line: 12, kind: "server_tool_use".to_owned() }]), 20);
        assert!(rendered.iter().all(|line| line.width() <= 20), "{:?}", texts(&rendered));
    }
}
