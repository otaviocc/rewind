//! Context injections: the `attachment` records that outnumber Claude's own replies, collapsed to
//! one line per run and expanded only when asked for.

use std::fmt::Write as _;

use ratatui::style::Style;

use crate::domain::thread::{Conversation, NodeId, NodeKind};
use crate::render::line::{RenderedLine, StyledSpan, truncate};

const COLLAPSED: &str = "· ";
const EXPANDED: &str = "▾ ";
const INDENT: &str = "  ";
const SEPARATOR: &str = ", ";
const NAMED: usize = 3;
const UNKNOWN: &str = "injection";

pub fn kinds(conversation: &Conversation, run: &[NodeId]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for &id in run {
        let kind = kind(conversation, id);
        if !seen.contains(&kind) {
            seen.push(kind);
        }
    }
    seen
}

fn kind(conversation: &Conversation, id: NodeId) -> String {
    let Some(node) = conversation.node(id) else { return UNKNOWN.to_owned() };
    let NodeKind::Attachment(record) = &node.kind else { return UNKNOWN.to_owned() };
    record.attachment.get("type").and_then(serde_json::Value::as_str).unwrap_or(UNKNOWN).to_owned()
}

pub fn run(conversation: &Conversation, run: &[NodeId], expanded: bool, width: usize, style: Style) -> RenderedLine {
    let count = run.len();
    let plural = if count == 1 { "injection" } else { "injections" };
    let kinds = kinds(conversation, run);
    let named = kinds.len().min(NAMED);
    let (head, rest) = kinds.split_at_checked(named).unwrap_or((&kinds, &[]));
    let mut text = format!("{count} {plural}");
    if !head.is_empty() {
        text.push_str(" · ");
        text.push_str(&head.join(SEPARATOR));
    }
    if !rest.is_empty() {
        let _ = write!(text, " +{}", rest.len());
    }
    let glyph = if expanded { EXPANDED } else { COLLAPSED };
    one(&format!("{glyph}{text}"), width, style)
}

pub fn each(conversation: &Conversation, run: &[NodeId], width: usize, style: Style) -> Vec<RenderedLine> {
    run.iter().map(|&id| one(&format!("{INDENT}{}", kind(conversation, id)), width, style)).collect()
}

fn one(text: &str, width: usize, style: Style) -> RenderedLine {
    let mut line = RenderedLine::blank();
    line.push(StyledSpan::new(truncate(text, width), style));
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use unicode_width::UnicodeWidthStr;

    use crate::domain::thread;

    fn built(kinds: &[&str]) -> (Conversation, Vec<NodeId>) {
        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("session.jsonl");
        let mut lines = vec![
            r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:00Z","origin":{"kind":"human"},"message":{"role":"user","content":"go"}}"#
                .to_owned(),
        ];
        for (index, kind) in kinds.iter().enumerate() {
            let parent = if index == 0 { "u1".to_owned() } else { format!("x{}", index.saturating_sub(1)) };
            lines.push(format!(
                r#"{{"type":"attachment","uuid":"x{index}","parentUuid":"{parent}","sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:01Z","attachment":{{"type":"{kind}"}}}}"#
            ));
        }
        fs::write(&path, lines.join("\n") + "\n").expect("a written transcript");
        let conversation = thread::build(&path).expect("a built conversation");
        let run: Vec<NodeId> = (0..kinds.len()).filter_map(|index| conversation.id_of(&format!("x{index}"))).collect();
        (conversation, run)
    }

    #[test]
    fn a_run_says_how_many_it_hides_and_which_kinds_they_were() {
        let (conversation, found) = built(&["date", "instructions", "environment"]);
        assert_eq!(
            run(&conversation, &found, false, 80, Style::new()).text(),
            "· 3 injections · date, instructions, environment"
        );
    }

    #[test]
    fn one_injection_reads_as_one_rather_than_as_a_plural() {
        let (conversation, found) = built(&["date"]);
        assert_eq!(run(&conversation, &found, false, 80, Style::new()).text(), "· 1 injection · date");
    }

    #[test]
    fn a_repeated_kind_is_named_once_however_many_times_it_occurs() {
        let (conversation, found) = built(&["total_tokens_reminder"; 14]);
        assert_eq!(
            run(&conversation, &found, false, 80, Style::new()).text(),
            "· 14 injections · total_tokens_reminder",
            "the two noise types are 79% of the corpus and must not be named 14 times"
        );
    }

    #[test]
    fn more_than_three_kinds_names_three_and_counts_the_rest() {
        let (conversation, found) = built(&["date", "instructions", "environment", "plan_mode", "model"]);
        assert_eq!(
            run(&conversation, &found, false, 80, Style::new()).text(),
            "· 5 injections · date, instructions, environment +2"
        );
    }

    #[test]
    fn an_attachment_with_no_type_is_named_rather_than_left_blank() {
        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("session.jsonl");
        let lines = [
            r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:00Z","origin":{"kind":"human"},"message":{"role":"user","content":"go"}}"#,
            r#"{"type":"attachment","uuid":"x0","parentUuid":"u1","sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:01Z","attachment":{"payload":1}}"#,
        ];
        fs::write(&path, lines.join("\n") + "\n").expect("a written transcript");
        let conversation = thread::build(&path).expect("a built conversation");
        let found = vec![conversation.id_of("x0").expect("the attachment")];
        assert_eq!(run(&conversation, &found, false, 80, Style::new()).text(), "· 1 injection · injection");
    }

    #[test]
    fn expanding_a_run_names_every_injection_in_it_in_order() {
        let (conversation, found) = built(&["date", "instructions", "date"]);
        let lines: Vec<String> = each(&conversation, &found, 80, Style::new()).iter().map(RenderedLine::text).collect();
        assert_eq!(lines, ["  date", "  instructions", "  date"], "expanded, a repeat is shown every time it happened");
    }

    #[test]
    fn an_expanded_run_is_marked_as_such_on_its_own_line() {
        let (conversation, found) = built(&["date"]);
        assert!(run(&conversation, &found, true, 80, Style::new()).text().starts_with("▾ "));
        assert!(run(&conversation, &found, false, 80, Style::new()).text().starts_with("· "));
    }

    #[test]
    fn no_injection_line_overflows_or_ends_in_whitespace_at_any_width() {
        let (conversation, found) = built(&["date", "instructions", "environment", "plan_mode"]);
        for width in 1..=100_usize {
            for line in std::iter::once(run(&conversation, &found, true, width, Style::new())).chain(each(
                &conversation,
                &found,
                width,
                Style::new(),
            )) {
                let text = line.text();
                assert!(text.width() <= width, "width {width} overflowed to {}", text.width());
                assert_eq!(text.trim_end(), text, "width {width} left trailing whitespace: {text:?}");
            }
        }
    }
}
