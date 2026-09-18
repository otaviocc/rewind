//! A tool call as one dense line: a glyph, the tool's name, a digest of what it was asked to do,
//! and how it went.

use std::collections::HashSet;
use std::hash::BuildHasher;

use ratatui::style::Style;
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::domain::subagent::Agent;
use crate::domain::thread::Conversation;
use crate::domain::tool::{self, Hunk, Label, Outcome, Status};
use crate::render::line::{RenderedLine, StyledSpan, normalise, truncate};
use crate::render::{Ctx, Overflow};

const COLLAPSED: &str = "▸ ";
const EXPANDED: &str = "▾ ";
const SEPARATOR: &str = " · ";
const HEAD_GAP: &str = "  ";
const TAIL_GAP: usize = 1;
const LEAST_DIGEST: usize = 8;
const ENTER: &str = "⏎";
const RULE: &str = "─";
const MARK_GAP: &str = "   ";
const AGENT_TOOLS: [&str; 2] = ["Agent", "Task"];
const GUTTER: &str = "  ┃ ";
const GUTTER_BLANK: &str = "  ┃";
const FOLD: usize = 20;
const KEY_COLUMN: usize = 14;
const DIFF_TOOLS: [&str; 2] = ["Edit", "Write"];

pub struct Styles {
    pub glyph: Style,
    pub name: Style,
    pub digest: Style,
    pub muted: Style,
    pub error: Style,
    pub added: Style,
    pub removed: Style,
    pub context: Style,
    pub enter: Style,
    pub ok: Style,
}

pub fn spawned<'a>(conversation: &Conversation, ctx: &'a Ctx<'_>, id: &str, name: &str) -> Option<&'a Agent> {
    if !AGENT_TOOLS.contains(&name) {
        return None;
    }
    let result = conversation.result_of(id).and_then(|node| Outcome::of(node, id));
    let agent_id = result.and_then(|outcome| outcome.detail).and_then(|detail| detail.get("agentId")).and_then(Value::as_str);
    ctx.agents.spawned_by(id, agent_id)
}

pub fn call(
    conversation: &Conversation,
    ctx: &Ctx<'_>,
    id: &str,
    name: &str,
    input: &Value,
    styles: &Styles,
) -> Vec<RenderedLine> {
    let outcome = conversation.result_of(id).and_then(|node| Outcome::of(node, id));
    let detail = outcome.and_then(|outcome| outcome.detail);
    let status = tool::status(outcome.as_ref());
    let agent = spawned(conversation, ctx, id, name);
    let digest = agent.map_or_else(
        || {
            let digest = tool::digest(name, input, detail);
            joined(digest.primary.as_deref(), digest.secondary.as_deref())
        },
        |agent| joined(Some(agent.label()), agent.description.as_deref()),
    );
    let expanded = ctx.is_expanded(id);
    let glyph = if expanded { EXPANDED } else { COLLAPSED };
    let inline = AGENT_TOOLS.contains(&name) && conversation.inline_agent(id).is_some();
    let mark = (agent.is_some_and(Agent::enterable) || inline).then_some(ENTER);
    let mut lines = vec![line(glyph, name, &digest, mark, status, ctx.width, styles)];
    if expanded {
        lines.extend(body(ctx, id, name, input, outcome.as_ref(), styles));
    }
    lines
}

pub fn unreached<'a, S: BuildHasher>(ctx: &'a Ctx<'_>, reached: &HashSet<Box<str>, S>) -> Vec<&'a Agent> {
    ctx.agents.all().iter().filter(|agent| agent.parent.is_none() && agent.enterable() && !reached.contains(&agent.id)).collect()
}

pub fn unreached_header(agents: &[&Agent], width: usize, styles: &Styles) -> Vec<RenderedLine> {
    let count = agents.len();
    let what = if agents.iter().all(|agent| agent.is_forked_skill()) { "forked skill" } else { "agent" };
    let plural = if count == 1 { "" } else { "s" };
    vec![
        cut(&RULE.repeat(width), width, styles.muted),
        cut(&format!("{count} {what}{plural} ran with no spawning call"), width, styles.muted),
    ]
}

