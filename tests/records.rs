//! The load tier against the fixture tree: every record parses or is counted, and known
//! latch names never count as drift.

#![allow(clippy::expect_used)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::fixtures;
use rewind::domain::block::{Block, Content};
use rewind::domain::latch::Latch;
use rewind::domain::lines::{LineNote, Lines};
use rewind::domain::record::{self, ParseError, Record};

const DRIFT: &str = "44444444-4444-4444-8444-444444444444.jsonl";
const FRAGMENTED: &str = "33333333-3333-4333-8333-333333333333.jsonl";

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

fn find(name: &str) -> PathBuf {
    transcripts().into_iter().find(|path| path.file_name().is_some_and(|found| found == name)).expect("the fixture is present")
}

fn parse_all(path: &Path) -> Vec<Result<Record, ParseError>> {
    let mut lines = Lines::open(path).expect("a readable fixture transcript");
    let mut parsed = Vec::new();
    while let Some(line) = lines.next_line().expect("a readable fixture transcript") {
        parsed.push(record::parse(line));
    }
    parsed
}

#[test]
fn every_record_in_the_fixture_tree_parses_or_is_a_known_defect() {
    for path in transcripts() {
        for outcome in parse_all(&path) {
            if let Err(error) = outcome {
                assert!(
                    matches!(error, ParseError::UnknownType(ref kind) if kind == "telemetry-latch"),
                    "{}: unexpected parse failure: {error}",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn the_drift_fixture_still_reports_exactly_three_defects() {
    let path = find(DRIFT);
    let outcomes = parse_all(&path);

    let unknown_records = outcomes.iter().filter(|outcome| matches!(outcome, Err(ParseError::UnknownType(_)))).count();
    assert_eq!(unknown_records, 1, "the telemetry-latch record is the only unknown record type");

    let unknown_blocks: usize = outcomes.iter().filter_map(|outcome| outcome.as_ref().ok()).map(Record::unknown_blocks).sum();
    assert_eq!(unknown_blocks, 1, "the server_tool_use block is the only unknown block");

    let mut lines = Lines::open(&path).expect("a readable fixture transcript");
    while lines.next_line().expect("a readable fixture transcript").is_some() {}
    assert_eq!(lines.notes(), [LineNote::Truncated { offset: lines.complete_offset() }]);

    let total_defects = unknown_records.saturating_add(unknown_blocks).saturating_add(lines.notes().len());
    assert_eq!(total_defects, 3, "the drift fixture's defect count moved");
}

#[test]
fn a_custom_title_record_parses_as_the_dedicated_latch_and_a_telemetry_latch_does_not() {
    let outcomes = parse_all(&find("11111111-1111-4111-8111-111111111111.jsonl"));
    assert!(outcomes.iter().any(|outcome| matches!(outcome, Ok(Record::Latch(Latch::CustomTitle(_))))));

    let outcomes = parse_all(&find(DRIFT));
    assert!(outcomes.iter().any(|outcome| matches!(outcome, Err(ParseError::UnknownType(kind)) if kind == "telemetry-latch")));
}

#[test]
fn the_new_queue_operation_pr_link_and_file_history_delta_latches_are_known_not_drift() {
    let outcomes = parse_all(&find("11111111-1111-4111-8111-111111111111.jsonl"));
    let known = outcomes.iter().filter(|outcome| matches!(outcome, Ok(Record::Latch(Latch::Known { .. })))).count();
    assert!(known >= 6, "expected the six generic latches from #29 (3 queue-operation, pr-link, 2 file-history-delta)");

    let unexpected_failures =
        outcomes.iter().filter(|outcome| matches!(outcome, Err(error) if !matches!(error, ParseError::UnknownType(_)))).count();
    assert_eq!(unexpected_failures, 0);
}

#[test]
fn the_fixture_images_deserialize_to_a_length_and_never_materialize_the_payload() {
    let outcomes = parse_all(&find(FRAGMENTED));
    let mut sources = Vec::new();
    for outcome in outcomes {
        let Ok(Record::User(user)) = outcome else { continue };
        if let Content::Blocks(blocks) = &user.message.content {
            for block in blocks {
                if let Block::Image { source } = block {
                    sources.push(source.clone());
                }
            }
        }
    }
    sources.sort_by_key(|source| source.bytes);
    let lengths: Vec<usize> = sources.iter().map(|source| source.bytes).collect();
    assert_eq!(lengths, [92, 6796], "the two fixture images did not both come through as byte lengths");
    assert!(sources.iter().all(|source| source.media_type.as_deref() == Some("image/png")));
}

#[test]
fn the_three_fragment_assistant_message_reports_usage_per_fragment() {
    let outcomes = parse_all(&find(FRAGMENTED));
    let usages: Vec<u64> = outcomes
        .into_iter()
        .filter_map(|outcome| {
            let Ok(Record::Assistant(assistant)) = outcome else { return None };
            assistant.message.usage.map(|usage| usage.output_tokens)
        })
        .collect();
    assert!(usages.len() >= 3, "expected at least the three fragments plus the retry, got {usages:?}");
}

#[test]
#[ignore = "reads the developer's real ~/.claude, not the fixture tree"]
fn every_line_in_the_real_store_parses_with_zero_defects() {
    let Some(home) = dirs_home() else { return };
    let projects = home.join(".claude").join("projects");
    if !projects.is_dir() {
        return;
    }

    let mut found = Vec::new();
    collect(&projects, &mut found);

    let mut unexpected = Vec::new();
    for path in found {
        for (line_number, outcome) in parse_all(&path).into_iter().enumerate() {
            if let Err(error) = outcome
                && !matches!(error, ParseError::UnknownType(_))
            {
                unexpected.push(format!("{}:{}: {error}", path.display(), line_number.saturating_add(1)));
            }
        }
    }
    assert!(unexpected.is_empty(), "unexpected parse failures against the real store: {unexpected:#?}");
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}
