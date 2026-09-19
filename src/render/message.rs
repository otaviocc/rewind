//! One conversation becomes styled lines: a role rail, one header per run of replies, prose, a
//! two-row head per tool call, and the same shape for a local slash command and its result.

use std::collections::HashSet;

use ratatui::style::Style;

use crate::domain::block::{Block, Content, ImageSource};
use crate::domain::command::Command as SlashCommand;
use crate::domain::record::CompactMetadata;
use crate::domain::thread::{AssistantTurn, Conversation, Node, NodeId, NodeKind};
use crate::render::line::{RenderedLine, StyledSpan, truncate};
use crate::render::prose;
use crate::render::{Ctx, command, divider, injection, tool};
use crate::theme::{Element, Theme};
use unicode_width::UnicodeWidthStr;

const RAIL: &str = "▎ ";
const RAIL_BLANK: &str = "▎";
const HUMAN_LABEL: &str = "you";
const PROMPT_LABEL: &str = "prompt";
const SUMMARY_LABEL: &str = "summary";
const ASSISTANT_LABEL: &str = "claude";
const MODEL_PREFIX: &str = "claude-";
const SEPARATOR: &str = " · ";
const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
const UNIT: usize = 1024;

fn tool_styles(theme: &Theme) -> tool::Styles {
    tool::Styles {
        body: theme.style(Element::Body),
        name: theme.style(Element::ToolName),
        digest: theme.style(Element::ToolSummary),
        muted: theme.style(Element::Muted),
        error: theme.style(Element::ToolError),
        added: theme.style(Element::DiffAdded),
        removed: theme.style(Element::DiffRemoved),
        context: theme.style(Element::DiffContext),
        enter: theme.style(Element::Subagent),
        ok: theme.style(Element::ToolOk),
    }
}