pub fn unreached_line(agent: &Agent, width: usize, styles: &Styles) -> RenderedLine {
    let kind = Some(&*agent.kind).filter(|kind| *kind != agent.label());
    let digest = joined(kind, agent.description.as_deref());
    line(COLLAPSED, agent.label(), &digest, Some(ENTER), Status::Ok, width, styles)
}

fn body(ctx: &Ctx<'_>, id: &str, name: &str, input: &Value, outcome: Option<&Outcome<'_>>, styles: &Styles) -> Vec<RenderedLine> {
    let inner = ctx.width.saturating_sub(GUTTER.width()).max(1);
    let mut lines = fields(input, inner, styles);
    let tail = output(ctx, id, name, outcome, inner, styles);
    if !lines.is_empty() && !tail.is_empty() {
        lines.push(RenderedLine::blank());
    }
    lines.extend(tail);
    for line in &mut lines {
        detrail(line);
        let glyph = if line.spans.is_empty() { GUTTER_BLANK } else { GUTTER };
        line.prefix(StyledSpan::new(glyph, styles.muted));
    }
    lines
}

fn detrail(line: &mut RenderedLine) {
    while let Some(last) = line.spans.last_mut() {
        let trimmed = last.text.trim_end();
        if trimmed.len() == last.text.len() {
            return;
        }
        if trimmed.is_empty() {
            line.spans.pop();
        } else {
            last.text = trimmed.to_owned();
            return;
        }
    }
}

fn fields(input: &Value, width: usize, styles: &Styles) -> Vec<RenderedLine> {
    let Some(fields) = input.as_object() else { return Vec::new() };
    fields.iter().map(|(key, value)| field(key, value, width, styles)).collect()
}

fn field(key: &str, value: &Value, width: usize, styles: &Styles) -> RenderedLine {
    let mut line = RenderedLine::blank();
    let column = KEY_COLUMN.min(width.saturating_sub(2));
    if column == 0 {
        line.push(StyledSpan::new(truncate(key, width), styles.muted));
        return line;
    }
    let key = truncate(key, column);
    let pad = column.saturating_sub(key.width()).saturating_add(1);
    line.push(StyledSpan::new(format!("{key}{}", " ".repeat(pad)), styles.muted));
    let room = width.saturating_sub(column).saturating_sub(1);
    line.push(StyledSpan::new(truncate(&flattened(value), room), styles.digest));
    line
}

fn flattened(value: &Value) -> String {
    value.as_str().map_or_else(|| serde_json::to_string(value).unwrap_or_default(), normalise).replace('\n', " ")
}

fn output(
    ctx: &Ctx<'_>,
    id: &str,
    name: &str,
    outcome: Option<&Outcome<'_>>,
    width: usize,
    styles: &Styles,
) -> Vec<RenderedLine> {
    let Some(outcome) = outcome else { return Vec::new() };
    if DIFF_TOOLS.contains(&name)
        && let Some(hunks) = outcome.detail.and_then(tool::patch)
    {
        return folded(diff(&hunks, width, styles), width, styles);
    }
    if let Some(found) = outcome.detail.and_then(tool::overflow) {
        match ctx.outputs.get(id) {
            Some(Overflow::Lines(lines)) => {
                return folded(lines.iter().map(|text| cut(text, width, styles.digest)).collect(), width, styles);
            }
            Some(Overflow::Pending) | None => {
                return vec![cut(&format!("reading {} …", found.name), width, styles.muted)];
            }
            Some(Overflow::Failed(error)) => return vec![cut(error, width, styles.error)],
        }
    }
    let body = outcome.body();
    let body = tool::without_preamble(&body);
    let style = if outcome.status().is_error() { styles.error } else { styles.digest };
    folded(normalise(body).lines().map(|text| cut(text, width, style)).collect(), width, styles)
}

