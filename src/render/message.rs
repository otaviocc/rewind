//! One conversation becomes styled lines: role headers, prose, and a one-line stand-in for
//! everything M2 will make expandable.

use ratatui::style::{Color, Modifier, Style};
use serde_json::Value;

use crate::domain::block::{Block, Content, ImageSource};
use crate::domain::thread::{Conversation, NodeKind};
use crate::render::line::{RenderedLine, StyledSpan, truncate};
use crate::render::prose;
use unicode_width::UnicodeWidthStr;

const GUTTER: &str = "▎";
const HUMAN_LABEL: &str = "you";
const ASSISTANT_LABEL: &str = "claude";
const TOOL_MARKER: &str = "▸ ";
const SEPARATOR: &str = " · ";
const LABEL_GAP: usize = 2;
const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
const UNIT: usize = 1024;

const fn label_style() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

const fn dim_style() -> Style {
    Style::new().fg(Color::DarkGray)
}

const fn body_style() -> Style {
    Style::new()
}

pub fn transcript(conversation: &Conversation, width: usize) -> Vec<RenderedLine> {
    let mut lines = Vec::new();
    for &id in conversation.thread() {
        let Some(node) = conversation.node(id) else { continue };
        let before = lines.len();
        match &node.kind {
            NodeKind::User(record) if record.is_human_turn() && !record.is_compact_summary => {
                lines.push(header(HUMAN_LABEL, None, width));
                content(&record.message.content, width, &mut lines);
            }
            NodeKind::Assistant(turn) => {
                lines.push(header(ASSISTANT_LABEL, turn.model.as_deref(), width));
                blocks(&turn.content, width, &mut lines);
            }
            NodeKind::User(_) | NodeKind::System(_) | NodeKind::Attachment(_) => {}
        }
        if lines.len() > before {
            lines.push(RenderedLine::blank());
        }
    }
    while lines.last().is_some_and(RenderedLine::is_blank) {
        lines.pop();
    }
    lines
}

fn header(label: &str, detail: Option<&str>, width: usize) -> RenderedLine {
    let mut line = RenderedLine::blank();
    let room = width.saturating_sub(GUTTER.width());
    line.push(StyledSpan::new(truncate(label, room), label_style()));
    let left = room.saturating_sub(label.width()).saturating_sub(LABEL_GAP);
    if let Some(detail) = detail.filter(|_| left > 0) {
        line.push(StyledSpan::new(format!("{:LABEL_GAP$}{}", "", truncate(detail, left)), dim_style()));
    }
    line.prefix(StyledSpan::new(GUTTER, dim_style()));
    line
}

fn content(content: &Content, width: usize, lines: &mut Vec<RenderedLine>) {
    match content {
        Content::Text(text) => lines.extend(markdown(text, width)),
        Content::Blocks(blocks_of) => blocks(blocks_of, width, lines),
    }
}

fn blocks(blocks: &[Block], width: usize, lines: &mut Vec<RenderedLine>) {
    for block in blocks {
        match block {
            Block::Text { text } => lines.extend(markdown(text, width)),
            Block::Thinking { thinking } => lines.push(one(&thinking_summary(thinking), dim_style(), width)),
            Block::ToolUse { name, input, .. } => lines.push(one(&tool_summary(name, input), body_style(), width)),
            Block::Image { source } => lines.push(one(&image_summary(source), dim_style(), width)),
            Block::ToolResult { .. } | Block::Other => {}
        }
    }
}

fn markdown(text: &str, width: usize) -> Vec<RenderedLine> {
    prose::render(&crate::markdown::parse(text), width)
}

fn one(text: &str, style: Style, width: usize) -> RenderedLine {
    let mut line = RenderedLine::blank();
    line.push(StyledSpan::new(truncate(text, width), style));
    line
}

fn thinking_summary(thinking: &str) -> String {
    let count = thinking.lines().filter(|line| !line.trim().is_empty()).count().max(1);
    let plural = if count == 1 { "line" } else { "lines" };
    format!("thinking{SEPARATOR}{count} {plural}")
}

