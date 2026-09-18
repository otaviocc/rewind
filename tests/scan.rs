//! The scanner and the reader against the fixture tree: the schema they are meant to hold.

#![allow(clippy::expect_used)]

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Instant;

use common::fixtures;
use rewind::domain::lines::{LineNote, Lines};
use rewind::domain::scan::{top_level_is_null, top_level_str};

const DRIFT: &str = "44444444-4444-4444-8444-444444444444.jsonl";
const FRAGMENTED: &str = "33333333-3333-4333-8333-333333333333.jsonl";

const EVERY_RECORD_TYPE: [&str; 24] = [
    "agent-color",
    "agent-name",
    "ai-title",
    "artifact-autoreact-ledger",
    "artifact-comment-monitor",
    "assistant",
    "atis-latch",
    "attachment",
    "continued-in",
    "cost-state",
    "custom-title",
    "file-history-delta",
    "file-history-snapshot",
    "fork-context-ref",
    "frame-link",
    "last-prompt",
    "mode",
    "permission-mode",
    "pr-link",
    "queue-operation",
    "summary",
    "system",
    "telemetry-latch",
    "user",
];

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

fn every_line() -> Vec<Vec<u8>> {
    let mut all = Vec::new();
    for path in transcripts() {
        let mut lines = Lines::open(&path).expect("a readable fixture transcript");
        while let Some(line) = lines.next_line().expect("a readable fixture transcript") {
            all.push(line.to_vec());
        }
    }
    all
}

#[test]
fn every_record_in_the_fixture_tree_names_its_type() {
    let mut seen = BTreeSet::new();
    for line in every_line() {
        let found = top_level_str(&line, "type");
        assert!(found.is_some(), "a record with no top-level type: {}", String::from_utf8_lossy(&line));
        seen.extend(found.map(str::to_owned));
    }
    assert_eq!(seen, EVERY_RECORD_TYPE.iter().map(|&name| name.to_owned()).collect::<BTreeSet<_>>());
}

#[test]
fn a_nested_type_inside_message_is_never_mistaken_for_the_record_type() {
    let nested = every_line()
        .iter()
        .filter(|line| memchr::memmem::find(line, br#"{"type":"text""#).is_some())
        .filter(|line| top_level_str(line, "type") == Some("text"))
        .count();
    assert_eq!(nested, 0);
    assert!(every_line().iter().any(|line| memchr::memmem::find(line, br#"{"type":"text""#).is_some()));
}

#[test]
fn a_root_reads_as_a_null_parent_and_a_child_does_not() {
    let lines = every_line();
    let roots = lines.iter().filter(|line| top_level_is_null(line, "parentUuid")).count();
    assert_eq!(roots, 16);
    assert!(lines.iter().any(|line| top_level_str(line, "parentUuid").is_some()));
}

#[test]
fn a_latch_record_carries_a_session_id_and_no_envelope() {
    let lines = every_line();
    let latch = lines
        .iter()
        .find(|line| top_level_str(line, "type") == Some("custom-title"))
        .expect("the baseline session has a custom-title latch");
    assert!(top_level_str(latch, "sessionId").is_some());
    assert!(top_level_str(latch, "uuid").is_none());
    assert!(!top_level_is_null(latch, "parentUuid"));
}

#[test]
fn a_file_history_delta_carries_a_message_id_and_no_session_id() {
    let lines = every_line();
    let delta = lines
        .iter()
        .find(|line| top_level_str(line, "type") == Some("file-history-delta"))
        .expect("the baseline session has a file-history-delta latch");
    assert!(top_level_str(delta, "messageId").is_some());
    assert!(top_level_str(delta, "sessionId").is_none());
}

#[test]
fn a_compaction_boundary_is_a_system_record_with_a_logical_parent() {
    let lines = every_line();
    let boundary = lines
        .iter()
        .find(|line| top_level_str(line, "logicalParentUuid").is_some())
        .expect("the compaction fixture has a boundary");
    assert_eq!(top_level_str(boundary, "type"), Some("system"));
    assert!(top_level_is_null(boundary, "parentUuid"));
}

#[test]
fn the_drift_fixture_reports_exactly_one_truncated_tail() {
    let path = transcripts()
        .into_iter()
        .find(|path| path.file_name().is_some_and(|name| name == DRIFT))
        .expect("the drift fixture is present");
    let mut lines = Lines::open(&path).expect("a readable fixture transcript");
    let mut read = 0_usize;
    while lines.next_line().expect("a readable fixture transcript").is_some() {
        read = read.saturating_add(1);
    }
    assert!(read > 0);
    assert_eq!(lines.notes(), [LineNote::Truncated { offset: lines.complete_offset() }]);
    let length = fs::metadata(&path).expect("a stattable fixture").len();
    assert!(lines.complete_offset() < length, "the truncated tail is not part of any complete line");
}

#[test]
fn the_fixture_images_sit_below_the_redaction_floor_and_survive_untouched() {
    let path = transcripts()
        .into_iter()
        .find(|path| path.file_name().is_some_and(|name| name == FRAGMENTED))
        .expect("the fragmentation fixture is present");
    let raw = fs::read(&path).expect("a readable fixture transcript");
    let images: Vec<&[u8]> =
        raw.split(|&byte| byte == b'\n').filter(|line| memchr::memmem::find(line, br#""type":"base64""#).is_some()).collect();
    assert_eq!(images.len(), 2);

    let mut lines = Lines::open(&path).expect("a readable fixture transcript");
    let mut read = Vec::new();
    while let Some(line) = lines.next_line().expect("a readable fixture transcript") {
        read.push(line.to_vec());
    }
    for image in images {
        assert!(read.iter().any(|line| line == image), "an inline image was altered on the way through");
    }
}

#[test]
fn a_hundred_megabytes_of_records_scan_in_seconds_not_minutes() {
    let corpus_floor: usize = 100 * 1024 * 1024;
    let source = every_line();
    let mut corpus = Vec::with_capacity(corpus_floor.saturating_add(1024));
    while corpus.len() < corpus_floor {
        for line in &source {
            corpus.extend_from_slice(line);
            corpus.push(b'\n');
        }
    }

    let started = Instant::now();
    let mut lines = Lines::new(Cursor::new(&corpus));
    let mut records = 0_usize;
    let mut roots = 0_usize;
    while let Some(line) = lines.next_line().expect("an in-memory reader never fails") {
        if top_level_str(line, "type").is_some() {
            records = records.saturating_add(1);
        }
        if top_level_is_null(line, "parentUuid") {
            roots = roots.saturating_add(1);
        }
    }
    let elapsed = started.elapsed();

    assert!(records > 0 && roots > 0);
    assert!(
        elapsed.as_secs() < 20,
        "scanning {} bytes took {elapsed:?}; something is calling serde_json or walking twice",
        corpus.len()
    );
}
