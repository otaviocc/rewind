//! `meta.json`: one small, plain-JSON file for the whole cache store.
//!
//! Fully regenerated on every build — session summaries the shard's blob doesn't carry
//! (titles, counts, branches, timestamps, cwd), plus a metadata-tier approximation of the
//! conversation forest's shape.
//!
//! "Root and branch counts" here are **not** `domain::thread::build`'s authoritative forest —
//! that needs a full load-tier parse per session (compaction re-parenting, cycle severing,
//! sidechain restitching), too slow to run over the whole store in the cache's time budget.
//! A root is a non-sidechain `user`/`assistant`/`system` record whose effective parent
//! (`logicalParentUuid` for a `compact_boundary`, else `parentUuid`) is null or absent; a
//! branch is an effective-parent shared by 2+ `assistant` or human-turn `user` records. This
//! matches `thread::build` on well-formed sessions and only diverges on already-corrupt ones,
//! which a session's own diagnostics count already flags separately.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::domain::cache::atomic;
use crate::domain::scan;
use crate::domain::session;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub cache_version: u16,
    pub built_at_s: i64,
    pub projects: Vec<MetaProject>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaProject {
    pub directory: String,
    pub sessions: Vec<MetaSession>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaSession {
    pub id: String,
    pub title: String,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub first_activity_s: Option<i64>,
    pub last_activity_s: Option<i64>,
    pub records: u64,
    pub messages: u64,
    pub roots: u32,
    pub branches: u32,
}

pub fn write(path: &Path, meta: &Meta) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(meta)?;
    atomic::write(path, &bytes)
}

pub fn read(path: &Path) -> Option<Meta> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[derive(Debug, Default)]
pub struct ForestTally {
    roots: u32,
    children: HashMap<String, u32>,
}

impl ForestTally {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, line: &[u8]) {
        let Some(kind) = scan::top_level_str(line, "type") else { return };
        if !matches!(kind, "user" | "assistant" | "system") {
            return;
        }
        if scan::top_level_true(line, "isSidechain") {
            return;
        }

        let is_compact_boundary = kind == "system" && scan::top_level_str(line, "subtype") == Some("compact_boundary");
        let effective_parent = if is_compact_boundary {
            scan::top_level_str(line, "logicalParentUuid").or_else(|| scan::top_level_str(line, "parentUuid"))
        } else {
            scan::top_level_str(line, "parentUuid")
        };

        let Some(parent) = effective_parent else {
            self.roots = self.roots.saturating_add(1);
            return;
        };