fn tool_summary(name: &str, input: &Value) -> String {
    let summary = match input {
        Value::Null => String::new(),
        Value::Object(fields) if fields.is_empty() => String::new(),
        other => serde_json::to_string(other).unwrap_or_default(),
    };
    let summary = summary.replace(['\n', '\r', '\t'], " ");
    if summary.is_empty() { format!("{TOOL_MARKER}{name}") } else { format!("{TOOL_MARKER}{name}  {summary}") }
}

fn image_summary(source: &ImageSource) -> String {
    let kind = source.media_type.as_deref().and_then(|media| media.rsplit('/').next()).filter(|kind| !kind.is_empty());
    let size = decoded_bytes(source.bytes).map(human_bytes);
    let parts: Vec<String> = std::iter::once("image".to_owned()).chain(kind.map(str::to_owned)).chain(size).collect();
    format!("[{}]", parts.join(SEPARATOR))
}

const fn decoded_bytes(base64_chars: usize) -> Option<usize> {
    if base64_chars == 0 {
        return None;
    }
    match base64_chars.checked_div(4) {
        Some(quads) => Some(quads.saturating_mul(3)),
        None => None,
    }
}

fn human_bytes(bytes: usize) -> String {
    let mut value = bytes;
    let mut unit = 0usize;
    let last = UNITS.len().saturating_sub(1);
    while value >= UNIT && unit < last {
        value = value.checked_div(UNIT).unwrap_or(0);
        unit = unit.saturating_add(1);
    }
    let suffix = UNITS.get(unit).copied().unwrap_or("B");
    if unit == 0 {
        return format!("{bytes} {suffix}");
    }
    let divisor = UNIT.checked_pow(u32::try_from(unit).unwrap_or(0)).unwrap_or(1);
    let tenths = bytes.saturating_mul(10).checked_div(divisor).unwrap_or(0);
    let whole = tenths.checked_div(10).unwrap_or(0);
    let fraction = tenths.checked_rem(10).unwrap_or(0);
    format!("{whole}.{fraction} {suffix}")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::domain::thread;

    fn rendered(lines: &[&str]) -> Vec<String> {
        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("session.jsonl");
        fs::write(&path, lines.join("\n") + "\n").expect("a written transcript");
        let conversation = thread::build(&path).expect("a built conversation");
        transcript(&conversation, 60).iter().map(RenderedLine::text).collect()
    }

    const HUMAN: &str = r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:00Z","origin":{"kind":"human"},"message":{"role":"user","content":"read the grid scanner back to me"}}"#;

    fn assistant(uuid: &str, parent: &str, content: &str) -> String {
        format!(
            r#"{{"type":"assistant","uuid":"{uuid}","parentUuid":"{parent}","sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:01Z","message":{{"id":"msg_{uuid}","model":"opus-5","role":"assistant","content":{content}}}}}"#
        )
    }

    #[test]
    fn a_human_turn_and_an_assistant_turn_each_get_a_gutter_and_a_label() {
        let lines = rendered(&[HUMAN, &assistant("a1", "u1", r#"[{"type":"text","text":"the array reads clean"}]"#)]);
        assert_eq!(
            lines,
            vec![
                "▎you".to_owned(),
                "read the grid scanner back to me".to_owned(),
                String::new(),
                "▎claude  opus-5".to_owned(),
                "the array reads clean".to_owned(),
            ]
        );
    }

    #[test]
    fn a_user_record_that_is_plumbing_is_not_a_turn() {
        let tool_result = r#"{"type":"user","uuid":"u2","parentUuid":null,"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:00Z","toolUseResult":{"stdout":"ok"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#;
        let meta = r#"{"type":"user","uuid":"u3","parentUuid":null,"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:00Z","isMeta":true,"message":{"role":"user","content":"caveat"}}"#;
        let peer = r#"{"type":"user","uuid":"u4","parentUuid":null,"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:00Z","origin":{"kind":"peer"},"message":{"role":"user","content":"from elsewhere"}}"#;
        assert!(rendered(&[tool_result, meta, peer]).is_empty());
    }

    #[test]
    fn a_compact_summary_is_not_rendered_as_a_giant_human_turn() {
        let summary = r#"{"type":"user","uuid":"u5","parentUuid":null,"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:00Z","isCompactSummary":true,"message":{"role":"user","content":"this session is being continued from"}}"#;
        assert!(rendered(&[summary]).is_empty());
    }

    #[test]
    fn an_attachment_is_excluded_from_the_transcript() {
        let attachment = r#"{"type":"attachment","uuid":"x1","parentUuid":"u1","sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:02Z","attachment":{"type":"date","date":"2026-01-05"}}"#;
        let lines = rendered(&[HUMAN, attachment]);
        assert_eq!(lines, vec!["▎you".to_owned(), "read the grid scanner back to me".to_owned()]);
    }

    #[test]
    fn a_thinking_block_collapses_to_one_dim_line_and_never_shows_its_signature() {
        let block = r#"[{"type":"thinking","thinking":"one\ntwo\nthree","signature":"EqoBCkgIBRABGAI..."}]"#;
        let lines = rendered(&[HUMAN, &assistant("a1", "u1", block)]);
        assert!(lines.contains(&"thinking · 3 lines".to_owned()), "{lines:?}");
        assert!(!lines.iter().any(|line| line.contains("EqoBCkgI")), "the signature leaked: {lines:?}");
    }

    #[test]
    fn a_tool_call_is_one_line_of_its_name_and_a_summary() {
        let block = r#"[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la"}}]"#;
        let lines = rendered(&[HUMAN, &assistant("a1", "u1", block)]);
        let call = lines.iter().find(|line| line.starts_with("▸ Bash")).expect("a tool call line");
        assert_eq!(call, r#"▸ Bash  {"command":"ls -la"}"#);
    }

    #[test]
    fn a_tool_call_summary_never_breaks_across_lines() {
        let block = r#"[{"type":"tool_use","id":"t1","name":"Write","input":{"content":"first\nsecond\nthird and a great deal more prose besides"}}]"#;
        let lines = rendered(&[HUMAN, &assistant("a1", "u1", block)]);
        let calls = lines.iter().filter(|line| line.starts_with("▸ ")).count();
        assert_eq!(calls, 1);
        assert!(lines.iter().all(|line| line.chars().count() <= 60), "{lines:?}");
    }

    #[test]
    fn an_image_names_its_type_and_decoded_size_without_decoding_anything() {
        let block = r#"[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"","redactedBytes":1960000}}]"#;
        let lines = rendered(&[HUMAN, &assistant("a1", "u1", block)]);
        assert!(lines.contains(&"[image · png · 1.4 MB]".to_owned()), "{lines:?}");
    }

    #[test]
    fn an_image_with_no_media_type_drops_that_segment_rather_than_leaving_a_gap() {
        assert_eq!(image_summary(&ImageSource { media_type: None, bytes: 400 }), "[image · 300 B]");
        assert_eq!(image_summary(&ImageSource { media_type: Some("image/jpeg".to_owned()), bytes: 0 }), "[image · jpeg]");
    }

    #[test]
    fn a_pasted_image_in_a_human_turn_is_named_too() {
        let human = r#"{"type":"user","uuid":"u9","parentUuid":null,"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:00Z","origin":{"kind":"human"},"message":{"role":"user","content":[{"type":"text","text":"look at this"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"","redactedBytes":6000}}]}}"#;
        let lines = rendered(&[human]);
        assert_eq!(lines, vec!["▎you".to_owned(), "look at this".to_owned(), "[image · png · 4.3 KB]".to_owned()]);
    }

    #[test]
    fn sizes_read_the_way_a_file_manager_reports_them() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(1_470_000), "1.4 MB");
    }

    #[test]
    fn prose_wraps_to_the_width_it_is_given() {
        let text = "the deflector array reads back one plate at a time and then reports";
        let block = format!(r#"[{{"type":"text","text":"{text}"}}]"#);
        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("session.jsonl");
        fs::write(&path, format!("{}\n{}\n", HUMAN, assistant("a1", "u1", &block))).expect("a written transcript");
        let conversation = thread::build(&path).expect("a built conversation");
        for line in transcript(&conversation, 24) {
            assert!(line.width() <= 24, "{:?}", line.text());
        }
    }
}
