//! A tool call as one dense line: a glyph, the tool's name, a digest of what it was asked to do,
//! and how it went.

use ratatui::style::Style;
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::domain::thread::Conversation;
use crate::domain::tool::{self, Label, Outcome, Status};
use crate::render::line::{RenderedLine, StyledSpan, normalise, truncate};

const COLLAPSED: &str = "▸ ";
const SEPARATOR: &str = " · ";
const HEAD_GAP: &str = "  ";
const TAIL_GAP: usize = 1;
const LEAST_DIGEST: usize = 8;

pub struct Styles {
    pub glyph: Style,
    pub name: Style,
    pub digest: Style,
    pub muted: Style,
    pub error: Style,
}

pub fn collapsed(
    conversation: &Conversation,
    id: &str,
    name: &str,
    input: &Value,
    width: usize,
    styles: &Styles,
) -> RenderedLine {
    let node = conversation.result_of(id);
    let outcome = node.and_then(|node| Outcome::of(node, id));
    let status = tool::status(outcome.as_ref());
    let digest = tool::digest(name, input, outcome.and_then(|outcome| outcome.detail));
    let detail = joined(digest.primary.as_deref(), digest.secondary.as_deref());
    line(name, &detail, status, width, styles)
}

fn joined(primary: Option<&str>, secondary: Option<&str>) -> String {
    match (primary, secondary) {
        (None, None) => String::new(),
        (Some(text), None) | (None, Some(text)) => normalise(text).replace('\n', " "),
        (Some(primary), Some(secondary)) => {
            format!("{}{SEPARATOR}{}", normalise(primary).replace('\n', " "), normalise(secondary).replace('\n', " "))
        }
    }
}

fn line(name: &str, detail: &str, status: Status, width: usize, styles: &Styles) -> RenderedLine {
    let mut line = RenderedLine::blank();
    let heading = heading(name);
    let head_width = COLLAPSED.width().saturating_add(heading.width());
    let outcome = label_of(status);
    let outcome_width = outcome.width();

    if head_width.saturating_add(TAIL_GAP).saturating_add(outcome_width) > width {
        line.push(StyledSpan::new(truncate(&format!("{COLLAPSED}{heading}"), width), styles.name));
        return line;
    }
    line.push(StyledSpan::new(COLLAPSED, styles.glyph));
    line.push(StyledSpan::new(heading, styles.name));

    let spent = head_width.saturating_add(HEAD_GAP.width()).saturating_add(outcome_width).saturating_add(TAIL_GAP);
    let room = width.saturating_sub(spent);
    let detail = if room >= LEAST_DIGEST { truncate(detail, room) } else { String::new() };

    let mut tail = head_width;
    if !detail.is_empty() {
        line.push(StyledSpan::new(HEAD_GAP, styles.digest));
        line.push(StyledSpan::new(detail.clone(), styles.digest));
        tail = tail.saturating_add(HEAD_GAP.width()).saturating_add(detail.width());
    }

    let pad = width.saturating_sub(tail).saturating_sub(outcome_width).max(TAIL_GAP);
    line.push(StyledSpan::new(" ".repeat(pad), styles.muted));
    line.push(StyledSpan::new(outcome, if status.is_error() { styles.error } else { styles.muted }));
    line
}

fn heading(name: &str) -> String {
    match tool::label(name) {
        Label::Plain(name) => name.to_owned(),
        Label::Mcp { server, tool } => format!("{server}{SEPARATOR}{tool}"),
    }
}

const fn label_of(status: Status) -> &'static str {
    match status {
        Status::Ok => "ok",
        Status::Failed => "failed",
        Status::Denied => "denied",
        Status::Interrupted => "interrupted",
        Status::Pending => "pending",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier};

    fn styles() -> Styles {
        Styles {
            glyph: Style::new(),
            name: Style::new().add_modifier(Modifier::BOLD),
            digest: Style::new(),
            muted: Style::new().fg(Color::DarkGray),
            error: Style::new().fg(Color::Red),
        }
    }

    fn rendered(name: &str, detail: &str, status: Status, width: usize) -> String {
        line(name, detail, status, width, &styles()).text()
    }

    #[test]
    fn a_collapsed_call_is_its_name_its_digest_and_a_right_aligned_outcome() {
        let text = rendered("Bash", "wc -l src/engine/grid.rs", Status::Ok, 60);
        assert_eq!(text, "▸ Bash  wc -l src/engine/grid.rs                          ok");
        assert_eq!(text.width(), 60, "the outcome ends on the last column");
    }

    #[test]
    fn an_mcp_name_reads_as_a_server_and_a_tool() {
        assert!(rendered("mcp__jeffries__beam_status", "", Status::Denied, 60).starts_with("▸ jeffries · beam_status"));
    }

    #[test]
    fn the_outcome_survives_a_column_too_narrow_for_the_digest() {
        let text = rendered("Bash", "cargo build --release", Status::Failed, 16);
        assert_eq!(text, "▸ Bash    failed", "the digest goes before the outcome does");
        assert_eq!(text.width(), 16);
    }

    #[test]
    fn a_column_too_narrow_for_even_the_outcome_keeps_the_name() {
        let text = rendered("mcp__jeffries__beam_status", "", Status::Ok, 12);
        assert_eq!(text, "▸ jeffries …", "the name is what is left");
        assert!(text.width() <= 12);
    }

    #[test]
    fn no_collapsed_line_ends_in_whitespace_at_any_width() {
        for width in 1..=120_usize {
            for status in [Status::Ok, Status::Failed, Status::Denied, Status::Interrupted, Status::Pending] {
                let text = rendered("mcp__jeffries__beam_status", "cargo build -v 2>&1", status, width);
                assert_eq!(text.trim_end(), text, "width {width} left trailing whitespace: {text:?}");
                assert!(text.width() <= width, "width {width} overflowed to {}", text.width());
            }
        }
    }

    #[test]
    fn a_newline_in_a_digest_never_reaches_the_width_arithmetic() {
        let detail = joined(Some("git commit -m 'one\ntwo'"), None);
        assert!(!detail.contains('\n'));
        assert_eq!(detail, "git commit -m 'one two'");
    }

    #[test]
    fn an_ansi_escape_in_a_digest_is_stripped_before_it_is_measured() {
        let detail = joined(Some("\u{1b}[31mred\u{1b}[0m"), None);
        assert_eq!(detail, "red");
    }

    #[test]
    fn a_digest_with_both_halves_joins_them_with_the_separator() {
        assert_eq!(joined(Some("Explore"), Some("Trace the grid scanner")), "Explore · Trace the grid scanner");
        assert_eq!(joined(None, Some("212 lines")), "212 lines");
        assert_eq!(joined(None, None), "");
    }

    #[test]
    fn only_the_three_error_outcomes_are_painted_in_the_error_style() {
        let error = |status| {
            line("Bash", "x", status, 40, &styles())
                .spans
                .last()
                .map(|span| span.style)
                .is_some_and(|style| style == styles().error)
        };
        assert!(error(Status::Failed) && error(Status::Denied) && error(Status::Interrupted));
        assert!(!error(Status::Ok), "a call that worked is not an error");
        assert!(!error(Status::Pending), "a session that ended mid-call did not fail");
    }
}