        let counts_as_turn_child = match kind {
            "assistant" => true,
            "user" => session::is_human_turn(&session::scan_line(line)),
            _ => false,
        };
        if counts_as_turn_child {
            self.children.entry(parent.to_owned()).and_modify(|count| *count = count.saturating_add(1)).or_insert(1);
        }
    }

    pub fn finish(&self) -> (u32, u32) {
        let branches = u32::try_from(self.children.values().filter(|&&count| count >= 2).count()).unwrap_or(u32::MAX);
        (self.roots, branches)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    fn sample() -> Meta {
        Meta {
            cache_version: 1,
            built_at_s: 1_767_225_600,
            projects: vec![MetaProject {
                directory: "-Users-fixture-Developer-holodeck".to_owned(),
                sessions: vec![MetaSession {
                    id: "s1".to_owned(),
                    title: "read the grid scanner back".to_owned(),
                    cwd: Some("/Users/fixture/Developer/holodeck".to_owned()),
                    git_branch: Some("main".to_owned()),
                    first_activity_s: Some(1_767_225_600),
                    last_activity_s: Some(1_767_229_200),
                    records: 42,
                    messages: 8,
                    roots: 1,
                    branches: 0,
                }],
            }],
        }
    }

    #[test]
    fn a_meta_file_round_trips_through_write_and_read() {
        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("meta.json");
        let meta = sample();

        write(&path, &meta).expect("a successful write");

        assert_eq!(read(&path), Some(meta));
    }

    #[test]
    fn a_missing_meta_file_reads_as_none() {
        assert_eq!(read(Path::new("/nonexistent-rewind-meta-test/meta.json")), None);
    }

    #[test]
    fn a_corrupt_meta_file_reads_as_none_rather_than_panicking() {
        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("meta.json");
        fs::write(&path, b"not json at all").expect("a written garbage file");

        assert_eq!(read(&path), None);
    }

    #[test]
    fn a_lone_root_with_no_children_counts_one_root_and_no_branches() {
        let mut tally = ForestTally::new();
        tally.record(br#"{"type":"user","uuid":"u1","parentUuid":null,"origin":{"kind":"human"}}"#);
        assert_eq!(tally.finish(), (1, 0));
    }

    #[test]
    fn an_absent_parent_uuid_counts_as_a_root_the_same_as_an_explicit_null() {
        let mut tally = ForestTally::new();
        tally.record(br#"{"type":"user","uuid":"u1","origin":{"kind":"human"}}"#);
        assert_eq!(tally.finish(), (1, 0));
    }

    #[test]
    fn two_human_turns_sharing_a_parent_are_one_branch_point() {
        let mut tally = ForestTally::new();
        tally.record(br#"{"type":"user","uuid":"u0","parentUuid":null,"origin":{"kind":"human"}}"#);
        tally.record(br#"{"type":"user","uuid":"u1","parentUuid":"u0","origin":{"kind":"human"}}"#);
        tally.record(br#"{"type":"user","uuid":"u2","parentUuid":"u0","origin":{"kind":"human"}}"#);
        assert_eq!(tally.finish(), (1, 1));
    }

    #[test]
    fn a_tool_result_user_record_does_not_count_as_a_fork_candidate() {
        let mut tally = ForestTally::new();
        tally.record(br#"{"type":"assistant","uuid":"a0","parentUuid":null}"#);
        tally.record(br#"{"type":"user","uuid":"u1","parentUuid":"a0","toolUseResult":{"stdout":"x"}}"#);
        tally.record(br#"{"type":"user","uuid":"u2","parentUuid":"a0","toolUseResult":{"stdout":"y"}}"#);
        assert_eq!(tally.finish(), (1, 0), "tool-result echoes are not turns and must not look like a fork");
    }

    #[test]
    fn a_sidechain_record_is_never_counted() {
        let mut tally = ForestTally::new();
        tally.record(br#"{"type":"user","uuid":"u1","parentUuid":null,"isSidechain":true,"origin":{"kind":"human"}}"#);
        assert_eq!(tally.finish(), (0, 0));
    }

    #[test]
    fn a_compact_boundary_reparents_through_logical_parent_uuid_and_is_not_a_root() {
        let mut tally = ForestTally::new();
        tally.record(br#"{"type":"assistant","uuid":"a1","parentUuid":null}"#);
        tally.record(br#"{"type":"system","subtype":"compact_boundary","uuid":"s1","parentUuid":null,"logicalParentUuid":"a1"}"#);
        assert_eq!(tally.finish().0, 1, "the compact boundary must not be double-counted as a second root");
    }

    #[test]
    fn a_compact_boundary_with_no_logical_parent_falls_back_to_parent_uuid() {
        let mut tally = ForestTally::new();
        tally.record(br#"{"type":"system","subtype":"compact_boundary","uuid":"s1","parentUuid":"a1"}"#);
        assert_eq!(tally.finish(), (0, 0));
    }

    #[test]
    fn an_attachment_or_latch_record_is_never_counted() {
        let mut tally = ForestTally::new();
        tally.record(br#"{"type":"attachment","uuid":"a1","parentUuid":null}"#);
        tally.record(br#"{"type":"ai-title","aiTitle":"t","sessionId":"s1"}"#);
        assert_eq!(tally.finish(), (0, 0));
    }
}
