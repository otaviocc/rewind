//! A conversation, or one node of it, as plain Markdown. Pure: no I/O, no terminal.
//!
//! Shares `render::Ctx` with the painter, so an export honours the same expansion state and
//! injection toggle the screen is currently showing.

use std::fmt::Write as _;

use crate::domain::block::{Block, Content};
use crate::domain::record::CompactMetadata;
use crate::domain::thread::{AssistantTurn, Conversation, Node, NodeId, NodeKind};
use crate::render::{Ctx, Overflow, injection};

const HUMAN_HEADING: &str = "## you";
const PROMPT_HEADING: &str = "## prompt";
const ASSISTANT_HEADING: &str = "## claude";
const SUMMARY_HEADING: &str = "## summary";
const INDENT: &str = "> ";

pub fn message(conversation: &Conversation, id: NodeId, ctx: &Ctx<'_>) -> Option<String> {
    let node = conversation.node(id)?;
    let sidechain = ctx.root.is_some() || conversation.is_sidechain();
    let mut out = String::new();
    push_node(conversation, ctx, node, sidechain, &mut out);
    let text = out.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

pub fn session(conversation: &Conversation, ctx: &Ctx<'_>) -> String {
    let sidechain = ctx.root.is_some() || conversation.is_sidechain();
    let walk =
        ctx.root.map_or_else(|| conversation.thread_with(ctx.branches), |root| conversation.path_from_with(root, ctx.branches));
    let mut out = String::new();
    for id in walk {
        let Some(node) = conversation.node(id) else { continue };
        push_node(conversation, ctx, node, sidechain, &mut out);
    }
    out.trim().to_owned()
}

fn push_node(conversation: &Conversation, ctx: &Ctx<'_>, node: &Node, sidechain: bool, out: &mut String) {
    match &node.kind {
        NodeKind::User(record) => {
            if let Some(command) = record.command() {
                match command {
                    crate::domain::command::Command::Invocation { name, args } => {
                        let _ = writeln!(out, "\n> {name}{}\n", args.map(|args| format!(" {args}")).unwrap_or_default());
                    }
                    crate::domain::command::Command::Output(text) => {
                        let _ = writeln!(out, "```\n{text}\n```\n");
                    }
                }
            } else if record.is_compact_summary {
                let _ = writeln!(out, "\n{SUMMARY_HEADING}\n");
                push_content(&record.message.content, out);
            } else if record.is_human_turn() {
                let heading = if sidechain { PROMPT_HEADING } else { HUMAN_HEADING };
                let _ = writeln!(out, "\n{heading}\n");
                push_content(&record.message.content, out);
            }
        }
        NodeKind::Assistant(turn) => push_assistant(conversation, ctx, turn, out),
        NodeKind::System(record) => {
            if let Some(metadata) = &record.compact_metadata {
                push_compaction(metadata, out);
            }
        }
        NodeKind::Attachment(_) => {
            if ctx.injections {
                let kinds = injection::kinds(conversation, std::slice::from_ref(&node.id));
                let kind = kinds.first().map_or("injection", String::as_str);
                let _ = writeln!(out, "\n_injection: {kind}_\n");
            }
        }
    }
}

fn push_content(content: &Content, out: &mut String) {
    match content {
        Content::Text(text) => {
            out.push_str(text.trim());
            out.push('\n');
        }
        Content::Blocks(blocks) => push_blocks(blocks, out),
    }
}

fn push_assistant(conversation: &Conversation, ctx: &Ctx<'_>, turn: &AssistantTurn, out: &mut String) {
    let heading =
        turn.model.as_deref().map_or_else(|| ASSISTANT_HEADING.to_owned(), |model| format!("{ASSISTANT_HEADING} · {model}"));
    let _ = writeln!(out, "\n{heading}\n");
    for block in &turn.content {
        match block {
            Block::ToolUse { id, name, input } => push_tool_use(conversation, ctx, id, name, input, out),
            other => push_block(other, out),
        }
    }
}

fn push_blocks(blocks: &[Block], out: &mut String) {
    for block in blocks {
        push_block(block, out);
    }
}

fn push_block(block: &Block, out: &mut String) {
    match block {
        Block::Text { text } => {
            out.push_str(text.trim());
            out.push('\n');
        }
        Block::Thinking { thinking } => {
            for line in thinking.trim().lines() {
                let _ = writeln!(out, "{INDENT}{line}");
            }
            out.push('\n');
        }
        Block::Image { source } => {
            let _ = writeln!(out, "_image{}_", source.media_type.as_deref().map(|kind| format!(" · {kind}")).unwrap_or_default());
        }
        Block::ToolUse { .. } | Block::ToolResult { .. } | Block::Other { .. } => {}
    }
}

fn push_tool_use(conversation: &Conversation, ctx: &Ctx<'_>, id: &str, name: &str, input: &serde_json::Value, out: &mut String) {
    let pretty = serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
    let _ = writeln!(out, "```\n{name} {pretty}\n```");
    if !ctx.is_expanded(id) {
        out.push('\n');
        return;
    }
    let Some(result) = conversation.result_of(id) else {
        out.push('\n');
        return;
    };
    let outcome = crate::domain::tool::Outcome::of(result, id);
    let body = match ctx.outputs.get(id) {
        Some(Overflow::Lines(lines)) => lines.join("\n"),
        _ => outcome.as_ref().map(crate::domain::tool::Outcome::body).unwrap_or_default().into_owned(),
    };
    if !body.trim().is_empty() {
        let _ = writeln!(out, "\n```\n{}\n```", body.trim());
    }
    out.push('\n');
}

fn push_compaction(metadata: &CompactMetadata, out: &mut String) {
    let mut line = "compacted".to_owned();
    if let (Some(pre), Some(post)) = (metadata.pre_tokens, metadata.post_tokens) {
        let _ = write!(line, " · {pre} → {post} tokens");
    }
    let _ = writeln!(out, "\n---\n_{line}_\n---\n");
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::domain::thread;
    use crate::render::{Branches, Expanded, Outputs};
    use crate::theme::Theme;

    fn built(lines: &[String]) -> (TempDir, Conversation) {
        let dir = TempDir::new().expect("a temp dir");
        let path = dir.path().join("session.jsonl");
        fs::write(&path, lines.join("\n") + "\n").expect("a written transcript");
        let conversation = thread::build(&path).expect("a built conversation");
        (dir, conversation)
    }

    fn ctx<'a>(
        expanded: &'a Expanded,
        outputs: &'a Outputs,
        agents: &'a crate::domain::subagent::Agents,
        branches: &'a Branches,
        theme: &'a Theme,
        injections: bool,
    ) -> Ctx<'a> {
        Ctx { width: 80, theme, expanded, outputs, agents, root: None, branches, injections }
    }

    fn simple_session() -> Vec<String> {
        vec![
            r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"hello there"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#.to_owned(),
            r#"{"parentUuid":"u1","isSidechain":false,"message":{"model":"m","id":"msg1","role":"assistant","content":[{"type":"text","text":"hi back"}]},"type":"assistant","uuid":"a1","timestamp":"2026-01-01T00:01:00Z","sessionId":"s1"}"#.to_owned(),
        ]
    }

    fn tool_call_session() -> Vec<String> {
        vec![
            r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"run it"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#.to_owned(),
            r#"{"parentUuid":"u1","isSidechain":false,"message":{"model":"m","id":"msg1","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"echo hi"}}]},"type":"assistant","uuid":"a1","timestamp":"2026-01-01T00:01:00Z","sessionId":"s1"}"#.to_owned(),
            r#"{"parentUuid":"a1","isSidechain":false,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"hi\n"}]},"type":"user","toolUseResult":{},"uuid":"u2","timestamp":"2026-01-01T00:02:00Z","sessionId":"s1"}"#.to_owned(),
        ]
    }

    #[test]
    fn a_human_message_exports_with_its_heading() {
        let (_dir, conversation) = built(&simple_session());
        let expanded = Expanded::default();
        let outputs = Outputs::default();
        let agents = crate::domain::subagent::Agents::default();
        let branches = Branches::default();
        let theme = Theme::default();
        let id = conversation.id_of("u1").expect("the human turn");
        let text =
            message(&conversation, id, &ctx(&expanded, &outputs, &agents, &branches, &theme, false)).expect("exported text");
        assert!(text.starts_with(HUMAN_HEADING), "{text}");
        assert!(text.contains("hello there"));
    }

    #[test]
    fn an_unexpanded_tool_call_exports_without_its_result() {
        let (_dir, conversation) = built(&tool_call_session());
        let expanded = Expanded::default();
        let outputs = Outputs::default();
        let agents = crate::domain::subagent::Agents::default();
        let branches = Branches::default();
        let theme = Theme::default();
        let text = session(&conversation, &ctx(&expanded, &outputs, &agents, &branches, &theme, false));
        assert!(text.contains("Bash"));
        assert!(!text.contains("echo hi\n```\n\n```\nhi"), "the result must not appear unexpanded");
        assert!(!text.contains("hi\n```"), "{text}");
    }

    #[test]
    fn an_expanded_tool_call_exports_with_its_result() {
        let (_dir, conversation) = built(&tool_call_session());
        let mut expanded = Expanded::default();
        expanded.insert(Box::from("t1"));
        let outputs = Outputs::default();
        let agents = crate::domain::subagent::Agents::default();
        let branches = Branches::default();
        let theme = Theme::default();
        let text = session(&conversation, &ctx(&expanded, &outputs, &agents, &branches, &theme, false));
        assert!(text.contains("hi"), "{text}");
    }

    #[test]
    fn an_attachment_exports_only_when_injections_are_on() {
        let lines = vec![
            r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"go"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#.to_owned(),
            r#"{"parentUuid":"u1","isSidechain":false,"type":"attachment","attachment":{"type":"date"},"uuid":"x1","timestamp":"2026-01-01T00:00:01Z","sessionId":"s1"}"#.to_owned(),
        ];
        let (_dir, conversation) = built(&lines);
        let expanded = Expanded::default();
        let outputs = Outputs::default();
        let agents = crate::domain::subagent::Agents::default();
        let branches = Branches::default();
        let theme = Theme::default();
        let off = session(&conversation, &ctx(&expanded, &outputs, &agents, &branches, &theme, false));
        assert!(!off.contains("injection"), "{off}");
        let on = session(&conversation, &ctx(&expanded, &outputs, &agents, &branches, &theme, true));
        assert!(on.contains("date"), "{on}");
    }

    #[test]
    fn a_session_export_carries_every_turn_in_order() {
        let (_dir, conversation) = built(&simple_session());
        let expanded = Expanded::default();
        let outputs = Outputs::default();
        let agents = crate::domain::subagent::Agents::default();
        let branches = Branches::default();
        let theme = Theme::default();
        let text = session(&conversation, &ctx(&expanded, &outputs, &agents, &branches, &theme, false));
        let you_at = text.find(HUMAN_HEADING).expect("the human heading");
        let claude_at = text.find(ASSISTANT_HEADING).expect("the assistant heading");
        assert!(you_at < claude_at, "{text}");
    }
}