fn diff(hunks: &[Hunk], width: usize, styles: &Styles) -> Vec<RenderedLine> {
    let mut lines = Vec::new();
    for hunk in hunks {
        let head = format!("@@ -{},{} +{},{} @@", hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines);
        lines.push(cut(&head, width, styles.muted));
        for text in &hunk.lines {
            let style = match text.as_bytes().first() {
                Some(b'+') => styles.added,
                Some(b'-') => styles.removed,
                _ => styles.context,
            };
            lines.push(cut(text, width, style));
        }
    }
    lines
}

fn folded(mut lines: Vec<RenderedLine>, width: usize, styles: &Styles) -> Vec<RenderedLine> {
    let Some(hidden) = lines.len().checked_sub(FOLD).filter(|hidden| *hidden > 0) else { return lines };
    lines.truncate(FOLD);
    let plural = if hidden == 1 { "line" } else { "lines" };
    lines.push(cut(&format!("… {hidden} more {plural}"), width, styles.muted));
    lines
}

fn cut(text: &str, width: usize, style: Style) -> RenderedLine {
    let mut line = RenderedLine::blank();
    line.push(StyledSpan::new(truncate(&normalise(text).replace('\n', " "), width), style));
    line
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

fn line(
    glyph: &str,
    name: &str,
    detail: &str,
    mark: Option<&str>,
    status: Status,
    width: usize,
    styles: &Styles,
) -> RenderedLine {
    let mut line = RenderedLine::blank();
    let heading = heading(name);
    let head_width = glyph.width().saturating_add(heading.width());
    let outcome = label_of(status);
    let mark_width = mark.map_or(0, |mark| mark.width().saturating_add(MARK_GAP.width()));
    let outcome_width = outcome.width().saturating_add(mark_width);

    if head_width.saturating_add(TAIL_GAP).saturating_add(outcome.width()) > width {
        line.push(StyledSpan::new(truncate(&format!("{glyph}{heading}"), width), styles.name));
        return line;
    }
    line.push(StyledSpan::new(glyph, styles.glyph));
    line.push(StyledSpan::new(heading, styles.name));

    let affordable = head_width.saturating_add(TAIL_GAP).saturating_add(outcome_width) <= width;
    let mark = mark.filter(|_| affordable);
    let outcome_width = if affordable { outcome_width } else { outcome.width() };

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
    if let Some(mark) = mark {
        line.push(StyledSpan::new(format!("{mark}{MARK_GAP}"), styles.enter));
    }
    line.push(StyledSpan::new(outcome, if status.is_error() { styles.error } else { styles.ok }));
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
            added: Style::new().fg(Color::Green),
            removed: Style::new().fg(Color::Red),
            context: Style::new(),
            enter: Style::new().add_modifier(Modifier::BOLD),
            ok: Style::new().fg(Color::DarkGray),
        }
    }

    fn rendered(name: &str, detail: &str, status: Status, width: usize) -> String {
        line(COLLAPSED, name, detail, None, status, width, &styles()).text()
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
    fn a_body_line_is_right_trimmed_so_no_gutter_or_diff_context_leaves_whitespace() {
        let mut line = RenderedLine::blank();
        line.push(StyledSpan::new(" use crate::engine::grid::Cell;  ", Style::new()));
        line.push(StyledSpan::new("   ", Style::new()));
        detrail(&mut line);
        assert_eq!(line.text(), " use crate::engine::grid::Cell;", "the leading column survives, the tail does not");

        let mut blank = RenderedLine::blank();
        blank.push(StyledSpan::new(" ", Style::new()));
        detrail(&mut blank);
        assert!(blank.spans.is_empty(), "a blank diff context line becomes a blank body line");
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
            line(COLLAPSED, "Bash", "x", None, status, 40, &styles())
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
