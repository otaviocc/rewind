//! A tool call as a two-row head: a name line with a glyph and the outcome, and a dimmed digest
//! of what it was asked to do indented beneath it.

use std::collections::HashSet;
use std::hash::BuildHasher;
use std::path::Path;

use ratatui::style::Style;
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::domain::subagent::Agent;
use crate::domain::thread::Conversation;
use crate::domain::tool::{self, Hunk, Label, Outcome, Status};
use crate::render::line::{RenderedLine, StyledSpan, normalise, truncate, wrap};
use crate::render::prose;
use crate::render::{Ctx, Overflow};
use crate::theme::Theme;

pub(super) const COLLAPSED: &str = "▸ ";
pub(super) const EXPANDED: &str = "▾ ";
const SEPARATOR: &str = " · ";
const TAIL_GAP: usize = 1;
const LEAST_DIGEST: usize = 8;
const LEAST_VALUE: usize = 24;
const ENTER: &str = "⏎";
const RULE: &str = "─";
const MARK_GAP: &str = "   ";
const AGENT_TOOLS: [&str; 2] = ["Agent", "Task"];
pub(super) const GUTTER: &str = "  ┃ ";
pub(super) const GUTTER_BLANK: &str = "  ┃";
pub(super) const FOLD: usize = 20;
const KEY_COLUMN: usize = 14;
const DIFF_TOOLS: [&str; 2] = ["Edit", "Write"];
const DIGEST_INDENT: &str = "  └ ";

