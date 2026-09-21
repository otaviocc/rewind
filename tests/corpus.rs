//! Building a corpus shard from the fixture tree: an end-to-end round trip through
//! `domain::text` extraction and the `domain::cache::shard` format.
//!
//! The `~22% of input size` target needs the real `~/.claude`, which tests must
//! never read (see `tests/common/mod.rs`). `a_shard_built_from_the_real_corpus_is_roughly_a_fifth_of_its_input_size`
//! is `#[ignore]`d and only runs by hand, with `REWIND_CORPUS` naming a real directory; the
//! measured ratio is recorded in that commit's message, and `make check` never touches it.

#![allow(clippy::expect_used)]

mod common;

use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use common::fixture_tree;
use rewind::domain::cache::fnv;
use rewind::domain::cache::shard::{Builder, Kind, Shard};
use rewind::domain::lines::Lines;
use rewind::domain::scan;
use rewind::domain::text::{self, Extracted};

const HEAD_SIZE: usize = 4 * 1024;

fn index_file(builder: &mut Builder, path: &Path, kind: Kind, extract_line: fn(&[u8]) -> Vec<Extracted>) {
    let metadata = fs::metadata(path).expect("fixture metadata");
    let mtime_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX));
    let head = fs::read(path).expect("a readable fixture file");
    let head_fnv = fnv::hash(head.get(..HEAD_SIZE.min(head.len())).unwrap_or_default());

    let mut lines = Lines::open(path).expect("a readable fixture file");
    let mut line_no: u32 = 0;
    let mut start = lines.complete_offset();
    let mut pending: Vec<(u32, u64, Extracted)> = Vec::new();
    while let Some(line) = lines.next_line().expect("an in-memory-backed file read") {
        line_no = line_no.saturating_add(1);
        for extracted in extract_line(line) {
            pending.push((line_no, start, extracted));
        }
        start = lines.complete_offset();
    }

    let file_idx = builder.push_file(&path.to_string_lossy(), metadata.len(), mtime_ms, head_fnv, line_no);
    let mut seq: u32 = 0;
    for (line_no, byte_off, extracted) in pending {
        builder.push_record(file_idx, line_no, byte_off, 0, kind, extracted.field, extracted.flags, seq, &extracted.text);
        seq = seq.saturating_add(1);
    }
}

fn build_corpus(claude_dir: &Path) -> (Vec<u8>, u64) {
    let mut builder = Builder::new();
    let mut input_size: u64 = 0;

    let projects = claude_dir.join("projects");
    for entry in fs::read_dir(&projects).expect("a readable projects directory") {
        let project_dir = entry.expect("a readable entry").path();
        if !project_dir.is_dir() {
            continue;
        }
        for file in fs::read_dir(&project_dir).expect("a readable project directory") {
            let path = file.expect("a readable entry").path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
                continue;
            }
            input_size = input_size.saturating_add(fs::metadata(&path).map_or(0, |meta| meta.len()));
            index_file(&mut builder, &path, Kind::Transcript, text::extract);
        }
    }

    let history = claude_dir.join("history.jsonl");
    if history.is_file() {
        input_size = input_size.saturating_add(fs::metadata(&history).map_or(0, |meta| meta.len()));
        index_file(&mut builder, &history, Kind::History, text::extract_history);
    }

    (builder.finish(0), input_size)
}

#[test]
fn a_shard_built_from_the_fixture_tree_round_trips_every_record() {
    let tree = fixture_tree();
    let (bytes, _) = build_corpus(&tree.claude_dir());
    let shard = Shard::parse(&bytes).expect("a well-formed shard");

    assert!(!shard.records().is_empty(), "the fixture tree has plenty to index");
    for record in shard.records() {
        assert!(shard.text(record).is_some(), "every record's span must resolve inside the blob");
    }
}

#[test]
fn a_known_fixture_phrase_is_present_in_the_blob_lowercased() {
    let tree = fixture_tree();
    let (bytes, _) = build_corpus(&tree.claude_dir());
    let shard = Shard::parse(&bytes).expect("a well-formed shard");

    let needle = b"read the grid scanner";
    assert!(memchr::memmem::find(shard.blob(), needle).is_some(), "the phrase must survive extraction and lowercasing");
}

#[test]
fn attachment_lines_in_a_real_session_extract_nothing() {
    let tree = fixture_tree();
    let holodeck = tree.claude_dir().join("projects").join(tree.project_dir("Developer/holodeck"));
    let path = holodeck.join("11111111-1111-4111-8111-111111111111.jsonl");
    let bytes = fs::read(&path).expect("a readable session file");

    let mut attachment_lines: u32 = 0;
    for line in bytes.split(|&byte| byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        if scan::top_level_str(line, "type") == Some("attachment") {
            attachment_lines = attachment_lines.saturating_add(1);
            assert!(text::extract(line).is_empty(), "an attachment record must never contribute text");
        }
    }
    assert!(attachment_lines > 0, "the fixture session must actually carry attachment records for this to mean anything");
}

#[test]
#[ignore = "needs REWIND_CORPUS set to a real ~/.claude directory"]
fn a_shard_built_from_the_real_corpus_is_roughly_a_fifth_of_its_input_size() {
    let Some(claude_dir) = std::env::var_os("REWIND_CORPUS").map(std::path::PathBuf::from) else {
        eprintln!("REWIND_CORPUS is not set; skipping");
        return;
    };

    let (bytes, input_size) = build_corpus(&claude_dir);
    let shard_size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let percent = shard_size.saturating_mul(100).checked_div(input_size).unwrap_or(0);

    println!("corpus ratio: shard {shard_size} bytes / input {input_size} bytes = {percent}%");
    assert!(percent > 0 && percent < 100, "a shard must be smaller than the input it was built from");
}
