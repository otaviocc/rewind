//! What a tool call asked for and what came back: the label, a digest of the input, an outcome,
//! and the diff a file edit carries with it.

use std::borrow::Cow;

use serde::Deserialize;
use serde_json::Value;

use crate::domain::block::{Block, Content, ToolResultContent};
use crate::domain::thread::{Node, NodeKind};

const MCP_PREFIX: &str = "mcp__";
const MCP_SEPARATOR: &str = "__";
const ERROR_PREFIX: &str = "Error: ";
const ERROR_OPEN: &str = "<tool_use_error>";
const INTERRUPTED: &str = "[Request interrupted by user";
const DENIALS: [&str; 3] =
    ["The user doesn't want to proceed with this tool use", "User rejected tool use", "Permission for this action was denied"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label<'a> {
    Plain(&'a str),
    Mcp { server: &'a str, tool: &'a str },
}

pub fn label(name: &str) -> Label<'_> {
    let Some(rest) = name.strip_prefix(MCP_PREFIX) else { return Label::Plain(name) };
    match rest.split_once(MCP_SEPARATOR) {
        Some((server, tool)) if !server.is_empty() && !tool.is_empty() => Label::Mcp { server, tool },
        _ => Label::Plain(name),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Failed,
    Denied,
    Interrupted,
    Pending,
}

impl Status {
    pub const fn is_error(self) -> bool {
        matches!(self, Self::Failed | Self::Denied | Self::Interrupted)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Outcome<'a> {
    pub content: Option<&'a ToolResultContent>,
    pub is_error: Option<bool>,
    pub detail: Option<&'a Value>,
}

impl<'a> Outcome<'a> {
    pub fn of(node: &'a Node, tool_use_id: &str) -> Option<Self> {
        let NodeKind::User(record) = &node.kind else { return None };
        let Content::Blocks(blocks) = &record.message.content else { return None };
        blocks.iter().find_map(|block| match block {
            Block::ToolResult { tool_use_id: Some(id), content, is_error } if id == tool_use_id => {
                Some(Self { content: content.as_ref(), is_error: *is_error, detail: record.tool_use_result.as_ref() })
            }
            _ => None,
        })
    }

    pub fn body(&self) -> Cow<'a, str> {
        match self.content {
            None => Cow::Borrowed(""),
            Some(ToolResultContent::Text(text)) => Cow::Borrowed(text),
            Some(ToolResultContent::Blocks(blocks)) => match blocks.as_slice() {
                [] => Cow::Borrowed(""),
                [only] => Cow::Borrowed(&only.text),
                many => Cow::Owned(many.iter().map(|block| block.text.as_str()).collect::<Vec<_>>().join("\n")),
            },
        }
    }

    pub fn status(&self) -> Status {
        let body = self.body();
        let marker = marker(&body);
        if marker.starts_with(INTERRUPTED) || self.detail.and_then(|detail| detail.get("interrupted")) == Some(&Value::Bool(true))
        {
            return Status::Interrupted;
        }
        if DENIALS.iter().any(|denial| marker.starts_with(denial)) {
            return Status::Denied;
        }
        if self.is_error == Some(true) || self.detail.and_then(Value::as_str).is_some_and(|text| text.starts_with(ERROR_PREFIX)) {
            return Status::Failed;
        }
        Status::Ok
    }
}

pub fn status(outcome: Option<&Outcome<'_>>) -> Status {
    outcome.map_or(Status::Pending, Outcome::status)
}

fn marker(body: &str) -> &str {
    let body = body.trim_start();
    let body = body.strip_prefix(ERROR_PREFIX).unwrap_or(body);
    body.strip_prefix(ERROR_OPEN).unwrap_or(body)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Digest<'a> {
    pub primary: Option<Cow<'a, str>>,
    pub secondary: Option<Cow<'a, str>>,
}

pub fn digest<'a>(name: &str, input: &'a Value, detail: Option<&'a Value>) -> Digest<'a> {
    if name == "TodoWrite" {
        return todos(input);
    }
    let primary = keyed(input, primary_key(name)).map(Cow::Borrowed).or_else(|| generic(input));
    let secondary = computed(name, detail)
        .or_else(|| keyed(input, secondary_key(name)).map(Cow::Borrowed))
        .filter(|secondary| Some(secondary.as_ref()) != primary.as_deref());
    Digest { primary, secondary }
}

fn primary_key(name: &str) -> &'static str {
    match name {
        "Bash" | "Monitor" => "command",
        "Read" | "Edit" | "Write" | "NotebookEdit" => "file_path",
        "Agent" | "Task" => "subagent_type",
        "Skill" => "skill",
        "WebFetch" => "url",
        "WebSearch" | "ToolSearch" => "query",
        "Grep" | "Glob" => "pattern",
        "SendMessage" => "to",
        "TaskOutput" | "TaskStop" => "task_id",
        _ => "",
    }
}