pub struct Styles {
    pub body: Style,
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

pub(super) struct RowStyles {
    pub body: Style,
    pub name: Style,
    pub digest: Style,
    pub muted: Style,
    pub enter: Style,
}

impl Styles {
    pub(super) const fn rows(&self) -> RowStyles {
        RowStyles { body: self.body, name: self.name, digest: self.digest, muted: self.muted, enter: self.enter }
    }
}

pub fn spawned<'a>(conversation: &Conversation, ctx: &'a Ctx<'_>, id: &str, name: &str) -> Option<&'a Agent> {
    if !AGENT_TOOLS.contains(&name) {
        return None;
    }
    let result = conversation.result_of(id).and_then(|node| Outcome::of(node, id));
    let agent_id = result.and_then(|outcome| outcome.detail).and_then(|detail| detail.get("agentId")).and_then(Value::as_str);
    ctx.agents.spawned_by(id, agent_id)
}

pub struct Call {
    pub lines: Vec<RenderedLine>,
    pub head: usize,
}

pub fn call(
    conversation: &Conversation,
    ctx: &Ctx<'_>,
    id: &str,
    name: &str,
    input: &Value,
    repeat: bool,
    styles: &Styles,
) -> Call {
    let outcome = conversation.result_of(id).and_then(|node| Outcome::of(node, id));
    let detail = outcome.and_then(|outcome| outcome.detail);
    let status = tool::status(outcome.as_ref());
    let agent = spawned(conversation, ctx, id, name);
    let digest = agent.map_or_else(
        || {
            let digest = tool::digest(name, input, detail);
            let primary = if tool::primary_is_path(name) {
                shortened(digest.primary.as_deref(), conversation.cwd())
            } else {
                digest.primary.as_deref()
            };
            joined(primary, digest.secondary.as_deref())
        },
        |agent| joined(Some(agent.label()), agent.description.as_deref()),
    );
    let expanded = ctx.is_expanded(id);
    let glyph = if expanded { EXPANDED } else { COLLAPSED };
    let inline = AGENT_TOOLS.contains(&name) && conversation.inline_agent(id).is_some();
    let readable = tool::plan_text(name, input).is_some();
    let mark = (agent.is_some_and(Agent::enterable) || inline || readable).then_some(ENTER);
    let word = label_of(status);
    let outcome_mark = Some((word, if status.is_error() { styles.error } else { styles.ok }));
    let rows = styles.rows();
    let under = repeat && !expanded && mark.is_none() && word.is_empty();
    let mut lines = under
        .then(|| digest_row(&digest, ctx.width, &rows).map(|row| vec![row]))
        .flatten()
        .unwrap_or_else(|| head(glyph, name, &digest, mark, outcome_mark, ctx.width, &rows));
    let head = lines.len();
    if expanded {
        lines.extend(body(ctx, id, name, input, outcome.as_ref(), styles));
    }
    Call { lines, head }
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

pub fn unreached_line(agent: &Agent, width: usize, styles: &Styles) -> Vec<RenderedLine> {
    let kind = Some(&*agent.kind).filter(|kind| *kind != agent.label());
    let digest = joined(kind, agent.description.as_deref());
    head(COLLAPSED, agent.label(), &digest, Some(ENTER), None, width, &styles.rows())
}

fn body(ctx: &Ctx<'_>, id: &str, name: &str, input: &Value, outcome: Option<&Outcome<'_>>, styles: &Styles) -> Vec<RenderedLine> {
    let inner = ctx.width.saturating_sub(GUTTER.width()).max(1);
    let plan = tool::plan_text(name, input);
    let mut lines = fields(input, if plan.is_some() { tool::PLAN_KEY } else { "" }, inner, styles);
    if let Some(plan) = plan {
        let document = document(plan, inner, ctx.theme, styles.muted);
        if !lines.is_empty() && !document.is_empty() {
            lines.push(RenderedLine::blank());
        }
        lines.extend(document);
    }
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

pub(super) fn detrail(line: &mut RenderedLine) {
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

fn fields(input: &Value, skip: &str, width: usize, styles: &Styles) -> Vec<RenderedLine> {
    let Some(fields) = input.as_object() else { return Vec::new() };
    fields.iter().filter(|(key, _)| key.as_str() != skip).flat_map(|(key, value)| field(key, value, width, styles)).collect()
}

fn document(text: &str, width: usize, theme: &Theme, muted: Style) -> Vec<RenderedLine> {
    let (kept, hidden) = fold_prose(text);
    let mut lines = prose::render(&crate::markdown::parse(&kept.join("\n")), width, theme);
    if let Some(hidden) = hidden {
        lines.push(more(hidden, width, muted));
    }
    lines
}

fn field(key: &str, value: &Value, width: usize, styles: &Styles) -> Vec<RenderedLine> {
    let column = KEY_COLUMN.min(width.saturating_sub(2));
    if column == 0 {
        let mut line = RenderedLine::blank();
        line.push(StyledSpan::new(truncate(key, width), styles.muted));
        return vec![line];
    }
    let label = truncate(key, column);
    let pad = column.saturating_sub(label.width()).saturating_add(1);
    let gutter = format!("{label}{}", " ".repeat(pad));
    let indent = " ".repeat(gutter.width());
    let room = width.saturating_sub(column).saturating_sub(1).max(1);
    let mut lines =
        if room < LEAST_VALUE { vec![cut(&flattened(value), room, styles.body)] } else { value_lines(value, room, styles.body) };
    if lines.is_empty() {
        lines.push(RenderedLine::blank());
    }
    for (index, line) in lines.iter_mut().enumerate() {
        let head = if index == 0 { gutter.clone() } else { indent.clone() };
        line.prefix(StyledSpan::new(head, styles.muted));
    }
    lines
}

fn flattened(value: &Value) -> String {
    value.as_str().map_or_else(|| serde_json::to_string(value).unwrap_or_default(), normalise).replace('\n', " ")
}

fn value_lines(value: &Value, width: usize, style: Style) -> Vec<RenderedLine> {
    value.as_str().map_or_else(
        || vec![cut(&serde_json::to_string(value).unwrap_or_default(), width, style)],
        |text| wrap(text.trim_end(), style, width),
    )
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
        return folded(diff(&hunks, width, styles), width, styles.muted);
    }
    if let Some(found) = outcome.detail.and_then(tool::overflow) {
        match ctx.outputs.get(id) {
            Some(Overflow::Lines(lines)) => {
                return folded(lines.iter().map(|text| cut(text, width, styles.body)).collect(), width, styles.muted);
            }
            Some(Overflow::Pending) | None => {
                return vec![cut(&format!("reading {} …", found.name), width, styles.muted)];
            }
            Some(Overflow::Failed(error)) => return vec![cut(error, width, styles.error)],
        }
    }
    let body = outcome.body();
    let body = tool::without_preamble(&body);
    let style = if outcome.status().is_error() { styles.error } else { styles.body };
    let normalised = normalise(body);
    let (kept, hidden) = fold_source(normalised.lines());
    let mut lines: Vec<RenderedLine> = kept.into_iter().flat_map(|text| wrap(text, style, width)).collect();
    if let Some(hidden) = hidden {
        lines.push(more(hidden, width, styles.muted));
    }
    lines
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

pub(super) fn folded(mut lines: Vec<RenderedLine>, width: usize, muted: Style) -> Vec<RenderedLine> {
    let Some(hidden) = lines.len().checked_sub(FOLD).filter(|hidden| *hidden > 0) else { return lines };
    lines.truncate(FOLD);
    lines.push(more(hidden, width, muted));
    lines
}

fn fold_prose(text: &str) -> (Vec<&str>, Option<usize>) {
    let mut kept = Vec::new();
    let mut taken = 0usize;
    let mut hidden = 0usize;
    for line in text.lines() {
        let blank = line.trim().is_empty();
        if taken >= FOLD {
            if !blank {
                hidden = hidden.saturating_add(1);
            }
            continue;
        }
        if !blank {
            taken = taken.saturating_add(1);
        }
        kept.push(line);
    }
    (kept, (hidden > 0).then_some(hidden))
}

fn fold_source<'a>(lines: impl Iterator<Item = &'a str>) -> (Vec<&'a str>, Option<usize>) {
    let mut kept: Vec<&str> = lines.collect();
    let hidden = kept.len().checked_sub(FOLD).filter(|hidden| *hidden > 0);
    if hidden.is_some() {
        kept.truncate(FOLD);
    }
    (kept, hidden)
}

fn more(hidden: usize, width: usize, muted: Style) -> RenderedLine {
    let plural = if hidden == 1 { "line" } else { "lines" };
    cut(&format!("… {hidden} more {plural}"), width, muted)
}

pub(super) fn cut(text: &str, width: usize, style: Style) -> RenderedLine {
    let mut line = RenderedLine::blank();
    line.push(StyledSpan::new(truncate(&normalise(text).replace('\n', " "), width), style));
    line
}

fn shortened<'a>(primary: Option<&'a str>, cwd: Option<&Path>) -> Option<&'a str> {
    let text = primary?;
    let Some(cwd) = cwd else { return Some(text) };
    let relative = Path::new(text).strip_prefix(cwd).ok().and_then(Path::to_str).filter(|rest| !rest.is_empty());
    Some(relative.unwrap_or(text))
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

pub(super) fn head(
    glyph: &str,
    name: &str,
    digest: &str,
    mark: Option<&str>,
    outcome: Option<(&str, Style)>,
    width: usize,
    styles: &RowStyles,
) -> Vec<RenderedLine> {
    let mut lines = vec![name_line(glyph, name, mark, outcome, width, styles)];
    if let Some(row) = digest_row(digest, width, styles) {
        lines.push(row);
    }
    lines
}

fn name_line(
    glyph: &str,
    name: &str,
    mark: Option<&str>,
    outcome: Option<(&str, Style)>,
    width: usize,
    styles: &RowStyles,
) -> RenderedLine {
    let mut line = RenderedLine::blank();
    let heading = heading(name);
    let head_width = glyph.width().saturating_add(heading.width());
    let (outcome_text, outcome_style) = outcome.unwrap_or_else(|| ("", Style::default()));

    if head_width.saturating_add(TAIL_GAP).saturating_add(outcome_text.width()) > width {
        line.push(StyledSpan::new(truncate(&format!("{glyph}{heading}"), width), styles.name));
        return line;
    }
    line.push(StyledSpan::new(glyph, styles.body));
    line.push(StyledSpan::new(heading, styles.name));

    if outcome_text.is_empty() && mark.is_none() {
        return line;
    }

    let gap = if outcome_text.is_empty() { "" } else { MARK_GAP };
    let mark_width = mark.map_or(0, |mark| mark.width().saturating_add(gap.width()));
    let outcome_width = outcome_text.width().saturating_add(mark_width);
    let affordable = head_width.saturating_add(TAIL_GAP).saturating_add(outcome_width) <= width;
    let mark = mark.filter(|_| affordable);
    if outcome_text.is_empty() && mark.is_none() {
        return line;
    }
    let outcome_width = if affordable { outcome_width } else { outcome_text.width() };

    let pad = width.saturating_sub(head_width).saturating_sub(outcome_width).max(TAIL_GAP);
    line.push(StyledSpan::new(" ".repeat(pad), styles.muted));
    if let Some(mark) = mark {
        line.push(StyledSpan::new(format!("{mark}{gap}"), styles.enter));
    }
    if !outcome_text.is_empty() {
        line.push(StyledSpan::new(outcome_text, outcome_style));
    }
    line
}

pub(super) fn digest_row(digest: &str, width: usize, styles: &RowStyles) -> Option<RenderedLine> {
    if digest.is_empty() {
        return None;
    }
    let room = width.saturating_sub(DIGEST_INDENT.width());
    if room < LEAST_DIGEST {
        return None;
    }
    let mut line = RenderedLine::blank();
    line.push(StyledSpan::new(DIGEST_INDENT, styles.muted));
    line.push(StyledSpan::new(truncate(digest, room), styles.digest));
    Some(line)
}

fn heading(name: &str) -> String {
    match tool::label(name) {
        Label::Plain(name) => name.to_owned(),
        Label::Mcp { server, tool } => format!("{server}{SEPARATOR}{tool}"),
    }
}

const fn label_of(status: Status) -> &'static str {
    match status {
        Status::Ok => "",
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
            body: Style::new(),
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

    fn field_rows(key: &str, value: &Value, width: usize) -> Vec<String> {
        field(key, value, width, &styles()).iter().map(RenderedLine::text).collect()
    }

    #[test]
    fn a_multi_line_input_value_keeps_every_row_under_the_key_column() {
        let value = Value::String("//! One coil.\n\npub struct Coil {\n    pub gain: f32,\n}".to_owned());
        let rows = field_rows("content", &value, 60);
        assert_eq!(rows.first().map(String::as_str), Some("content        //! One coil."));
        assert_eq!(rows.get(2).map(String::as_str), Some("               pub struct Coil {"));
        assert!(rows.iter().all(|row| !row.contains('…')), "a wrapped value is never cut: {rows:?}");
    }

    #[test]
    fn a_long_input_value_continues_rather_than_ending_in_an_ellipsis() {
        let value = Value::String("the deflector coil gain is nominal across every emitter on deck nine".to_owned());
        let rows = field_rows("note", &value, 40);
        assert!(rows.len() > 1, "{rows:?}");
        assert!(rows.iter().all(|row| row.width() <= 40), "{rows:?}");
        assert!(!rows.concat().contains('…'), "{rows:?}");
    }

    #[test]
    fn an_input_value_too_narrow_to_wrap_is_cut_on_one_row_instead_of_shredded() {
        let value = Value::String("/Users/fixture/Developer/holodeck/src/engine/deflector.rs".to_owned());
        let rows = field_rows("file_path", &value, 32);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(rows[0].ends_with('…'), "{:?}", rows[0]);
    }

    #[test]
    fn a_structured_input_value_stays_on_one_row() {
        let value = serde_json::json!({"deck": 9, "verbose": true});
        let rows = field_rows("filter", &value, 60);
        assert_eq!(rows.len(), 1, "{rows:?}");
    }

    #[test]
    fn the_fold_counts_the_lines_the_output_actually_has_not_the_rows_it_wraps_to() {
        let source = "a line that is long enough to need wrapping at this width\n".repeat(FOLD + 3);
        let (kept, hidden) = fold_source(source.lines());
        assert_eq!(kept.len(), FOLD);
        assert_eq!(hidden, Some(3));
    }

    #[test]
    fn a_diff_line_is_still_cut_rather_than_wrapped() {
        let hunks = vec![Hunk {
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            lines: vec!["+const COILS: usize = 12; // enough to hold the deflector array steady".to_owned()],
        }];
        let rows: Vec<String> = diff(&hunks, 40, &styles()).iter().map(RenderedLine::text).collect();
        assert_eq!(rows.len(), 2, "one header and one cut line: {rows:?}");
        assert!(rows[1].ends_with('…'), "{:?}", rows[1]);
    }

    fn plan_rows(text: &str, width: usize) -> Vec<String> {
        document(text, width, &Theme::default(), styles().muted).iter().map(RenderedLine::text).collect()
    }

    #[test]
    fn a_plan_renders_as_a_document_rather_than_as_one_squashed_field() {
        let rows = plan_rows("# Retune the array\n\n- Raise the coil count\n- Land it\n", 40);
        assert_eq!(rows.first().map(String::as_str), Some("Retune the array"));
        assert!(rows.iter().any(|row| row.starts_with("• Raise the coil count")), "{rows:?}");
        assert!(rows.iter().any(|row| row.starts_with("• Land it")), "{rows:?}");
    }

    #[test]
    fn a_plan_longer_than_the_fold_says_how_much_it_is_holding_back() {
        let text = "a line of the plan\n".repeat(FOLD + 5);
        let rows = plan_rows(&text, 60);
        assert_eq!(rows.last().map(String::as_str), Some("… 5 more lines"));
    }

    #[test]
    fn the_blank_lines_markdown_needs_between_blocks_do_not_count_against_a_plans_fold() {
        let text = "a line of the plan\n\n".repeat(FOLD);
        let (kept, hidden) = fold_prose(&text);
        assert_eq!(kept.iter().filter(|line| !line.trim().is_empty()).count(), FOLD);
        assert_eq!(hidden, None, "a plan of exactly FOLD content lines is shown whole");
    }

    #[test]
    fn a_folded_plan_counts_the_same_lines_its_digest_does() {
        let text = "# Retune the array\n\n".to_owned() + &"a line of the plan\n\n".repeat(FOLD + 3);
        let (_, hidden) = fold_prose(&text);
        assert_eq!(hidden, Some(4), "the title and FOLD content lines are shown, four content lines are not");
    }

    fn rendered(name: &str, detail: &str, status: Status, width: usize) -> Vec<String> {
        let styles = styles();
        let outcome = Some((label_of(status), if status.is_error() { styles.error } else { styles.ok }));
        head(COLLAPSED, name, detail, None, outcome, width, &styles.rows()).iter().map(RenderedLine::text).collect()
    }

    #[test]
    fn a_collapsed_call_is_its_name_on_one_row_and_its_digest_dimmed_on_the_next() {
        let rows = rendered("Bash", "wc -l src/engine/grid.rs", Status::Failed, 60);
        assert!(rows[0].starts_with("▸ Bash") && rows[0].ends_with("failed"), "{:?}", rows[0]);
        assert_eq!(rows[0].width(), 60, "the outcome ends on the last column");
        assert_eq!(rows.get(1).map(String::as_str), Some("  └ wc -l src/engine/grid.rs"));
    }

    #[test]
    fn a_call_with_no_digest_stays_one_row() {
        assert_eq!(rendered("Bash", "", Status::Ok, 60).len(), 1);
    }

    #[test]
    fn an_mcp_name_reads_as_a_server_and_a_tool() {
        assert!(rendered("mcp__jeffries__beam_status", "", Status::Denied, 60)[0].starts_with("▸ jeffries · beam_status"));
    }

    #[test]
    fn the_name_row_never_carries_the_digest() {
        let rows = rendered("Bash", "cargo build --release", Status::Failed, 16);
        assert_eq!(rows[0], "▸ Bash    failed");
        assert_eq!(rows[0].width(), 16);
    }

    #[test]
    fn the_digest_row_drops_out_when_the_column_is_too_narrow_for_it() {
        let rows = rendered("Bash", "cargo build --release", Status::Ok, 10);
        assert_eq!(rows.len(), 1, "narrower than DIGEST_INDENT plus LEAST_DIGEST leaves no room for a second row");
    }

    #[test]
    fn a_column_too_narrow_for_even_the_outcome_keeps_the_name() {
        let rows = rendered("mcp__jeffries__beam_status", "", Status::Ok, 12);
        assert_eq!(rows[0], "▸ jeffries …", "the name is what is left");
        assert!(rows[0].width() <= 12);
    }

    #[test]
    fn no_head_row_ends_in_whitespace_at_any_width() {
        for width in 1..=120_usize {
            for status in [Status::Ok, Status::Failed, Status::Denied, Status::Interrupted, Status::Pending] {
                let styles = styles();
                let outcome = Some((label_of(status), styles.ok));
                let marked = name_line(COLLAPSED, "Agent", Some(ENTER), outcome, width, &styles.rows()).text();
                assert_eq!(marked.trim_end(), marked, "width {width} left trailing whitespace: {marked:?}");
                assert!(marked.width() <= width, "width {width} overflowed to {}", marked.width());
                for text in rendered("mcp__jeffries__beam_status", "cargo build -v 2>&1", status, width) {
                    assert_eq!(text.trim_end(), text, "width {width} left trailing whitespace: {text:?}");
                    assert!(text.width() <= width, "width {width} overflowed to {}", text.width());
                }
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
    fn a_call_that_worked_says_nothing_at_all_on_the_right_edge() {
        let rows = rendered("Read", "src/engine/grid.rs · 212 lines", Status::Ok, 60);
        assert_eq!(rows[0], "\u{25b8} Read", "a call that worked is just its name");
        assert_eq!(rows.get(1).map(String::as_str), Some("  \u{2514} src/engine/grid.rs \u{b7} 212 lines"));
    }

    #[test]
    fn a_call_that_worked_and_can_be_entered_ends_on_the_enter_mark() {
        let styles = styles();
        let line = name_line(COLLAPSED, "Agent", Some(ENTER), Some((label_of(Status::Ok), styles.ok)), 40, &styles.rows());
        let text = line.text();
        assert_eq!(text.trim_end(), text, "the mark gap is only earned by a word after it: {text:?}");
        assert!(text.ends_with(ENTER), "{text:?}");
        assert_eq!(text.width(), 40);
    }

    #[test]
    fn only_the_calls_worth_a_word_get_one() {
        for status in [Status::Failed, Status::Denied, Status::Interrupted, Status::Pending] {
            assert!(!label_of(status).is_empty(), "{status:?} is worth saying");
        }
        assert!(label_of(Status::Ok).is_empty(), "a call that worked is not news");
    }

    #[test]
    fn a_path_under_the_working_directory_is_shown_relative_to_it() {
        let cwd = Path::new("/Users/fixture/Developer/holodeck");
        assert_eq!(
            shortened(Some("/Users/fixture/Developer/holodeck/src/engine/grid.rs"), Some(cwd)),
            Some("src/engine/grid.rs")
        );
    }

    #[test]
    fn a_path_outside_the_working_directory_is_left_alone() {
        let cwd = Path::new("/Users/fixture/Developer/holodeck");
        let elsewhere = "/etc/hosts";
        assert_eq!(shortened(Some(elsewhere), Some(cwd)), Some(elsewhere));
    }

    #[test]
    fn a_sibling_sharing_the_first_letters_is_not_mistaken_for_the_working_directory() {
        let cwd = Path::new("/Users/fixture/Developer/holodeck");
        let sibling = "/Users/fixture/Developer/holodeck-2/src/engine/grid.rs";
        assert_eq!(shortened(Some(sibling), Some(cwd)), Some(sibling), "a prefix must end on a component");
    }

    #[test]
    fn the_working_directory_itself_stays_absolute_rather_than_shortening_to_nothing() {
        let cwd = Path::new("/Users/fixture/Developer/holodeck");
        assert_eq!(shortened(Some("/Users/fixture/Developer/holodeck"), Some(cwd)), Some("/Users/fixture/Developer/holodeck"));
    }

    #[test]
    fn a_session_with_no_working_directory_shortens_nothing() {
        assert_eq!(
            shortened(Some("/Users/fixture/Developer/holodeck/src/main.rs"), None),
            Some("/Users/fixture/Developer/holodeck/src/main.rs")
        );
        assert_eq!(shortened(None, Some(Path::new("/Users/fixture"))), None);
    }

    #[test]
    fn a_digest_with_both_halves_joins_them_with_the_separator() {
        assert_eq!(joined(Some("Explore"), Some("Trace the grid scanner")), "Explore · Trace the grid scanner");
        assert_eq!(joined(None, Some("212 lines")), "212 lines");
        assert_eq!(joined(None, None), "");
    }

    #[test]
    fn only_the_three_error_outcomes_are_painted_in_the_error_style() {
        let error = |status: Status| {
            let styles = styles();
            let outcome = Some((label_of(status), if status.is_error() { styles.error } else { styles.ok }));
            name_line(COLLAPSED, "Bash", None, outcome, 40, &styles.rows())
                .spans
                .last()
                .map(|span| span.style)
                .is_some_and(|style| style == styles.error)
        };
        assert!(error(Status::Failed) && error(Status::Denied) && error(Status::Interrupted));
        assert!(!error(Status::Ok), "a call that worked is not an error");
        assert!(!error(Status::Pending), "a session that ended mid-call did not fail");
    }
}
