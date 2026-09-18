//! The fixture tree, guarded against quietly losing the cases it exists to reproduce.

#![allow(clippy::expect_used)]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use common::{FixtureTree, fixture_root, fixture_tree, fixtures, rewind};

const HOLODECK: &str = "-Users-fixture-Developer-holodeck";
const BASELINE: &str = "11111111-1111-4111-8111-111111111111";
const DRIFT: &str = "44444444-4444-4444-8444-444444444444";

fn transcripts() -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect(&fixtures().join("projects"), &mut found);
    found.sort();
    found
}

fn collect(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("a readable fixture directory") {
        let path = entry.expect("a readable fixture entry").path();
        if path.is_dir() {
            collect(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "jsonl") {
            found.push(path);
        }
    }
}

fn every_line() -> String {
    transcripts().iter().map(|path| read_lossy(path)).collect::<Vec<_>>().join("\n")
}

fn read_lossy(path: &Path) -> String {
    String::from_utf8_lossy(&fs::read(path).expect("a readable fixture file")).into_owned()
}

#[test]
fn the_binary_reads_the_fixture_tree() {
    let output = rewind().arg("--claude-dir").arg(fixtures()).output().expect("rewind runs");
    assert!(output.status.success(), "rewind --claude-dir tests/data/claude failed: {}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn the_projects_map_is_a_sibling_of_the_claude_directory() {
    assert!(fixture_root().join(".claude.json").is_file(), "the projects map is not beside claude/");
    assert!(!fixtures().join(".claude.json").is_file(), "a projects map inside claude/ would not match any real installation");
}

#[test]
fn exactly_one_line_in_the_tree_is_unparseable() {
    let mut unparseable = Vec::new();
    for path in transcripts() {
        for (number, line) in read_lossy(&path).lines().enumerate() {
            if serde_json::from_str::<serde_json::Value>(line).is_err() {
                unparseable.push(format!("{}:{}", path.display(), number.saturating_add(1)));
            }
        }
    }
    assert_eq!(unparseable.len(), 1, "expected only the truncated tail, got {unparseable:?}");
    assert!(unparseable[0].contains(DRIFT), "the unparseable line moved: {unparseable:?}");
}

#[test]
fn the_truncated_transcript_has_no_trailing_newline() {
    let bytes = fs::read(fixtures().join("projects").join(HOLODECK).join(format!("{DRIFT}.jsonl"))).expect("the drift fixture");
    assert_ne!(bytes.last(), Some(&b'\n'), "the truncated tail was repaired");
    assert!(!bytes.contains(&b'\r'), "a CRLF checkout rewrote the fixture bytes");
}

#[test]
fn no_transcript_carries_carriage_returns() {
    for path in transcripts() {
        let bytes = fs::read(&path).expect("a readable fixture file");
        assert!(!bytes.contains(&b'\r'), "{} has CRLF line endings", path.display());
    }
}

#[test]
fn the_deliberate_ds_store_survives_a_checkout() {
    assert!(fixtures().join("projects").join(".DS_Store").is_file(), "the .DS_Store was dropped, probably by a global gitignore");
}

#[test]
fn every_nasty_case_is_still_present() {
    let corpus = every_line();
    for (needle, what) in [
        (r#""logicalParentUuid""#, "the compaction stitch"),
        (r#""subtype":"compact_boundary""#, "compact_boundary as a system subtype"),
        (r#""isSidechain":true"#, "sidechain records"),
        (r#""apiBlockIndex""#, "fragment ordering"),
        (r#""type":"summary""#, "the legacy summary record"),
        (r#""type":"image""#, "an inline base64 image"),
        (r#""type":"attachment""#, "context injections"),
        (r#""persistedOutputPath""#, "overflowed tool output"),
        (r#""structuredPatch""#, "a diff to render an Edit or a Write from"),
        (r#""name":"Edit""#, "an Edit call"),
        (r#""name":"Write""#, "a Write call"),
        (r#""name":"mcp__"#, "an MCP tool name"),
        (r#""__unparsedToolInput""#, "a tool input the CLI could not parse"),
        ("[Request interrupted by user for tool use]", "an interrupted call"),
        ("The user doesn't want to proceed with this tool use", "a denied call"),
        (r#""origin":{"kind":"peer""#, "a peer-origin user turn"),
        (r#""isMeta":true"#, "meta user turns"),
        (r#""type":"server_tool_use""#, "an unknown content block"),
        (r#""type":"telemetry-latch""#, "an unknown record type"),
        (r#""name":"Task""#, "the legacy tool name Task"),
        (r#""name":"Grep""#, "the legacy tool name Grep"),
        (r#""name":"Glob""#, "the legacy tool name Glob"),
        (r#""name":"TodoWrite""#, "the legacy tool name TodoWrite"),
        (r#""type":"queue-operation""#, "a queue-operation latch"),
        (r#""type":"pr-link""#, "a pr-link latch"),
        (r#""type":"file-history-delta""#, "a file-history-delta latch"),
        (r#""reason":"absorbed_mid_turn""#, "a queue-operation removal reason"),
        (r#""continuedInSessionId""#, "the observed continued-in field name"),
        ("```rust", "a fenced code block in assistant prose"),
        ("| :-------- | :--: | ----: |", "a Markdown table with alignments"),
        ("- [x] recalibrate the grid", "a task list"),
    ] {
        assert!(corpus.contains(needle), "the fixtures lost {what}");
    }
}

#[test]
fn one_overflow_file_is_referenced_and_one_is_deliberately_orphaned() {
    let overflow = fixtures().join("projects").join(HOLODECK).join(BASELINE).join("tool-results");
    let corpus = every_line();
    let mut referenced = Vec::new();
    let mut orphaned = Vec::new();
    for entry in fs::read_dir(&overflow).expect("a readable tool-results directory") {
        let path = entry.expect("a readable tool-results entry").path();
        let name = path.file_name().expect("a named overflow file").to_string_lossy().into_owned();
        if corpus.contains(&name) { referenced.push(name) } else { orphaned.push(name) }
    }
    referenced.sort();
    orphaned.sort();
    assert_eq!(referenced, ["b7k2m9x4q.txt"], "the referenced overflow file moved");
    assert_eq!(orphaned, ["b0orphan1.txt"], "real sessions accumulate overflow files no record points at");
}

#[test]
fn the_title_latches_prove_both_precedence_and_recency() {
    let path = fixtures().join("projects").join(HOLODECK).join(format!("{BASELINE}.jsonl"));
    let text = read_lossy(&path);
    let kinds: Vec<&str> = text
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|record| record.get("type")?.as_str().map(str::to_owned))
        .filter(|kind| matches!(kind.as_str(), "ai-title" | "custom-title" | "agent-name" | "last-prompt"))
        .map(|kind| match kind.as_str() {
            "ai-title" => "ai-title",
            "custom-title" => "custom-title",
            "agent-name" => "agent-name",
            _ => "last-prompt",
        })
        .collect();
    assert_eq!(
        kinds,
        ["ai-title", "agent-name", "last-prompt", "custom-title", "ai-title"],
        "the latch order no longer proves that custom-title wins on precedence and the last ai-title on recency"
    );
    assert!(
        read_lossy(&fixtures().join("projects").join(HOLODECK).join(BASELINE).join("custom-title.json"))
            .contains("beaten by the record"),
        "custom-title.json must differ from the custom-title record"
    );
}

#[test]
fn every_subagent_meta_falls_into_exactly_one_of_the_four_ways_it_can_be_reached() {
    let subagents = fixtures().join("projects").join(HOLODECK).join(BASELINE).join("subagents");
    let session = read_lossy(&fixtures().join("projects").join(HOLODECK).join(format!("{BASELINE}.jsonl")));

    let mut joined = BTreeSet::new();
    let mut nested = BTreeSet::new();
    let mut unlinked = BTreeSet::new();
    let mut dangling = BTreeSet::new();
    for entry in fs::read_dir(&subagents).expect("the subagents directory") {
        let path = entry.expect("a readable entry").path();
        let name = path.file_name().expect("a file name").to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".meta.json") else { continue };
        let meta: serde_json::Value = serde_json::from_str(&read_lossy(&path)).expect("a parseable subagent meta");
        assert!(meta.get("agentType").is_some(), "{name} has no agentType");
        assert!(meta.get("spawnDepth").is_some(), "{name} has no spawnDepth");

        let Some(id) = meta.get("toolUseId").and_then(serde_json::Value::as_str) else {
            unlinked.insert(stem.to_owned());
            continue;
        };
        match meta.get("parentAgentId").and_then(serde_json::Value::as_str) {
            Some(parent) => {
                let inside = read_lossy(&subagents.join(format!("agent-{parent}.jsonl")));
                assert!(inside.contains(id), "{name} is depth 2 but its toolUseId is not in agent-{parent}");
                assert!(!session.contains(id), "a depth-2 toolUseId must not resolve against the session");
                nested.insert(stem.to_owned());
            }
            None if session.contains(id) => {
                joined.insert(stem.to_owned());
            }
            None => {
                dangling.insert(stem.to_owned());
            }
        }
    }

    let names = |set: &BTreeSet<String>| set.iter().cloned().collect::<Vec<_>>();
    assert_eq!(names(&joined), ["agent-a1b2c3d4e5f607182"], "joined by its own toolUseId");
    assert_eq!(names(&nested), ["agent-e5f60718293a4b5c6"], "depth 2, joined inside its parent agent");
    assert_eq!(
        names(&unlinked),
        ["agent-b2c3d4e5f60718293", "agent-d4e5f60718293a4b5"],
        "forked skills: one reachable only by listing, one only via the result's agentId"
    );
    assert_eq!(names(&dangling), ["agent-c3d4e5f607182934a"], "a toolUseId that resolves nowhere");

    assert!(!subagents.join("agent-c3d4e5f607182934a.jsonl").exists(), "the dangling meta must have no transcript beside it");
    assert!(
        session.contains(r#""agentId":"d4e5f60718293a4b5""#),
        "the result's agentId is the only key that reaches the forked skill"
    );
}

#[test]
fn three_projects_map_keys_collapse_onto_one_directory() {
    let map: serde_json::Value =
        serde_json::from_str(&read_lossy(&fixture_root().join(".claude.json"))).expect("the projects map");
    let keys: Vec<String> =
        map.get("projects").and_then(serde_json::Value::as_object).expect("a projects object").keys().cloned().collect();

    let mut by_encoded: BTreeMap<String, usize> = BTreeMap::new();
    for key in &keys {
        let encoded: String = key.chars().map(|c| if matches!(c, '/' | '.' | ' ') { '-' } else { c }).collect();
        *by_encoded.entry(encoded).or_default() += 1;
    }
    assert_eq!(by_encoded.get("-Users-fixture-Developer-warp-core"), Some(&3), "the ambiguous encoding case is gone");
    assert!(
        !keys.iter().any(|key| key.ends_with("/jeffries-tube")),
        "jeffries-tube must stay absent from the map so the cwd fallback is exercised"
    );
}

#[test]
fn the_fixture_tree_helper_is_deterministic_and_rewrites_the_home() {
    let first = fixture_tree();
    let second = fixture_tree();

    let map = fs::read_to_string(first.home().join(".claude.json")).expect("the copied projects map");
    assert!(!map.contains("/Users/fixture"), "the fixture home was not rewritten");
    assert!(map.contains(&first.home().to_string_lossy().into_owned()), "the tempdir root is missing");

    assert!(first.working_copy("Developer/holodeck").is_dir(), "the present working copy is missing");
    assert!(!first.working_copy("Developer/nomad").exists(), "nomad must stay absent");

    assert_eq!(mtimes(&first), mtimes(&second), "mtimes are not deterministic");
}

#[test]
fn the_project_directory_names_are_re_encoded_from_the_rewritten_home() {
    let tree = fixture_tree();
    let holodeck = tree.claude_dir().join("projects").join(tree.project_dir("Developer/holodeck"));

    assert!(holodeck.is_dir(), "the project directory was not re-encoded to the tempdir home");
    assert!(
        !tree.claude_dir().join("projects").join("-Users-fixture-Developer-holodeck").exists(),
        "a directory encoding /Users/fixture cannot coexist with a map that names the tempdir"
    );
}

fn mtimes(tree: &FixtureTree) -> BTreeMap<String, i64> {
    let root = tree.claude_dir();
    let mut found = BTreeMap::new();
    walk(&root, &root, &mut found);
    found.into_iter().map(|(path, stamp)| (path.replace(&encoded_home(tree), "-HOME"), stamp)).collect()
}

fn encoded_home(tree: &FixtureTree) -> String {
    tree.home()
        .to_string_lossy()
        .chars()
        .map(|character| if matches!(character, '/' | '.' | ' ') { '-' } else { character })
        .collect()
}

fn walk(path: &Path, root: &Path, found: &mut BTreeMap<String, i64>) {
    let relative = path.strip_prefix(root).expect("a path under the root").to_string_lossy().into_owned();
    let stamp = filetime::FileTime::from_last_modification_time(&fs::metadata(path).expect("readable metadata"));
    found.insert(relative, stamp.unix_seconds());
    if path.is_dir() {
        for entry in fs::read_dir(path).expect("a readable directory") {
            walk(&entry.expect("a readable entry").path(), root, found);
        }
    }
}
