//! Rendering a real fixture session to plain text: the exit criteria in #9.

#![allow(clippy::expect_used)]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use common::fixtures;
use rewind::domain::subagent::{self, Agents};
use rewind::domain::thread;
use rewind::domain::tool::{self, Outcome};
use rewind::render::line::RenderedLine;
use rewind::render::message::Transcript;
use rewind::render::message::transcript;
use rewind::render::{Ctx, Expanded, Outputs, Overflow};
use tempfile::TempDir;

const HOLODECK: &str = "-Users-fixture-Developer-holodeck";
const BASELINE: &str = "11111111-1111-4111-8111-111111111111";
const WIDE: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const IMAGES: &str = "33333333-3333-4333-8333-333333333333";
const MARKDOWN: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const TOOLS: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
const LEGACY: &str = "44444444-4444-4444-8444-444444444444";

fn collect(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "jsonl") {
            found.push(path);
        }
    }
}

fn session_path(session: &str) -> PathBuf {
    fixtures().join("projects").join(HOLODECK).join(format!("{session}.jsonl"))
}

#[derive(Default)]
struct View {
    expanded: Expanded,
    outputs: Outputs,
    agents: Agents,
    root: Option<rewind::domain::thread::NodeId>,
}

impl View {
    const fn ctx(&self, width: usize) -> Ctx<'_> {
        Ctx { width, expanded: &self.expanded, outputs: &self.outputs, agents: &self.agents, root: self.root }
    }

    fn expanding(path: &Path) -> Self {
        let conversation = thread::build(path).expect("a built conversation");
        let mut view = Self { agents: subagent::discover(path), ..Self::default() };
        for (id, detail) in ids(&conversation) {
            view.expanded.insert(id.clone());
            if let Some(found) = detail.as_ref().and_then(tool::overflow) {
                let read = tool::read_overflow(&tool::overflow_path(path, found.name));
                let overflow = match read {
                    Ok(lines) => Overflow::Lines(std::sync::Arc::new(lines)),
                    Err(error) => Overflow::Failed(error),
                };
                view.outputs.insert(id, overflow);
            }
        }
        view
    }
}

fn ids(conversation: &rewind::domain::thread::Conversation) -> Vec<(Box<str>, Option<serde_json::Value>)> {
    let mut found = Vec::new();
    for &node in conversation.thread() {
        let Some(node) = conversation.node(node) else { continue };
        let rewind::domain::thread::NodeKind::Assistant(turn) = &node.kind else { continue };
        for block in &turn.content {
            let rewind::domain::block::Block::ToolUse { id, .. } = block else { continue };
            let detail =
                conversation.result_of(id).and_then(|node| Outcome::of(node, id)).and_then(|outcome| outcome.detail).cloned();
            found.push((Box::from(id.as_str()), detail));
        }
    }
    found
}

fn rendered(session: &str, width: usize) -> String {
    render_file(&session_path(session), width)
}

fn render_file(path: &Path, width: usize) -> String {
    lines_of(&built(path, width, &View { agents: subagent::discover(path), ..View::default() }))
}

fn expanded(session: &str, width: usize) -> String {
    let path = session_path(session);
    let view = View::expanding(&path);
    lines_of(&built(&path, width, &view))
}

fn built(path: &Path, width: usize, view: &View) -> Transcript {
    let conversation = thread::build(path).expect("a built conversation");
    transcript(&conversation, &view.ctx(width))
}

fn lines_of(transcript: &Transcript) -> String {
    transcript.lines.iter().map(RenderedLine::text).collect::<Vec<_>>().join("\n")
}

fn widths(path: &Path, width: usize) -> Vec<usize> {
    let view = View { agents: subagent::discover(path), ..View::default() };
    built(path, width, &view).lines.iter().map(RenderedLine::width).collect()
}

fn expanded_widths(path: &Path, width: usize) -> Vec<usize> {
    let view = View::expanding(path);
    built(path, width, &view).lines.iter().map(RenderedLine::width).collect()
}

#[test]
fn the_baseline_session_renders_as_plain_text() {
    insta::assert_snapshot!("baseline-80", rendered(BASELINE, 80));
}

#[test]
fn the_baseline_session_rewraps_for_a_narrow_column() {
    insta::assert_snapshot!("baseline-32", rendered(BASELINE, 32));
}

#[test]
fn the_baseline_session_rewraps_for_a_forty_column_pane() {
    insta::assert_snapshot!("baseline-40", rendered(BASELINE, 40));
}

#[test]
fn a_session_of_wide_glyphs_renders_and_wraps_by_display_width() {
    insta::assert_snapshot!("wide-glyphs-40", rendered(WIDE, 40));
}

#[test]
fn a_markdown_session_renders_every_element_at_a_readable_column() {
    insta::assert_snapshot!("markdown-80", rendered(MARKDOWN, 80));
}

#[test]
fn a_markdown_session_renders_every_element_at_a_wide_column() {
    insta::assert_snapshot!("markdown-200", rendered(MARKDOWN, 200));
}