fn secondary_key(name: &str) -> &'static str {
    match name {
        "Agent" | "Task" => "description",
        "Skill" => "args",
        "Grep" | "Glob" => "path",
        _ => "",
    }
}

fn keyed<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    if key.is_empty() {
        return None;
    }
    let text = input.get(key)?.as_str()?;
    (!text.is_empty()).then_some(text)
}

fn generic(input: &Value) -> Option<Cow<'_, str>> {
    let fields = input.as_object()?;
    if let Some(text) = fields.values().find_map(|value| value.as_str()).filter(|text| !text.is_empty()) {
        return Some(Cow::Borrowed(text));
    }
    let rendered = serde_json::to_string(input).ok()?;
    (rendered != "{}").then_some(Cow::Owned(rendered))
}

fn computed<'a>(name: &str, detail: Option<&'a Value>) -> Option<Cow<'a, str>> {
    let detail = detail?;
    match name {
        "Read" => {
            let lines = detail.get("file")?.get("numLines")?.as_u64()?;
            Some(Cow::Owned(format!("{lines} {}", plural(lines, "line", "lines"))))
        }
        "Edit" | "Write" => {
            let hunks = u64::try_from(patch(detail)?.len()).ok()?;
            Some(Cow::Owned(format!("{hunks} {}", plural(hunks, "hunk", "hunks"))))
        }
        _ => None,
    }
}

fn todos(input: &Value) -> Digest<'static> {
    let Some(todos) = input.get("todos").and_then(Value::as_array) else { return Digest::default() };
    let total = todos.len();
    let done = todos.iter().filter(|todo| todo.get("status").and_then(Value::as_str) == Some("completed")).count();
    Digest { primary: Some(Cow::Owned(format!("{done} of {total} done"))), secondary: None }
}

const fn plural(count: u64, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    #[serde(default)]
    pub old_start: u64,
    #[serde(default)]
    pub old_lines: u64,
    #[serde(default)]
    pub new_start: u64,
    #[serde(default)]
    pub new_lines: u64,
    #[serde(default)]
    pub lines: Vec<String>,
}

pub fn patch(detail: &Value) -> Option<Vec<Hunk>> {
    let hunks = Vec::<Hunk>::deserialize(detail.get("structuredPatch")?).ok()?;
    (!hunks.is_empty()).then_some(hunks)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overflow<'a> {
    pub name: &'a str,
    pub bytes: u64,
}