fn command_styles(theme: &Theme) -> tool::RowStyles {
    tool::RowStyles {
        body: theme.style(Element::Body),
        name: theme.style(Element::ToolName),
        digest: theme.style(Element::ToolSummary),
        muted: theme.style(Element::Muted),
        enter: theme.style(Element::Muted),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rail {
    Human,
    Assistant,
    Seam,
    Command,
}

impl Rail {
    const fn element(self) -> Element {
        match self {
            Self::Human | Self::Command => Element::HumanGutter,
            Self::Assistant => Element::AssistantGutter,
            Self::Seam => Element::CompactDivider,
        }
    }
}

struct Group {
    rail: Rail,
    model: Option<String>,
    turn: bool,
    quiet: bool,
    run: Option<Box<str>>,
    lines: Vec<RenderedLine>,
    anchors: Vec<Anchor>,
    spans: Vec<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub id: Box<str>,
    pub line: usize,
    pub head: usize,
    pub agent: Option<Box<str>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub node: NodeId,
    pub line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub node: NodeId,
    pub offset: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Transcript {
    pub lines: Vec<RenderedLine>,
    pub anchors: Vec<Anchor>,
    pub spans: Vec<Span>,
    pub turns: Vec<usize>,
}

impl Transcript {
    pub fn position(&self, line: usize) -> Option<Position> {
        let index = self.spans.partition_point(|span| span.line <= line).checked_sub(1)?;
        let span = self.spans.get(index)?;
        Some(Position { node: span.node, offset: line.saturating_sub(span.line) })
    }

    pub fn line_of(&self, position: Position) -> Option<usize> {
        let index = self.spans.iter().position(|span| span.node == position.node)?;
        let span = self.spans.get(index)?;
        let ceiling =
            self.spans.get(index.saturating_add(1)).map_or_else(|| self.lines.len(), |next| next.line).saturating_sub(1);
        Some(span.line.saturating_add(position.offset).min(ceiling.max(span.line)))
    }

    pub fn turn_after(&self, line: usize) -> Option<usize> {
        self.turns.get(self.turns.partition_point(|turn| *turn <= line)).copied()
    }

    pub fn turn_before(&self, line: usize) -> Option<usize> {
        self.turns.get(self.turns.partition_point(|turn| *turn < line).checked_sub(1)?).copied()
    }
}

pub fn transcript(conversation: &Conversation, ctx: &Ctx<'_>) -> Transcript {
    let inner = ctx.width.saturating_sub(RAIL.width()).max(1);
    let ctx = &ctx.narrowed(inner);
    let mut groups: Vec<Group> = Vec::new();

    let sidechain = ctx.root.is_some() || conversation.is_sidechain();
    let human = if sidechain { PROMPT_LABEL } else { HUMAN_LABEL };
    let walk =
        ctx.root.map_or_else(|| conversation.thread_with(ctx.branches), |root| conversation.path_from_with(root, ctx.branches));

    let mut injections: Vec<NodeId> = Vec::new();
    for (index, &id) in walk.iter().enumerate() {
        if matches!(conversation.node(id).map(|node| &node.kind), Some(NodeKind::Attachment(_))) {
            injections.push(id);
            continue;
        }
        flush(conversation, ctx, &mut injections, &mut groups, inner);
        let Some(node) = conversation.node(id) else { continue };
        let seam = seam(node, sidechain, inner, ctx.theme);
        match &node.kind {
            NodeKind::User(record) => {
                if let Some(command) = record.command() {
                    push_command(id, node.uuid(), command, seam, ctx, &mut groups);
                } else if record.is_human_turn() && !record.is_compact_summary {
                    let mut lines = Vec::from_iter(seam);
                    lines.push(header(human, None, inner, ctx.theme));
                    let mut anchors = Vec::new();
                    content(conversation, ctx, &record.message.content, &mut lines, &mut anchors);
                    let spans = vec![Span { node: id, line: 0 }];
                    groups.push(Group {
                        rail: Rail::Human,
                        model: None,
                        turn: true,
                        quiet: false,
                        run: None,
                        lines,
                        anchors,
                        spans,
                    });
                } else if record.is_compact_summary {
                    let mut lines = Vec::from_iter(seam);
                    lines.push(header(SUMMARY_LABEL, None, inner, ctx.theme));
                    let mut anchors = Vec::new();
                    content(conversation, ctx, &record.message.content, &mut lines, &mut anchors);
                    let spans = vec![Span { node: id, line: 0 }];
                    groups.push(Group {
                        rail: Rail::Seam,
                        model: None,
                        turn: true,
                        quiet: false,
                        run: None,
                        lines,
                        anchors,
                        spans,
                    });
                } else {
                    push_seam_only(id, seam, &mut groups);
                }
            }
            NodeKind::Assistant(turn) => {
                push_assistant(conversation, ctx, id, turn, seam, inner, &mut groups);
            }
            NodeKind::System(_) | NodeKind::Attachment(_) => {
                push_seam_only(id, seam, &mut groups);
            }
        }
        let on = walk.get(index.saturating_add(1)).copied();
        marker(conversation, id, on, &mut groups, inner, ctx.theme);
    }
    flush(conversation, ctx, &mut injections, &mut groups, inner);

    if sidechain {
        return flatten(groups, ctx.theme);
    }

    let reached: HashSet<Box<str>> =
        groups.iter().flat_map(|group| group.anchors.iter()).filter_map(|anchor| anchor.agent.clone()).collect();
    if let Some(tail) = unreached(ctx, inner, &reached) {
        groups.push(tail);
    }

    flatten(groups, ctx.theme)
}

fn push_assistant(
    conversation: &Conversation,
    ctx: &Ctx<'_>,
    id: NodeId,
    turn: &AssistantTurn,
    seam: Option<RenderedLine>,
    width: usize,
    groups: &mut Vec<Group>,
) {
    let merging = matches!(groups.last(), Some(group) if group.rail == Rail::Assistant && group.model == turn.model);
    let mut run = if merging && seam.is_none() { groups.last().and_then(|group| group.run.clone()) } else { None };
    let mut body = Vec::new();
    let mut anchors = Vec::new();
    blocks(conversation, ctx, &turn.content, &mut body, &mut anchors, &mut run);
    if body.is_empty() {
        return;
    }
    let quiet = quiet(ctx, &turn.content);
    match groups.last_mut() {
        Some(group) if merging => {
            if !(group.quiet && quiet && seam.is_none()) {
                group.lines.push(RenderedLine::blank());
            }
            let starts = group.lines.len();
            group.lines.extend(seam);
            let at = group.lines.len();
            shift(&mut anchors, at);
            group.lines.append(&mut body);
            group.anchors.append(&mut anchors);
            group.spans.push(Span { node: id, line: starts });
            group.quiet = quiet;
            group.run = run;
        }
        _ => {
            let detail = model_label(turn.model.as_deref());
            let mut lines = Vec::from_iter(seam);
            lines.push(header(ASSISTANT_LABEL, detail.as_deref(), width, ctx.theme));
            let at = lines.len();
            shift(&mut anchors, at);
            lines.append(&mut body);
            let spans = vec![Span { node: id, line: 0 }];
            let model = turn.model.clone();
            groups.push(Group { rail: Rail::Assistant, model, turn: true, quiet, run, lines, anchors, spans });
        }
    }
}

fn push_seam_only(id: NodeId, seam: Option<RenderedLine>, groups: &mut Vec<Group>) {
    let Some(line) = seam else { return };
    let spans = vec![Span { node: id, line: 0 }];
    let group = Group {
        rail: Rail::Seam,
        model: None,
        turn: false,
        quiet: false,
        run: None,
        lines: vec![line],
        anchors: Vec::new(),
        spans,
    };
    groups.push(group);
}

fn push_command(
    id: NodeId,
    uuid: &str,
    command: SlashCommand<'_>,
    seam: Option<RenderedLine>,
    ctx: &Ctx<'_>,
    groups: &mut Vec<Group>,
) {
    let styles = command_styles(ctx.theme);
    let (call, gap) = match command {
        SlashCommand::Invocation { name, args } => (self::command::invocation(uuid, name, args, ctx, &styles), true),
        SlashCommand::Output(text) => (self::command::output(uuid, text, ctx, &styles), false),
    };
    let merge = seam.is_none() && matches!(groups.last(), Some(group) if group.rail == Rail::Command);
    if merge {
        let Some(group) = groups.last_mut() else { return };
        if gap {
            group.lines.push(RenderedLine::blank());
        }
        let at = group.lines.len();
        group.anchors.push(Anchor { id: Box::from(uuid), line: at, head: call.head, agent: None });
        group.lines.extend(call.lines);
        group.spans.push(Span { node: id, line: at });
        return;
    }
    let mut lines = Vec::from_iter(seam);
    let at = lines.len();
    let anchors = vec![Anchor { id: Box::from(uuid), line: at, head: call.head, agent: None }];
    lines.extend(call.lines);
    let spans = vec![Span { node: id, line: 0 }];
    groups.push(Group { rail: Rail::Command, model: None, turn: true, quiet: false, run: None, lines, anchors, spans });
}

fn marker(conversation: &Conversation, id: NodeId, on: Option<NodeId>, groups: &mut [Group], width: usize, theme: &Theme) {
    let alternates = conversation.alternates(id);
    if alternates.len() < 2 {
        return;
    }
    let Some(node) = conversation.node(id) else { return };
    let Some(group) = groups.last_mut() else { return };
    let showing = on.and_then(|on| alternates.iter().position(|alternate| *alternate == on)).map_or(1, |at| at.saturating_add(1));
    let text = format!("{} alternate branches here{SEPARATOR}showing {showing}{SEPARATOR}[b] to switch", alternates.len());
    group.anchors.push(Anchor { id: Box::from(node.uuid()), line: group.lines.len(), head: 1, agent: None });
    group.lines.push(divider::line(&text, width, theme.style(Element::BranchMarker)));
    group.quiet = false;
    group.run = None;
}

fn flush(conversation: &Conversation, ctx: &Ctx<'_>, pending: &mut Vec<NodeId>, groups: &mut Vec<Group>, width: usize) {
    let run = std::mem::take(pending);
    if run.is_empty() || !ctx.injections {
        return;
    }
    let Some(first) = run.first().copied() else { return };
    let Some(key) = conversation.node(first).map(|node| Box::<str>::from(node.uuid())) else { return };
    let expanded = ctx.is_expanded(&key);
    let style = ctx.theme.style(Element::Injection);
    let mut lines = vec![injection::run(conversation, &run, expanded, width, style)];
    if expanded {
        lines.extend(injection::each(conversation, &run, width, style));
    }
    if let Some(group) = groups.last_mut() {
        group.anchors.push(Anchor { id: key, line: group.lines.len(), head: 1, agent: None });
        group.lines.extend(lines);
        group.quiet = false;
        group.run = None;
        return;
    }
    let anchors = vec![Anchor { id: key, line: 0, head: 1, agent: None }];
    let spans = vec![Span { node: first, line: 0 }];
    groups.push(Group { rail: Rail::Seam, model: None, turn: false, quiet: false, run: None, lines, anchors, spans });
}

fn seam(node: &Node, sidechain: bool, width: usize, theme: &Theme) -> Option<RenderedLine> {
    if sidechain {
        return None;
    }
    let label = divider::label(node.divider?, compact_metadata(node))?;
    Some(divider::line(&label, width, theme.style(Element::CompactDivider)))
}

const fn compact_metadata(node: &Node) -> Option<&CompactMetadata> {
    let NodeKind::System(record) = &node.kind else { return None };
    record.compact_metadata.as_ref()
}

fn flatten(groups: Vec<Group>, theme: &Theme) -> Transcript {
    let mut transcript = Transcript::default();
    for group in groups {
        if !transcript.lines.is_empty() {
            transcript.lines.push(RenderedLine::blank());
        }
        let mut anchors = group.anchors;
        let mut spans = group.spans;
        if group.turn {
            transcript.turns.push(transcript.lines.len());
        }
        shift(&mut anchors, transcript.lines.len());
        for span in &mut spans {
            span.line = span.line.saturating_add(transcript.lines.len());
        }
        transcript.anchors.append(&mut anchors);
        transcript.spans.append(&mut spans);
        transcript.lines.extend(railed(group.lines, theme.style(group.rail.element())));
    }
    transcript
}

fn unreached(ctx: &Ctx<'_>, width: usize, reached: &HashSet<Box<str>>) -> Option<Group> {
    let styles = tool_styles(ctx.theme);
    let agents = tool::unreached(ctx, reached);
    if agents.is_empty() {
        return None;
    }
    let mut lines = tool::unreached_header(&agents, width, &styles);
    let mut anchors = Vec::new();
    for agent in agents {
        let rows = tool::unreached_line(agent, width, &styles);
        anchors.push(Anchor { id: agent.id.clone(), line: lines.len(), head: rows.len(), agent: Some(agent.id.clone()) });
        lines.extend(rows);
    }
    Some(Group { rail: Rail::Assistant, model: None, turn: false, quiet: false, run: None, lines, anchors, spans: Vec::new() })
}

fn shift(anchors: &mut [Anchor], by: usize) {
    for anchor in anchors {
        anchor.line = anchor.line.saturating_add(by);
    }
}

fn railed(lines: Vec<RenderedLine>, style: Style) -> Vec<RenderedLine> {
    lines
        .into_iter()
        .map(|mut line| {
            let glyph = if line.is_blank() { RAIL_BLANK } else { RAIL };
            line.prefix(StyledSpan::new(glyph, style));
            line
        })
        .collect()
}

fn model_label(model: Option<&str>) -> Option<String> {
    let model = model.map(str::trim).filter(|model| !model.is_empty())?;
    Some(model.strip_prefix(MODEL_PREFIX).unwrap_or(model).to_owned())
}

fn header(label: &str, detail: Option<&str>, width: usize, theme: &Theme) -> RenderedLine {
    let mut line = RenderedLine::blank();
    line.push(StyledSpan::new(truncate(label, width), theme.style(Element::Label)));
    let left = width.saturating_sub(label.width()).saturating_sub(SEPARATOR.width());
    if let Some(detail) = detail.filter(|_| left > 0) {
        line.push(StyledSpan::new(format!("{SEPARATOR}{}", truncate(detail, left)), theme.style(Element::Muted)));
    }
    line
}

fn content(
    conversation: &Conversation,
    ctx: &Ctx<'_>,
    content: &Content,
    lines: &mut Vec<RenderedLine>,
    anchors: &mut Vec<Anchor>,
) {
    match content {
        Content::Text(text) => lines.extend(markdown(text, ctx.width, ctx.theme)),
        Content::Blocks(blocks_of) => blocks(conversation, ctx, blocks_of, lines, anchors, &mut None),
    }
}

fn blocks(
    conversation: &Conversation,
    ctx: &Ctx<'_>,
    blocks: &[Block],
    lines: &mut Vec<RenderedLine>,
    anchors: &mut Vec<Anchor>,
    run: &mut Option<Box<str>>,
) {
    let styles = tool_styles(ctx.theme);
    for block in blocks {
        match block {
            Block::Text { text } => {
                let prose = markdown(text, ctx.width, ctx.theme);
                if !prose.is_empty() {
                    *run = None;
                }
                lines.extend(prose);
            }
            Block::Thinking { thinking } => {
                lines.push(one(&thinking_summary(thinking), ctx.theme.style(Element::Thinking), ctx.width));
                *run = None;
            }
            Block::ToolUse { id, name, input } => {
                let agent = tool::spawned(conversation, ctx, id, name)
                    .filter(|agent| agent.enterable())
                    .map(|agent| agent.id.clone())
                    .or_else(|| conversation.inline_agent(id).map(|_| Box::from(id.as_str())));
                let repeat = run.as_deref() == Some(name.as_str());
                let call = tool::call(conversation, ctx, id, name, input, repeat, &styles);
                anchors.push(Anchor { id: Box::from(id.as_str()), line: lines.len(), head: call.head, agent });
                lines.extend(call.lines);
                *run = (!ctx.is_expanded(id)).then(|| Box::from(name.as_str()));
            }
            Block::Image { source } => {
                lines.push(one(&image_summary(source), ctx.theme.style(Element::Muted), ctx.width));
                *run = None;
            }
            Block::ToolResult { .. } | Block::Other { .. } => {}
        }
    }
}

fn markdown(text: &str, width: usize, theme: &Theme) -> Vec<RenderedLine> {
    prose::render(&crate::markdown::parse(text), width, theme)
}

fn one(text: &str, style: Style, width: usize) -> RenderedLine {
    let mut line = RenderedLine::blank();
    line.push(StyledSpan::new(truncate(text, width), style));
    line
}

fn quiet(ctx: &Ctx<'_>, blocks: &[Block]) -> bool {
    blocks.iter().all(|block| match block {
        Block::Text { text } => text.trim().is_empty(),
        Block::ToolUse { id, .. } => !ctx.is_expanded(id),
        _ => true,
    })
}

fn thinking_summary(thinking: &str) -> String {
    let count = thinking.lines().filter(|line| !line.trim().is_empty()).count().max(1);
    let plural = if count == 1 { "line" } else { "lines" };
    format!("thinking{SEPARATOR}{count} {plural}")
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
    use crate::render::Expanded;

    fn plain(width: usize) -> Ctx<'static> {
        static THEME: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();
        static EXPANDED: std::sync::OnceLock<crate::render::Expanded> = std::sync::OnceLock::new();
        static OUTPUTS: std::sync::OnceLock<crate::render::Outputs> = std::sync::OnceLock::new();
        static AGENTS: std::sync::OnceLock<crate::domain::subagent::Agents> = std::sync::OnceLock::new();
        static BRANCHES: std::sync::OnceLock<crate::render::Branches> = std::sync::OnceLock::new();
        Ctx {
            width,
            theme: THEME.get_or_init(Theme::default),
            expanded: EXPANDED.get_or_init(crate::render::Expanded::new),
            outputs: OUTPUTS.get_or_init(crate::render::Outputs::new),
            agents: AGENTS.get_or_init(crate::domain::subagent::Agents::default),
            root: None,
            branches: BRANCHES.get_or_init(crate::render::Branches::new),
            injections: false,
        }
    }

    fn revealing(width: usize) -> Ctx<'static> {
        Ctx { injections: true, ..plain(width) }
    }

    fn rendered(lines: &[&str]) -> Vec<String> {
        built(lines).lines.iter().map(RenderedLine::text).collect()
    }

    fn built(lines: &[&str]) -> Transcript {
        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("session.jsonl");
        fs::write(&path, lines.join("\n") + "\n").expect("a written transcript");
        let conversation = thread::build(&path).expect("a built conversation");
        transcript(&conversation, &plain(60))
    }

    fn headers(lines: &[String]) -> usize {
        lines.iter().filter(|line| line.contains(ASSISTANT_LABEL)).count()
    }

    const PARAGRAPHS: &str = r#"[{"type":"text","text":"one\n\ntwo"}]"#;

    const HUMAN: &str = r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:00Z","origin":{"kind":"human"},"message":{"role":"user","content":"read the grid scanner back to me"}}"#;

    fn assistant(uuid: &str, parent: &str, content: &str) -> String {
        format!(
            r#"{{"type":"assistant","uuid":"{uuid}","parentUuid":"{parent}","sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:01Z","message":{{"id":"msg_{uuid}","model":"opus-5","role":"assistant","content":{content}}}}}"#
        )
    }

    #[test]
    fn a_human_turn_and_an_assistant_turn_each_get_a_rail_and_a_header() {
        let lines = rendered(&[HUMAN, &assistant("a1", "u1", r#"[{"type":"text","text":"the array reads clean"}]"#)]);
        assert_eq!(
            lines,
            vec![
                "▎ you".to_owned(),
                "▎ read the grid scanner back to me".to_owned(),
                String::new(),
                "▎ claude · opus-5".to_owned(),
                "▎ the array reads clean".to_owned(),
            ]
        );
    }

    #[test]
    fn a_run_of_consecutive_replies_gets_one_header() {
        let lines = rendered(&[
            HUMAN,
            &assistant("a1", "u1", r#"[{"type":"text","text":"first"}]"#),
            &assistant("a2", "a1", r#"[{"type":"text","text":"second"}]"#),
            &assistant("a3", "a2", r#"[{"type":"text","text":"third"}]"#),
        ]);
        assert_eq!(
            lines,
            vec![
                "▎ you".to_owned(),
                "▎ read the grid scanner back to me".to_owned(),
                String::new(),
                "▎ claude · opus-5".to_owned(),
                "▎ first".to_owned(),
                "▎".to_owned(),
                "▎ second".to_owned(),
                "▎".to_owned(),
                "▎ third".to_owned(),
            ]
        );
    }

    #[test]
    fn an_attachment_between_two_replies_does_not_break_the_run() {
        let attachment = r#"{"type":"attachment","uuid":"x1","parentUuid":"a1","sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:02Z","attachment":{"type":"date","date":"2026-01-05"}}"#;
        let lines = rendered(&[
            HUMAN,
            &assistant("a1", "u1", r#"[{"type":"text","text":"first"}]"#),
            attachment,
            &assistant("a2", "x1", r#"[{"type":"text","text":"second"}]"#),
        ]);
        assert_eq!(headers(&lines), 1, "{lines:?}");
    }

    #[test]
    fn a_reply_that_renders_nothing_at_all_does_not_break_the_run_either() {
        let nothing = r#"[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]"#;
        let lines = rendered(&[
            HUMAN,
            &assistant("a1", "u1", r#"[{"type":"text","text":"first"}]"#),
            &assistant("a2", "a1", nothing),
            &assistant("a3", "a2", r#"[{"type":"text","text":"second"}]"#),
        ]);
        assert_eq!(headers(&lines), 1, "{lines:?}");
    }

    #[test]
    fn a_change_of_model_starts_a_new_header() {
        let second = r#"{"type":"assistant","uuid":"a2","parentUuid":"a1","sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:02Z","message":{"id":"msg_a2","model":"haiku-4-5","role":"assistant","content":[{"type":"text","text":"second"}]}}"#;
        let lines = rendered(&[HUMAN, &assistant("a1", "u1", r#"[{"type":"text","text":"first"}]"#), second]);
        assert_eq!(headers(&lines), 2, "{lines:?}");
        assert!(lines.contains(&"▎ claude · opus-5".to_owned()), "{lines:?}");
        assert!(lines.contains(&"▎ claude · haiku-4-5".to_owned()), "{lines:?}");
    }

    #[test]
    fn the_claude_prefix_is_dropped_from_a_model_name_it_would_only_repeat() {
        assert_eq!(model_label(Some("claude-opus-5")).as_deref(), Some("opus-5"));
        assert_eq!(model_label(Some("opus-5")).as_deref(), Some("opus-5"));
        assert_eq!(model_label(Some("  ")), None);
        assert_eq!(model_label(None), None);
    }

    #[test]
    fn the_rail_reaches_every_line_of_a_turn_including_the_blank_ones() {
        let lines = rendered(&[HUMAN, &assistant("a1", "u1", PARAGRAPHS)]);
        assert!(lines.contains(&"▎".to_owned()), "no railed blank line in {lines:?}");
        assert!(
            lines.iter().all(|line| line.is_empty() || line.starts_with('▎')),
            "a line of a turn is missing its rail: {lines:?}"
        );
    }

    #[test]
    fn a_railed_blank_line_carries_no_trailing_space() {
        let lines = rendered(&[HUMAN, &assistant("a1", "u1", PARAGRAPHS)]);
        assert!(lines.iter().all(|line| line.trim_end() == *line), "{lines:?}");
    }

    #[test]
    fn the_gap_between_turns_carries_no_rail_and_is_the_only_unrailed_line() {
        let lines = rendered(&[HUMAN, &assistant("a1", "u1", PARAGRAPHS)]);
        assert_eq!(lines.iter().filter(|line| line.is_empty()).count(), 1, "{lines:?}");
    }

    #[test]
    fn a_human_rail_and_an_assistant_rail_are_two_different_styles() {
        let theme = Theme::default();
        assert_ne!(theme.style(Rail::Human.element()), theme.style(Rail::Assistant.element()));
    }

    #[test]
    fn a_human_rail_and_an_assistant_rail_each_carry_a_foreground_but_never_a_background() {
        let theme = Theme::default();
        for rail in [Rail::Human, Rail::Assistant] {
            let style = theme.style(rail.element());
            assert!(style.fg.is_some(), "{rail:?} has to be findable against a plain terminal");
            assert_eq!(style.bg, None, "{rail:?} pins an absolute background");
        }
    }

    #[test]
    fn the_inset_counts_the_rail_a_line_actually_carries() {
        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("session.jsonl");
        fs::write(&path, format!("{HUMAN}\n{}\n", assistant("a1", "u1", PARAGRAPHS))).expect("a written transcript");
        let conversation = thread::build(&path).expect("a built conversation");
        for line in transcript(&conversation, &plain(60)).lines {
            let expected = match line.text().as_str() {
                "" => 0,
                "▎" => 1,
                _ => 2,
            };
            assert_eq!(line.inset, expected, "{:?}", line.text());
        }
    }

    #[test]
    fn a_user_record_that_is_plumbing_is_not_a_turn_though_a_lost_parent_is_still_announced() {
        let tool_result = r#"{"type":"user","uuid":"u2","parentUuid":null,"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:00Z","toolUseResult":{"stdout":"ok"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#;
        let meta = r#"{"type":"user","uuid":"u3","parentUuid":null,"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:00Z","isMeta":true,"message":{"role":"user","content":"caveat"}}"#;
        let peer = r#"{"type":"user","uuid":"u4","parentUuid":null,"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:00Z","origin":{"kind":"peer"},"message":{"role":"user","content":"from elsewhere"}}"#;
        let lines = rendered(&[tool_result, meta, peer]);
        assert!(!lines.iter().any(|line| line.contains(HUMAN_LABEL)), "none of the three is a turn: {lines:?}");
        assert!(!lines.iter().any(|line| line.contains("ran the sweep")), "and none of their content shows: {lines:?}");
        let seams = lines.iter().filter(|line| line.contains("cleared")).count();
        assert_eq!(seams, 2, "three null-parent roots: the first is the session start, the other two are seams: {lines:?}");
    }

    #[test]
    fn a_compact_summary_is_its_own_block_and_not_a_giant_human_turn() {
        let summary = r#"{"type":"user","uuid":"u5","parentUuid":null,"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:00Z","isCompactSummary":true,"message":{"role":"user","content":"this session is being continued from"}}"#;
        let lines = rendered(&[summary]);
        assert!(lines.first().is_some_and(|line| line.contains(SUMMARY_LABEL)), "{lines:?}");
        assert!(!lines.iter().any(|line| line.contains(HUMAN_LABEL)), "never under a you rail: {lines:?}");
        assert!(lines.iter().any(|line| line.contains("this session is being continued")), "{lines:?}");
    }

    #[test]
    fn an_attachment_run_appears_only_once_the_injections_are_revealed() {
        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("session.jsonl");
        let attachment = r#"{"type":"attachment","uuid":"x1","parentUuid":"u1","sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:02Z","attachment":{"type":"date","date":"2026-01-05"}}"#;
        fs::write(&path, [HUMAN, attachment].join("\n") + "\n").expect("a written transcript");
        let conversation = thread::build(&path).expect("a built conversation");

        let hidden = transcript(&conversation, &plain(60));
        assert!(!hidden.lines.iter().any(|line| line.text().contains("injection")), "hidden by default");

        let shown = transcript(&conversation, &revealing(60));
        let lines: Vec<String> = shown.lines.iter().map(RenderedLine::text).collect();
        assert!(lines.iter().any(|line| line.contains("· 1 injection · date")), "{lines:?}");
        assert_eq!(shown.anchors.len(), 1, "the run is reachable with n and expandable with Space");
    }

    #[test]
    fn an_attachment_is_excluded_from_the_transcript() {
        let attachment = r#"{"type":"attachment","uuid":"x1","parentUuid":"u1","sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:02Z","attachment":{"type":"date","date":"2026-01-05"}}"#;
        let lines = rendered(&[HUMAN, attachment]);
        assert_eq!(lines, vec!["▎ you".to_owned(), "▎ read the grid scanner back to me".to_owned()]);
    }

    fn call(id: &str) -> String {
        format!(r#"[{{"type":"tool_use","id":"{id}","name":"Read","input":{{"file_path":"/holodeck/src/engine/grid.rs"}}}}]"#)
    }

    fn gap(line: &str) -> bool {
        line.trim_end() == RAIL_BLANK || line.trim_end().is_empty()
    }

    fn with_expanded(lines: &[&str], expanded: &Expanded) -> Vec<String> {
        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("session.jsonl");
        fs::write(&path, lines.join("\n") + "\n").expect("a written transcript");
        let conversation = thread::build(&path).expect("a built conversation");
        let ctx = Ctx { expanded, ..plain(60) };
        transcript(&conversation, &ctx).lines.iter().map(RenderedLine::text).collect()
    }

    #[test]
    fn a_run_of_tool_calls_costs_no_blank_line_between_them() {
        let lines = rendered(&[
            HUMAN,
            &assistant("a1", "u1", &call("t1")),
            &assistant("a2", "a1", &call("t2")),
            &assistant("a3", "a2", &call("t3")),
        ]);
        assert_eq!(lines.iter().filter(|line| line.contains("Read")).count(), 3, "{lines:?}");
        let head = lines.iter().position(|line| line.contains("Read")).expect("a first call");
        let tail = lines.iter().rposition(|line| line.contains("grid.rs")).expect("a last digest");
        let run = lines.get(head..=tail).expect("the whole run");
        assert!(run.iter().all(|line| !gap(line)), "a gap survived inside the run: {lines:?}");
    }

    #[test]
    fn prose_on_either_side_of_a_call_keeps_its_blank_line() {
        let prose = r#"[{"type":"text","text":"reading the scanner"}]"#;
        let lines = rendered(&[HUMAN, &assistant("a1", "u1", prose), &assistant("a2", "a1", &call("t1"))]);
        let at = lines.iter().position(|line| line.contains("reading the scanner")).expect("the prose");
        assert!(lines.get(at.saturating_add(1)).is_some_and(|line| gap(line)), "{lines:?}");
    }

    #[test]
    fn an_expanded_call_keeps_the_blank_line_that_separates_its_body_from_the_next_call() {
        let expanded: Expanded = std::iter::once(Box::from("t1")).collect();
        let lines = with_expanded(&[HUMAN, &assistant("a1", "u1", &call("t1")), &assistant("a2", "a1", &call("t2"))], &expanded);
        let at = lines.iter().rposition(|line| line.contains("Read")).expect("the second call");
        assert!(lines.get(at.saturating_sub(1)).is_some_and(|line| gap(line)), "{lines:?}");
    }

    fn read(id: &str, path: &str) -> String {
        format!(r#"[{{"type":"tool_use","id":"{id}","name":"Read","input":{{"file_path":"{path}"}}}}]"#)
    }

    fn done(uuid: &str, parent: &str, call: &str) -> String {
        format!(
            r#"{{"type":"user","uuid":"{uuid}","parentUuid":"{parent}","sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:02Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{call}","content":"ok","is_error":false}}]}},"toolUseResult":{{"stdout":"ok","interrupted":false}}}}"#
        )
    }

    #[test]
    fn a_second_call_of_the_same_tool_stacks_under_the_first_name() {
        let lines = rendered(&[
            HUMAN,
            &assistant("a1", "u1", &read("t1", "/holodeck/src/engine/grid.rs")),
            &done("r1", "a1", "t1"),
            &assistant("a2", "r1", &read("t2", "/holodeck/src/engine/coil.rs")),
            &done("r2", "a2", "t2"),
            &assistant("a3", "r2", &read("t3", "/holodeck/src/engine/deflector.rs")),
            &done("r3", "a3", "t3"),
        ]);
        assert_eq!(lines.iter().filter(|line| line.contains("▸ Read")).count(), 1, "one name for the run: {lines:?}");
        assert_eq!(lines.iter().filter(|line| line.contains("└ ")).count(), 3, "every call keeps its digest: {lines:?}");
    }

    #[test]
    fn every_call_in_a_stack_keeps_its_own_anchor() {
        let built = built(&[
            HUMAN,
            &assistant("a1", "u1", &read("t1", "/holodeck/a.rs")),
            &done("r1", "a1", "t1"),
            &assistant("a2", "r1", &read("t2", "/holodeck/b.rs")),
            &done("r2", "a2", "t2"),
        ]);
        let ids: Vec<&str> = built.anchors.iter().map(|anchor| &*anchor.id).collect();
        assert_eq!(ids, ["t1", "t2"], "both calls stay reachable and expandable");
        assert!(built.anchors.windows(2).all(|pair| pair[0].line < pair[1].line), "{:?}", built.anchors);
    }

    #[test]
    fn a_different_tool_starts_its_own_name() {
        let bash = r#"[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"ls"}}]"#;
        let calls =
            [HUMAN, &assistant("a1", "u1", &read("t1", "/holodeck/a.rs")), &done("r1", "a1", "t1"), &assistant("a2", "r1", bash)];
        let lines = rendered(&calls);
        assert!(lines.iter().any(|line| line.contains("▸ Read")), "{lines:?}");
        assert!(lines.iter().any(|line| line.contains("▸ Bash")), "{lines:?}");
    }

    #[test]
    fn a_thinking_row_between_two_calls_breaks_the_stack() {
        let thinking = r#"[{"type":"thinking","thinking":"weighing it"}]"#;
        let lines = rendered(&[
            HUMAN,
            &assistant("a1", "u1", &read("t1", "/holodeck/a.rs")),
            &done("r1", "a1", "t1"),
            &assistant("a2", "r1", thinking),
            &assistant("a3", "a2", &read("t2", "/holodeck/b.rs")),
            &done("r2", "a3", "t2"),
        ]);
        let names = lines.iter().filter(|line| line.contains("▸ Read")).count();
        assert_eq!(names, 2, "a digest orphaned from its name would be unreadable: {lines:?}");
    }

    #[test]
    fn an_expanded_call_does_not_lend_its_name_to_the_call_below_it() {
        let expanded: Expanded = std::iter::once(Box::from("t1")).collect();
        let lines = with_expanded(
            &[
                HUMAN,
                &assistant("a1", "u1", &read("t1", "/holodeck/a.rs")),
                &done("r1", "a1", "t1"),
                &assistant("a2", "r1", &read("t2", "/holodeck/b.rs")),
                &done("r2", "a2", "t2"),
            ],
            &expanded,
        );
        assert_eq!(lines.iter().filter(|line| line.contains("Read")).count(), 2, "{lines:?}");
    }

    #[test]
    fn an_injection_run_is_not_welded_to_the_call_below_it() {
        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("session.jsonl");
        let attachment = r#"{"type":"attachment","uuid":"x1","parentUuid":"a1","sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:02Z","attachment":{"type":"date","date":"2026-01-05"}}"#;
        let records = [HUMAN, &assistant("a1", "u1", &call("t1")), attachment, &assistant("a2", "x1", &call("t2"))];
        fs::write(&path, records.join("\n") + "\n").expect("a written transcript");
        let conversation = thread::build(&path).expect("a built conversation");
        let lines: Vec<String> = transcript(&conversation, &revealing(60)).lines.iter().map(RenderedLine::text).collect();

        let at = lines.iter().position(|line| line.contains("injection")).expect("the run");
        assert!(lines.get(at.saturating_add(1)).is_some_and(|line| gap(line)), "{lines:?}");
    }

    #[test]
    fn a_divider_between_two_calls_keeps_its_air() {
        let cleared = format!(
            r#"{{"type":"assistant","uuid":"a2","parentUuid":null,"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:02Z","message":{{"id":"msg_a2","model":"opus-5","role":"assistant","content":{}}}}}"#,
            call("t2")
        );
        let lines = rendered(&[HUMAN, &assistant("a1", "u1", &call("t1")), &cleared]);
        let at = lines.iter().position(|line| line.contains("──")).expect("a divider");
        assert!(lines.get(at.saturating_sub(1)).is_some_and(|line| gap(line)), "{lines:?}");
    }

    #[test]
    fn a_thinking_block_collapses_to_one_dim_line_and_never_shows_its_signature() {
        let block = r#"[{"type":"thinking","thinking":"one\ntwo\nthree","signature":"EqoBCkgIBRABGAI..."}]"#;
        let lines = rendered(&[HUMAN, &assistant("a1", "u1", block)]);
        assert!(lines.contains(&"▎ thinking · 3 lines".to_owned()), "{lines:?}");
        assert!(!lines.iter().any(|line| line.contains("EqoBCkgI")), "the signature leaked: {lines:?}");
    }

    #[test]
    fn every_rendered_node_gets_a_span_naming_the_line_it_starts_on() {
        let built = built(&[HUMAN, &assistant("a1", "u1", PARAGRAPHS)]);
        assert_eq!(built.spans.len(), 2, "one span per node that rendered");
        let lines: Vec<usize> = built.spans.iter().map(|span| span.line).collect();
        assert_eq!(lines, [0, 3], "the human turn at 0, the reply where its group starts");
        assert!(built.spans.windows(2).all(|pair| pair[0].line < pair[1].line), "spans are ascending");
    }

    #[test]
    fn a_line_resolves_to_the_node_it_came_from_and_back_again() {
        let built = built(&[HUMAN, &assistant("a1", "u1", PARAGRAPHS)]);
        let Some(first) = built.spans.first().copied() else { panic!("a span") };
        let Some(second) = built.spans.get(1).copied() else { panic!("two spans") };

        let position = built.position(1).expect("a position inside the human turn");
        assert_eq!(position.node, first.node);
        assert_eq!(position.offset, 1);
        assert_eq!(built.line_of(position), Some(1), "and it maps back to the same line");

        let position = built.position(second.line).expect("a position at the reply");
        assert_eq!(position.node, second.node);
        assert_eq!(position.offset, 0);
    }

    #[test]
    fn every_header_block_records_the_line_it_starts_on() {
        let built = built(&[HUMAN, &assistant("a1", "u1", PARAGRAPHS)]);
        assert_eq!(built.turns, vec![0, 3], "the you header and the claude header");
    }

    #[test]
    fn a_run_of_replies_under_one_header_is_one_stop_and_not_three() {
        let built = built(&[
            HUMAN,
            &assistant("a1", "u1", r#"[{"type":"text","text":"first"}]"#),
            &assistant("a2", "a1", r#"[{"type":"text","text":"second"}]"#),
            &assistant("a3", "a2", r#"[{"type":"text","text":"third"}]"#),
        ]);
        assert_eq!(built.spans.len(), 4, "four nodes rendered");
        assert_eq!(built.turns.len(), 2, "but only two headers to stop at: {:?}", built.turns);
    }

    #[test]
    fn a_change_of_model_is_a_second_stop_because_it_is_a_second_header() {
        let second = r#"{"type":"assistant","uuid":"a2","parentUuid":"a1","sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:02Z","message":{"id":"msg_a2","model":"haiku-4-5","role":"assistant","content":[{"type":"text","text":"second"}]}}"#;
        let built = built(&[HUMAN, &assistant("a1", "u1", r#"[{"type":"text","text":"first"}]"#), second]);
        assert_eq!(built.turns.len(), 3, "{:?}", built.turns);
    }

    #[test]
    fn the_turn_lines_ascend_and_each_one_opens_a_block() {
        let built = built(&[HUMAN, &assistant("a1", "u1", PARAGRAPHS)]);
        assert!(built.turns.windows(2).all(|pair| pair[0] < pair[1]), "{:?}", built.turns);
        for line in &built.turns {
            assert!(built.lines.get(*line).is_some_and(|line| !line.is_blank()), "turn at {line} is a blank line");
        }
    }

    #[test]
    fn a_step_forward_and_back_lands_on_the_next_and_previous_header() {
        let built = built(&[HUMAN, &assistant("a1", "u1", PARAGRAPHS)]);
        assert_eq!(built.turn_after(0), Some(3));
        assert_eq!(built.turn_after(1), Some(3), "from anywhere inside the first block");
        assert_eq!(built.turn_before(3), Some(0));
        assert_eq!(built.turn_before(4), Some(3), "from inside a block, back to its own header");
    }

    #[test]
    fn a_step_past_either_end_stops_rather_than_wrapping() {
        let built = built(&[HUMAN, &assistant("a1", "u1", PARAGRAPHS)]);
        assert_eq!(built.turn_before(0), None, "nothing above the first header");
        assert_eq!(built.turn_after(99), None, "and nothing below the last");
    }

    #[test]
    fn an_empty_transcript_has_nowhere_to_step() {
        let built = Transcript::default();
        assert_eq!(built.turn_after(0), None);
        assert_eq!(built.turn_before(0), None);
    }

    #[test]
    fn a_line_before_the_first_span_belongs_to_no_node() {
        let built = Transcript::default();
        assert_eq!(built.position(0), None, "an empty transcript anchors nothing");
    }

    #[test]
    fn an_offset_past_the_end_of_its_node_is_clamped_rather_than_leaking_into_the_next() {
        let built = built(&[HUMAN, &assistant("a1", "u1", PARAGRAPHS)]);
        let Some(first) = built.spans.first().copied() else { panic!("a span") };
        let Some(second) = built.spans.get(1).copied() else { panic!("two spans") };
        let line = built.line_of(Position { node: first.node, offset: 99 }).expect("a clamped line");
        assert!(line < second.line, "offset 99 in a two-line turn must not land in the reply");
    }

    #[test]
    fn a_span_survives_a_rewrap_because_it_is_keyed_on_the_node_not_the_line() {
        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("session.jsonl");
        let lines = [HUMAN.to_owned(), assistant("a1", "u1", PARAGRAPHS)];
        fs::write(&path, lines.join("\n") + "\n").expect("a written transcript");
        let conversation = thread::build(&path).expect("a built conversation");

        let wide = transcript(&conversation, &plain(60));
        let narrow = transcript(&conversation, &plain(24));
        assert_ne!(wide.lines.len(), narrow.lines.len(), "the two widths must actually differ");

        let nodes = |built: &Transcript| built.spans.iter().map(|span| span.node).collect::<Vec<_>>();
        assert_eq!(nodes(&wide), nodes(&narrow), "the same nodes, at different lines");
        assert_ne!(
            wide.spans.iter().map(|span| span.line).collect::<Vec<_>>(),
            narrow.spans.iter().map(|span| span.line).collect::<Vec<_>>(),
            "and the lines genuinely moved"
        );
    }

    #[test]
    fn a_tool_call_is_its_name_on_one_row_and_its_digest_dimmed_on_the_next() {
        let block = r#"[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la"}}]"#;
        let result = r#"{"type":"user","uuid":"u2","parentUuid":"a1","sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:03Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"total 0","is_error":false}]},"toolUseResult":{"stdout":"total 0\n","interrupted":false}}"#;
        let lines = rendered(&[HUMAN, &assistant("a1", "u1", block), result]);
        let at = lines.iter().position(|line| line.contains("▸ Bash")).expect("a tool call line");
        let name = lines.get(at).expect("a name row");
        assert_eq!(name, "▎ ▸ Bash", "a call that worked carries no word on the right edge");
        assert_eq!(lines.get(at.saturating_add(1)).map(String::as_str), Some("▎   └ ls -la"));
    }

    #[test]
    fn a_call_whose_result_never_arrived_reads_as_pending_rather_than_as_failed() {
        let block = r#"[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo build"}}]"#;
        let lines = rendered(&[HUMAN, &assistant("a1", "u1", block)]);
        let call = lines.iter().find(|line| line.contains("▸ Bash")).expect("a tool call line");
        assert!(call.ends_with("pending"), "{call:?}");
    }

    #[test]
    fn the_result_record_itself_is_still_not_a_turn_so_a_run_is_not_broken_by_one() {
        let first = r#"[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]"#;
        let result = r#"{"type":"user","uuid":"u2","parentUuid":"a1","sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:03Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok","is_error":false}]},"toolUseResult":{"stdout":"ok\n"}}"#;
        let second = r#"[{"type":"text","text":"Done."}]"#;
        let lines = rendered(&[HUMAN, &assistant("a1", "u1", first), result, &assistant("a2", "u2", second)]);
        assert_eq!(lines.iter().filter(|line| line.contains("claude")).count(), 1, "one header for the run: {lines:?}");
        assert!(!lines.iter().any(|line| line.contains("total 0")), "the result node is plumbing, not a turn");
    }

    #[test]
    fn the_error_style_names_a_foreground_and_never_a_background() {
        let style = Theme::default().style(Element::ToolError);
        assert!(style.fg.is_some(), "an error has to be findable");
        assert!(style.bg.is_none(), "an error colour may never sit on the background");
    }

    #[test]
    fn a_tool_call_summary_never_breaks_across_lines() {
        let block = r#"[{"type":"tool_use","id":"t1","name":"Write","input":{"content":"first\nsecond\nthird and a great deal more prose besides"}}]"#;
        let lines = rendered(&[HUMAN, &assistant("a1", "u1", block)]);
        let calls = lines.iter().filter(|line| line.contains("▸ ")).count();
        assert_eq!(calls, 1);
        assert!(lines.iter().all(|line| line.chars().count() <= 60), "{lines:?}");
        assert!(
            lines.iter().any(|line| line.contains("first second third")),
            "the newlines flattened into the one line: {lines:?}"
        );
    }

    #[test]
    fn an_image_names_its_type_and_decoded_size_without_decoding_anything() {
        let block = r#"[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"","redactedBytes":1960000}}]"#;
        let lines = rendered(&[HUMAN, &assistant("a1", "u1", block)]);
        assert!(lines.contains(&"▎ [image · png · 1.4 MB]".to_owned()), "{lines:?}");
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
        assert_eq!(lines, vec!["▎ you".to_owned(), "▎ look at this".to_owned(), "▎ [image · png · 4.3 KB]".to_owned()]);
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
        for line in transcript(&conversation, &plain(24)).lines {
            assert!(line.width() <= 24, "{:?}", line.text());
        }
    }
}