#[test]
fn every_tool_call_renders_its_name_its_digest_and_its_outcome() {
    insta::assert_snapshot!("tools-80", rendered(TOOLS, 80));
}

#[test]
fn a_tool_call_stays_one_line_however_narrow_the_column() {
    insta::assert_snapshot!("tools-32", rendered(TOOLS, 32));
}

#[test]
fn every_line_of_the_tool_surface_fits_the_column_it_was_wrapped_for() {
    let path = session_path(TOOLS);
    for width in [8, 12, 20, 32, 40, 80, 120, 200] {
        for line in widths(&path, width) {
            assert!(line <= width, "a line of {line} columns was wrapped for {width}");
        }
        for line in expanded_widths(&path, width) {
            assert!(line <= width, "an expanded line of {line} columns was wrapped for {width}");
        }
    }
}

#[test]
fn an_expanded_call_shows_its_input_its_output_and_its_diff() {
    insta::assert_snapshot!("tools-expanded-80", expanded(TOOLS, 80));
}

#[test]
fn an_expanded_call_degrades_rather_than_overflows_a_narrow_column() {
    insta::assert_snapshot!("tools-expanded-32", expanded(TOOLS, 32));
}

#[test]
fn an_overflowed_result_is_read_from_the_sidecar_beside_the_session_and_folded() {
    let text = expanded(BASELINE, 80);
    assert!(text.contains("Fresh   memchr"), "the sidecar was not read:\n{text}");
    assert!(text.contains("more lines"), "a two-thousand-line sidecar was not folded:\n{text}");
    assert!(!text.contains("<persisted-output>"), "the preamble reached the transcript:\n{text}");

    let sidecar = tool::overflow_path(&session_path(BASELINE), "b7k2m9x4q.txt");
    let whole = tool::read_overflow(&sidecar).expect("the sidecar reads");
    assert!(whole.len() > 1_000, "the fixture stopped being large enough to fold, at {} lines", whole.len());
    let on_screen = text.lines().count();
    assert!(on_screen < whole.len() / 10, "a {}-line result must not put {on_screen} lines on screen", whole.len());
}

#[test]
fn an_expanded_call_with_no_sidecar_loaded_says_so_rather_than_showing_the_preamble() {
    let path = session_path(BASELINE);
    let mut view = View::default();
    for (id, _) in ids(&thread::build(&path).expect("a built conversation")) {
        view.expanded.insert(id);
    }
    let text = lines_of(&built(&path, 80, &view));
    assert!(text.contains("reading b7k2m9x4q.txt …"), "no progress line for an unread sidecar:\n{text}");
}

#[test]
fn every_expanded_call_is_anchored_to_the_line_its_header_is_on() {
    let path = session_path(TOOLS);
    let view = View::expanding(&path);
    let transcript = built(&path, 80, &view);
    assert_eq!(transcript.anchors.len(), 9, "nine calls in the tool surface");
    for anchor in &transcript.anchors {
        let line = transcript.lines.get(anchor.line).map(RenderedLine::text).unwrap_or_default();
        assert!(line.contains("▾ "), "anchor {} does not point at an expanded header: {line:?}", anchor.line);
    }
    let lines: Vec<usize> = transcript.anchors.iter().map(|anchor| anchor.line).collect();
    let mut sorted = lines.clone();
    sorted.sort_unstable();
    assert_eq!(lines, sorted, "anchors are in render order");
}

#[test]
fn the_legacy_tool_names_are_digested_the_same_way_their_successors_are() {
    let text = rendered(LEGACY, 80);
    for expected in ["▸ Task  Audit the old shapes", "▸ Glob  **/*.jsonl", "▸ TodoWrite  1 of 1 done"] {
        assert!(text.contains(expected), "{expected:?} missing from:\n{text}");
    }
}

#[test]
fn every_line_of_a_markdown_session_fits_the_column_it_was_wrapped_for() {
    let path = session_path(MARKDOWN);
    for width in [12, 20, 32, 40, 80, 120, 200] {
        for line in widths(&path, width) {
            assert!(line <= width, "a line of {line} columns was wrapped for {width}");
        }
    }
}

#[test]
fn every_line_of_a_wide_glyph_session_fits_the_column_it_was_wrapped_for() {
    let path = session_path(WIDE);
    for width in [16, 24, 40, 80] {
        for line in widths(&path, width) {
            assert!(line <= width, "a line of {line} columns was wrapped for {width}");
        }
    }
}

#[test]
fn an_inline_image_is_named_and_never_decoded() {
    let text = rendered(IMAGES, 80);
    assert!(text.contains("[image · png · "), "no image line in:\n{text}");
    assert!(!text.contains("iVBOR"), "a base64 payload reached the transcript");
}

