//! The overlay behind `?`: type a query, see ranked hits, `Enter` to open one.

use ratatui::layout::Size;

use crate::domain::cache::shard::{Field, Kind};
use crate::domain::search::engine::Hit;
use crate::render::line::{RenderedLine, StyledSpan, truncate};
use crate::theme::{Element, Theme};
use crate::ui::FRAME;

pub const TITLE: &str = " Search ";
const MAX_WIDTH: u16 = 100;
const MARGIN: u16 = 4;
const MARGIN_Y: u16 = 2;
pub const HEADER_ROWS: u16 = 2;
const PROMPT_PREFIX: &str = "? ";
const PLACEHOLDER: &str = "type to search…";
const HELP: &str = "is:user|assistant|tool|thinking · project:name · -exclude · \"phrase\"";

pub fn outer(area: Size) -> Size {
    let width = area.width.saturating_sub(MARGIN).min(MAX_WIDTH);
    let height = area.height.saturating_sub(MARGIN_Y);
    Size::new(width, height)
}

pub fn inner(area: Size) -> Size {
    let outer = outer(area);
    Size::new(outer.width.saturating_sub(FRAME), outer.height.saturating_sub(FRAME))
}

pub fn prompt_line(query: &str, width: usize, theme: &Theme) -> RenderedLine {
    let mut line = RenderedLine::default();
    line.push(StyledSpan::new(PROMPT_PREFIX, theme.style(Element::Label)));
    let room = width.saturating_sub(PROMPT_PREFIX.len());
    if query.is_empty() {
        line.push(StyledSpan::new(truncate(PLACEHOLDER, room), theme.style(Element::Muted)));
    } else {
        line.push(StyledSpan::new(truncate(query, room), theme.style(Element::Body)));
    }
    line
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorpusStatus {
    Indexing,
    Partial,
    Complete,
}

pub fn status_line(query: &str, status: CorpusStatus, count: usize, width: usize, theme: &Theme) -> RenderedLine {
    let text = status_text(query, status, count);
    let mut line = RenderedLine::default();
    line.push(StyledSpan::new(truncate(&text, width), theme.style(Element::Status)));
    line
}

fn status_text(query: &str, status: CorpusStatus, count: usize) -> String {
    if status == CorpusStatus::Indexing {
        return "indexing…".to_owned();
    }
    let suffix = if status == CorpusStatus::Partial { " (partial)" } else { "" };
    if query.is_empty() {
        return format!("{HELP}{suffix}");
    }
    if count == 0 {
        return format!("no matches{suffix}");
    }
    let plural = if count == 1 { "match" } else { "matches" };
    format!("{count} {plural}{suffix}")
}

pub fn rows(hits: &[Hit], width: usize, theme: &Theme) -> Vec<RenderedLine> {
    hits.iter().map(|hit| row(hit, width, theme)).collect()
}

fn row(hit: &Hit, width: usize, theme: &Theme) -> RenderedLine {
    let place = hit.directory.as_deref().unwrap_or("history");
    let text = if hit.kind == Kind::Subagent {
        format!("{place} · subagent · {} · L{}", field_label(hit.kind, hit.field), hit.line_no)
    } else {
        format!("{place} · {} · L{}", field_label(hit.kind, hit.field), hit.line_no)
    };
    let mut line = RenderedLine::default();
    line.push(StyledSpan::new(truncate(&text, width), theme.style(Element::Body)));
    line
}

const fn field_label(kind: Kind, field: Field) -> &'static str {
    match (kind, field) {
        (Kind::History, _) => "history",
        (_, Field::UserPrompt) => "prompt",
        (_, Field::AssistantText) => "assistant",
        (_, Field::Thinking) => "thinking",
        (_, Field::ToolInput) => "tool input",
        (_, Field::ToolResult) => "tool result",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::default()
    }

    fn hit(directory: Option<&str>, kind: Kind, field: Field, line_no: u32) -> Hit {
        Hit {
            score: 0,
            directory: directory.map(str::to_owned),
            file_name: "s1.jsonl".to_owned(),
            line_no,
            byte_off: 0,
            ts_ms: 0,
            kind,
            field,
        }
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
    fn an_empty_query_shows_the_placeholder_and_the_syntax_help() {
        let prompt = prompt_line("", 80, &theme());
        assert_eq!(prompt.text(), format!("{PROMPT_PREFIX}{PLACEHOLDER}"));
        let status = status_line("", CorpusStatus::Complete, 0, 80, &theme());
        assert_eq!(status.text(), HELP);
    }

    #[test]
    fn the_status_row_reads_at_the_status_bars_colour_and_the_placeholder_still_recedes() {
        let theme = theme();
        let status = status_line("", CorpusStatus::Complete, 0, 80, &theme);
        assert_eq!(status.spans[0].style, theme.style(Element::Status));

        let prompt = prompt_line("", 80, &theme);
        assert_eq!(prompt.spans[1].style, theme.style(Element::Muted), "a placeholder is not prose to read");
    }

    #[test]
    fn a_loading_corpus_says_so_regardless_of_the_query() {
        let status = status_line("grid", CorpusStatus::Indexing, 0, 80, &theme());
        assert_eq!(status.text(), "indexing…");
    }

    #[test]
    fn zero_matches_reads_distinctly_from_no_query_typed_yet() {
        let status = status_line("nothing-matches-this", CorpusStatus::Complete, 0, 80, &theme());
        assert_eq!(status.text(), "no matches");
    }

    #[test]
    fn a_single_match_is_not_pluralized() {
        let status = status_line("grid", CorpusStatus::Complete, 1, 80, &theme());
        assert_eq!(status.text(), "1 match");
    }

    #[test]
    fn a_partial_scan_appends_partial_to_the_match_count() {
        let status = status_line("grid", CorpusStatus::Partial, 3, 80, &theme());
        assert_eq!(status.text(), "3 matches (partial)");
    }

    #[test]
    fn a_partial_scan_with_no_matches_yet_still_reads_partial() {
        let status = status_line("nothing-yet", CorpusStatus::Partial, 0, 80, &theme());
        assert_eq!(status.text(), "no matches (partial)");
    }

    #[test]
    fn a_partial_scan_with_no_query_appends_partial_to_the_help_text() {
        let status = status_line("", CorpusStatus::Partial, 0, 80, &theme());
        assert_eq!(status.text(), format!("{HELP} (partial)"));
    }

    #[test]
    fn a_history_hit_is_labelled_history_regardless_of_field() {
        let rendered = row(&hit(None, Kind::History, Field::UserPrompt, 3), 80, &theme());
        assert!(rendered.text().starts_with("history · history"));
    }

    #[test]
    fn a_transcript_hit_shows_its_project_field_and_line() {
        let rendered = row(&hit(Some("-a-project"), Kind::Transcript, Field::ToolResult, 12), 80, &theme());
        assert_eq!(rendered.text(), "-a-project · tool result · L12");
    }

    #[test]
    fn a_subagent_hit_is_distinguished_from_a_transcript_hit() {
        let rendered = row(&hit(Some("-a-project"), Kind::Subagent, Field::UserPrompt, 5), 80, &theme());
        assert_eq!(rendered.text(), "-a-project · subagent · prompt · L5");
    }

    #[test]
    fn a_narrow_pane_truncates_rather_than_overflows() {
        let hits = [hit(Some("-a-very-long-project-directory-name-here"), Kind::Transcript, Field::AssistantText, 999)];
        let rendered = rows(&hits, 20, &theme());
        assert!(rendered.iter().all(|line| line.width() <= 20), "{rendered:?}");
    }
}