pub fn overflow(detail: &Value) -> Option<Overflow<'_>> {
    let recorded = detail.get("persistedOutputPath")?.as_str()?;
    let name = recorded.rsplit(['/', '\\']).next().filter(|name| !name.is_empty())?;
    let bytes = detail.get("persistedOutputSize").and_then(Value::as_u64).unwrap_or(0);
    Some(Overflow { name, bytes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(body: &str) -> ToolResultContent {
        ToolResultContent::Text(body.to_owned())
    }

    fn outcome<'a>(content: &'a ToolResultContent, is_error: Option<bool>, detail: Option<&'a Value>) -> Outcome<'a> {
        Outcome { content: Some(content), is_error, detail }
    }

    #[test]
    fn an_mcp_name_splits_into_a_server_and_a_tool() {
        assert_eq!(label("mcp__jeffries__beam_status"), Label::Mcp { server: "jeffries", tool: "beam_status" });
        assert_eq!(
            label("mcp__claude_ai_Atlassian_Rovo__getJiraIssue"),
            Label::Mcp { server: "claude_ai_Atlassian_Rovo", tool: "getJiraIssue" },
            "the server may itself contain single underscores"
        );
    }

    #[test]
    fn a_name_that_only_looks_like_an_mcp_name_stays_plain() {
        assert_eq!(label("Bash"), Label::Plain("Bash"));
        assert_eq!(label("mcp__lonely"), Label::Plain("mcp__lonely"), "no tool half");
        assert_eq!(label("mcp____tool"), Label::Plain("mcp____tool"), "no server half");
    }

    #[test]
    fn a_call_with_no_result_at_all_is_pending_not_failed() {
        assert_eq!(status(None), Status::Pending);
        assert!(!Status::Pending.is_error(), "a session that ended mid-call did not fail");
    }

    #[test]
    fn the_three_outcomes_wearing_is_error_are_told_apart_by_their_body() {
        let denial = text("The user doesn't want to proceed with this tool use. The tool use was rejected");
        let interrupt = text("[Request interrupted by user for tool use]");
        let failure = text("<tool_use_error>File has been modified since read");
        assert_eq!(outcome(&denial, Some(true), None).status(), Status::Denied);
        assert_eq!(outcome(&interrupt, Some(true), None).status(), Status::Interrupted);
        assert_eq!(outcome(&failure, Some(true), None).status(), Status::Failed);
    }

    #[test]
    fn a_denial_is_recognised_through_the_error_prefix_a_bare_string_result_adds() {
        let denial = text("Error: Permission for this action was denied by the Claude Code auto mode classifier");
        assert_eq!(outcome(&denial, None, None).status(), Status::Denied);
    }

    #[test]
    fn a_bare_string_result_beginning_with_error_fails_even_with_no_is_error_flag() {
        let body = text("no matches");
        let detail = json!("Error: Exit code 1\nno matches");
        assert_eq!(outcome(&body, None, Some(&detail)).status(), Status::Failed);
    }

    #[test]
    fn the_interrupted_flag_on_the_result_is_believed_without_a_body() {
        let body = text("");
        let detail = json!({"stdout": "", "interrupted": true});
        assert_eq!(outcome(&body, None, Some(&detail)).status(), Status::Interrupted);
    }

    #[test]
    fn a_plain_result_is_ok() {
        let body = text("     212 src/engine/grid.rs");
        let detail = json!({"stdout": "     212 src/engine/grid.rs\n", "interrupted": false});
        assert_eq!(outcome(&body, Some(false), Some(&detail)).status(), Status::Ok);
    }

    #[test]
    fn a_result_body_split_across_text_blocks_is_joined_without_allocating_for_one() {
        let one = ToolResultContent::Blocks(vec![crate::domain::block::TextBlock { text: "only".to_owned() }]);
        assert!(matches!(outcome(&one, None, None).body(), Cow::Borrowed("only")));
        let two = ToolResultContent::Blocks(vec![
            crate::domain::block::TextBlock { text: "first".to_owned() },
            crate::domain::block::TextBlock { text: "second".to_owned() },
        ]);
        assert_eq!(outcome(&two, None, None).body(), "first\nsecond");
    }

    #[test]
    fn each_tool_is_digested_by_the_field_that_says_what_it_did() {
        let bash = json!({"command": "wc -l src/engine/grid.rs", "description": "Count the scanner lines"});
        assert_eq!(digest("Bash", &bash, None).primary.as_deref(), Some("wc -l src/engine/grid.rs"));

        let agent = json!({"description": "Trace the grid scanner", "prompt": "…", "subagent_type": "Explore"});
        let digested = digest("Agent", &agent, None);
        assert_eq!(digested.primary.as_deref(), Some("Explore"));
        assert_eq!(digested.secondary.as_deref(), Some("Trace the grid scanner"));

        let fetch = json!({"url": "https://holodeck.invalid/specs", "prompt": "…"});
        assert_eq!(digest("WebFetch", &fetch, None).primary.as_deref(), Some("https://holodeck.invalid/specs"));

        let grep = json!({"pattern": "row_cells", "path": "src/engine"});
        let digested = digest("Grep", &grep, None);
        assert_eq!(digested.primary.as_deref(), Some("row_cells"));
        assert_eq!(digested.secondary.as_deref(), Some("src/engine"));
    }

    #[test]
    fn the_legacy_tool_names_digest_exactly_as_their_successors_do() {
        let task = json!({"description": "Audit the old shapes", "prompt": "…", "subagent_type": "Explore"});
        assert_eq!(digest("Task", &task, None).secondary.as_deref(), Some("Audit the old shapes"));
        let glob = json!({"pattern": "**/*.jsonl"});
        assert_eq!(digest("Glob", &glob, None).primary.as_deref(), Some("**/*.jsonl"));
    }

    #[test]
    fn a_read_counts_its_lines_and_an_edit_counts_its_hunks_from_the_result() {
        let input = json!({"file_path": "/holodeck/src/engine/grid.rs"});
        let read = json!({"type": "text", "file": {"numLines": 212, "totalLines": 212}});
        assert_eq!(digest("Read", &input, Some(&read)).secondary.as_deref(), Some("212 lines"));

        let one = json!({"structuredPatch": [{"oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1, "lines": [" a"]}]});
        assert_eq!(digest("Edit", &input, Some(&one)).secondary.as_deref(), Some("1 hunk"));
    }

    #[test]
    fn an_unknown_tool_name_digests_to_the_first_string_field_rather_than_to_nothing() {
        let input = json!({"quantity": 1, "pattern": "tea, earl grey, hot"});
        let digested = digest("Replicator", &input, None);
        assert_eq!(
            digested.primary.as_deref(),
            Some("tea, earl grey, hot"),
            "keys are visited in order and quantity is not a string"
        );
        assert_eq!(digested.secondary, None);
    }

    #[test]
    fn a_secondary_that_only_repeats_the_primary_is_dropped_rather_than_said_twice() {
        let task = json!({"description": "Audit the old shapes", "prompt": "…"});
        let digested = digest("Task", &task, None);
        assert_eq!(digested.primary.as_deref(), Some("Audit the old shapes"), "no subagent_type, so the generic field wins");
        assert_eq!(digested.secondary, None, "and it is not then repeated as the secondary");
    }

    #[test]
    fn an_input_with_no_string_field_at_all_falls_back_to_its_json_rather_than_to_nothing() {
        let input = json!({"deck": 9, "verbose": true});
        assert_eq!(digest("mcp__jeffries__beam_status", &input, None).primary.as_deref(), Some(r#"{"deck":9,"verbose":true}"#));
    }

    #[test]
    fn an_empty_input_digests_to_nothing_at_all() {
        assert_eq!(digest("ListAgents", &json!({}), None), Digest::default());
        assert_eq!(digest("ListAgents", &Value::Null, None), Digest::default());
    }

    #[test]
    fn an_elided_input_falls_through_the_tools_own_key_to_the_generic_one() {
        let input = json!({"__unparsedToolInput": "{\"file_path\": \"/holodeck/src/engine/co"});
        let digested = digest("Read", &input, None);
        assert_eq!(digested.primary.as_deref(), Some("{\"file_path\": \"/holodeck/src/engine/co"));
    }

    #[test]
    fn a_todo_write_counts_what_is_done_because_its_input_has_no_string_worth_showing() {
        let input = json!({"todos": [
            {"content": "audit the shapes", "status": "completed", "activeForm": "Auditing"},
            {"content": "render the diffs", "status": "pending", "activeForm": "Rendering"},
        ]});
        assert_eq!(digest("TodoWrite", &input, None).primary.as_deref(), Some("1 of 2 done"));
    }

    #[test]
    fn a_structured_patch_deserializes_into_hunks_and_an_absent_one_into_nothing() {
        let detail = json!({"structuredPatch": [
            {"oldStart": 14, "oldLines": 5, "newStart": 14, "newLines": 5, "lines": [" a", "-b", "+c"]},
        ]});
        let hunks = patch(&detail).expect("one hunk");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks.first().map(|hunk| hunk.new_start), Some(14));
        assert_eq!(hunks.first().map(|hunk| hunk.lines.len()), Some(3));
        assert_eq!(patch(&json!({"structuredPatch": []})), None, "an empty patch is no patch");
        assert_eq!(patch(&json!({"filePath": "/x"})), None);
    }

    #[test]
    fn an_overflow_path_is_read_as_a_name_and_never_as_a_location() {
        let detail = json!({
            "persistedOutputPath": "/Users/fixture/.claude/projects/-Users-fixture-Developer-holodeck/11111111/tool-results/b7k2m9x4q.txt",
            "persistedOutputSize": 50104,
        });
        assert_eq!(overflow(&detail), Some(Overflow { name: "b7k2m9x4q.txt", bytes: 50104 }));
    }

    #[test]
    fn an_overflow_path_recorded_with_windows_separators_still_yields_its_name() {
        let detail = json!({"persistedOutputPath": r"C:\Users\fixture\.claude\tool-results\b7k2m9x4q.txt"});
        assert_eq!(overflow(&detail).map(|found| found.name), Some("b7k2m9x4q.txt"));
    }

    #[test]
    fn a_result_with_no_overflow_at_all_points_at_no_file() {
        assert_eq!(overflow(&json!({"stdout": "ok"})), None);
    }
}