fn huge_session() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("a temporary directory");
    let path = dir.path().join("huge.jsonl");
    let blob = "A".repeat(4 * 1024 * 1024);
    let prose = "the deflector array reads back one plate at a time ".repeat(40);
    let mut text = String::new();
    let _ = write!(
        text,
        r#"{{"parentUuid":null,"isSidechain":false,"type":"user","uuid":"u0","timestamp":"2026-01-01T00:00:00Z","sessionId":"h","origin":{{"kind":"human"}},"message":{{"role":"user","content":[{{"type":"text","text":"look"}},{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{blob}"}}}}]}}}}"#
    );
    text.push('\n');
    for index in 0..8_000u32 {
        let parent = if index == 0 { "u0".to_owned() } else { format!("a{}", index.saturating_sub(1)) };
        let _ = write!(
            text,
            r#"{{"parentUuid":"{parent}","isSidechain":false,"type":"assistant","uuid":"a{index}","timestamp":"2026-01-01T00:00:01Z","sessionId":"h","requestId":"r{index}","message":{{"model":"opus-5","id":"msg_{index}","role":"assistant","content":[{{"type":"text","text":"{prose}"}}]}}}}"#
        );
        text.push('\n');
    }
    fs::write(&path, text).expect("a written transcript");
    (dir, path)
}

#[test]
fn a_ten_megabyte_session_renders_without_decoding_a_byte_of_base64() {
    let (_dir, path) = huge_session();
    assert!(fs::metadata(&path).expect("the transcript exists").len() > 10 * 1024 * 1024);

    let conversation = thread::build(&path).expect("a built conversation");
    let view = View::default();
    let lines = transcript(&conversation, &view.ctx(80)).lines;

    assert!(lines.len() > 8_000, "only {} lines", lines.len());
    assert!(lines.iter().all(|line| line.width() <= 80));
    assert!(!lines.iter().any(|line| line.text().contains("AAAAAAAAAAAAAAAA")), "a base64 payload reached the transcript");
    assert!(lines.iter().any(|line| line.text().contains("[image · png · ")), "the image was not named");
}

fn calls(conversation: &rewind::domain::thread::Conversation) -> Vec<(String, serde_json::Value, bool)> {
    let mut found = Vec::new();
    for &id in conversation.thread() {
        let Some(node) = conversation.node(id) else { continue };
        let rewind::domain::thread::NodeKind::Assistant(turn) = &node.kind else { continue };
        for block in &turn.content {
            let rewind::domain::block::Block::ToolUse { id, name, input } = block else { continue };
            let detail = conversation.result_of(id).and_then(|node| Outcome::of(node, id)).and_then(|outcome| outcome.detail);
            let digest = tool::digest(name, input, detail);
            found.push((name.clone(), input.clone(), digest.primary.is_some()));
        }
    }
    found
}

fn has_input(input: &serde_json::Value) -> bool {
    input.as_object().is_some_and(|fields| !fields.is_empty())
}

#[test]
fn every_tool_call_in_the_fixture_tree_that_was_given_an_input_digests_to_something() {
    let mut names = BTreeSet::new();
    for session in [BASELINE, IMAGES, LEGACY, TOOLS, WIDE] {
        let conversation = thread::build(&session_path(session)).expect("a built conversation");
        for (name, input, digested) in calls(&conversation) {
            assert!(digested || !has_input(&input), "{name} was given {input} and digested to nothing");
            names.insert(name);
        }
    }
    assert_eq!(
        names.iter().map(String::as_str).collect::<Vec<_>>(),
        [
            "Agent",
            "Bash",
            "Edit",
            "Glob",
            "Grep",
            "Read",
            "Replicator",
            "Task",
            "TodoWrite",
            "WebFetch",
            "Write",
            "mcp__jeffries__beam_status"
        ],
        "the fixture tree stopped covering a tool shape"
    );
}

#[test]
#[ignore = "reads the developer's real ~/.claude, not the fixture tree"]
fn every_tool_name_in_the_real_store_digests_to_something_when_it_was_given_an_input() {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return };
    let projects = home.join(".claude").join("projects");
    if !projects.is_dir() {
        return;
    }

    let mut transcripts = Vec::new();
    collect(&projects, &mut transcripts);

    let mut digested: BTreeMap<String, bool> = BTreeMap::new();
    let mut inputless: BTreeSet<String> = BTreeSet::new();
    for path in transcripts {
        let Ok(conversation) = thread::build(&path) else { continue };
        for (name, input, found) in calls(&conversation) {
            if has_input(&input) {
                let entry = digested.entry(name).or_default();
                *entry = *entry || found;
            } else {
                inputless.insert(name);
            }
        }
    }

    assert!(!digested.is_empty(), "no tool call was read out of the real store at all");
    println!("{} tool names with an input, {} without", digested.len(), inputless.len());
    println!("no input at all, so nothing to summarise: {inputless:?}");
    let blank: Vec<&String> = digested.iter().filter(|(_, found)| !**found).map(|(name, _)| name).collect();
    assert!(blank.is_empty(), "these tool names were given an input and digested to nothing: {blank:#?}");
}
