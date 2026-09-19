//! The cache store against the real fixture tree and the compiled binary: corruption
//! degrading to a rebuild, `~/.claude` staying read-only, the `--rebuild-cache` CLI mode, and
//! `ForestTally` cross-checked against `domain::thread::build`'s authoritative forest.
//!
//! Per-file append/unchanged/fresh mechanics are unit-tested in `domain::cache::build`; this
//! suite covers what only the real fixture tree and the real binary can prove.

#![allow(clippy::expect_used)]

mod common;

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use common::{fixture_tree, rewind};
use rewind::domain::cache::meta;
use rewind::domain::cache::shard::{self, Shard};
use rewind::domain::cache::store;
use rewind::domain::thread::{self, Conversation, NodeId};

fn holodeck(tree: &common::FixtureTree) -> std::path::PathBuf {
    tree.claude_dir().join("projects").join(tree.project_dir("Developer/holodeck"))
}

fn version_dir(cache_root: &Path) -> std::path::PathBuf {
    cache_root.join(format!("v{}", shard::CACHE_VERSION))
}

#[test]
fn a_cold_build_over_the_fixture_tree_produces_a_shard_per_present_project() {
    let tree = fixture_tree();
    let cache = tempfile::TempDir::new().expect("a temporary cache directory");

    let report = store::rebuild(&tree.claude_dir(), cache.path());

    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.projects_indexed, report.projects_total);
    assert!(version_dir(cache.path()).join(format!("{}.shard", tree.project_dir("Developer/holodeck"))).is_file());
    assert!(version_dir(cache.path()).join("meta.json").is_file());
}

#[test]
fn a_deleted_shard_degrades_to_a_full_rebuild_of_just_that_project() {
    let tree = fixture_tree();
    let cache = tempfile::TempDir::new().expect("a temporary cache directory");
    store::rebuild(&tree.claude_dir(), cache.path());

    let shard_path = version_dir(cache.path()).join(format!("{}.shard", tree.project_dir("Developer/holodeck")));
    fs::remove_file(&shard_path).expect("a removable shard");

    let report = store::rebuild(&tree.claude_dir(), cache.path());
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert!(report.cold_rebuilds.is_empty(), "a missing shard is a normal cold build, not a corruption to note");
    assert!(shard_path.is_file(), "the shard must be rebuilt from scratch");
    let rebuilt = fs::read(&shard_path).expect("a readable shard");
    let shard = Shard::parse(&rebuilt).expect("a well-formed rebuilt shard");
    assert!(!shard.records().is_empty());
}

#[test]
fn a_truncated_shard_degrades_to_a_full_rebuild() {
    let tree = fixture_tree();
    let cache = tempfile::TempDir::new().expect("a temporary cache directory");
    store::rebuild(&tree.claude_dir(), cache.path());

    let shard_path = version_dir(cache.path()).join(format!("{}.shard", tree.project_dir("Developer/holodeck")));
    let original = fs::read(&shard_path).expect("a readable shard");
    fs::write(&shard_path, original.get(..original.len().saturating_sub(10)).unwrap_or_default()).expect("a truncated shard");

    let report = store::rebuild(&tree.claude_dir(), cache.path());
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.cold_rebuilds, [tree.project_dir("Developer/holodeck")], "a truncated shard must be noted, not silent");
    Shard::parse(&fs::read(&shard_path).expect("a readable shard")).expect("a well-formed rebuilt shard");
}

#[test]
fn a_shard_with_a_corrupted_magic_degrades_to_a_full_rebuild() {
    let tree = fixture_tree();
    let cache = tempfile::TempDir::new().expect("a temporary cache directory");
    store::rebuild(&tree.claude_dir(), cache.path());

    let shard_path = version_dir(cache.path()).join(format!("{}.shard", tree.project_dir("Developer/holodeck")));
    let mut bytes = fs::read(&shard_path).expect("a readable shard");
    if let Some(first) = bytes.first_mut() {
        *first = b'X';
    }
    fs::write(&shard_path, &bytes).expect("a corrupted shard");

    let report = store::rebuild(&tree.claude_dir(), cache.path());
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.cold_rebuilds, [tree.project_dir("Developer/holodeck")], "a bad-magic shard must be noted, not silent");
    Shard::parse(&fs::read(&shard_path).expect("a readable shard")).expect("a well-formed rebuilt shard");
}

#[test]
fn a_rebuild_over_the_real_fixture_tree_never_writes_under_claude_dir() {
    let tree = fixture_tree();
    let cache = tempfile::TempDir::new().expect("a temporary cache directory");

    let before = snapshot(&tree.claude_dir());
    store::rebuild(&tree.claude_dir(), cache.path());
    let after = snapshot(&tree.claude_dir());

    assert_eq!(before, after, "the claude directory must be byte-for-byte unchanged");
}

fn snapshot(dir: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut entries = Vec::new();
    collect(dir, &mut entries);
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn collect(dir: &Path, out: &mut Vec<(std::path::PathBuf, Vec<u8>)>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if let Ok(bytes) = fs::read(&path) {
            out.push((path, bytes));
        }
    }
}

#[test]
fn rebuild_cache_exits_zero_and_prints_a_summary() {
    let tree = fixture_tree();
    let cache = tempfile::TempDir::new().expect("a temporary cache directory");

    let mut command = rewind();
    command.env("XDG_CACHE_HOME", cache.path());
    let output = command.arg("--claude-dir").arg(tree.claude_dir()).arg("--rebuild-cache").output().expect("rewind runs");

    assert!(output.status.success(), "rewind failed: {}", String::from_utf8_lossy(&output.stderr));
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(printed.contains("cache rebuilt"), "unexpected output:\n{printed}");
    assert!(version_dir(&cache.path().join("rewind")).join("meta.json").is_file());
}

#[test]
fn rebuild_cache_with_no_cache_writes_nothing_and_says_so() {
    let tree = fixture_tree();
    let cache = tempfile::TempDir::new().expect("a temporary cache directory");

    let mut command = rewind();
    command.env("XDG_CACHE_HOME", cache.path());
    let output = command
        .arg("--claude-dir")
        .arg(tree.claude_dir())
        .arg("--rebuild-cache")
        .arg("--no-cache")
        .output()
        .expect("rewind runs");

    assert!(output.status.success());
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(printed.contains("disabled"), "unexpected output:\n{printed}");
    assert!(!cache.path().join("rewind").exists(), "no cache directory should have been created");
}

#[test]
fn forest_tally_matches_thread_builds_root_and_fork_count_on_the_fragmentation_fixture() {
    let tree = fixture_tree();
    let path = holodeck(&tree).join("33333333-3333-4333-8333-333333333333.jsonl");

    let conversation = thread::build(&path).expect("the fixture session builds");
    let expected_roots = u32::try_from(conversation.roots().len()).unwrap_or(u32::MAX);
    let expected_branches = u32::try_from(fork_points(&conversation)).unwrap_or(u32::MAX);

    let bytes = fs::read(&path).expect("a readable fixture file");
    let mut tally = meta::ForestTally::new();
    for line in bytes.split(|&byte| byte == b'\n') {
        if !line.is_empty() {
            tally.record(line);
        }
    }
    let (roots, branches) = tally.finish();

    assert_eq!(roots, expected_roots, "root count must match domain::thread::build on a well-formed fixture");
    assert_eq!(branches, expected_branches, "the fixture's known three-way fork must be counted as exactly one branch point");
    assert!(expected_branches > 0, "sanity: this fixture is documented to contain a fork");
}

#[test]
fn assembling_the_build_from_its_pieces_matches_the_serial_rebuild_byte_for_byte() {
    use rewind::domain::cache::build;
    use rewind::domain::project;

    let tree = fixture_tree();
    let serial_cache = tempfile::TempDir::new().expect("a temporary cache directory");
    let serial_report = store::rebuild(&tree.claude_dir(), serial_cache.path());
    assert!(serial_report.failures.is_empty(), "{:?}", serial_report.failures);

    let assembled_cache = tempfile::TempDir::new().expect("a temporary cache directory");
    let plan = store::prepare(&tree.claude_dir(), assembled_cache.path()).expect("a preparable cache root");
    let projects = project::discover(&tree.claude_dir()).expect("discoverable fixture projects");
    let mut outcomes: Vec<store::Outcome> =
        projects.iter().map(|project| store::build_project(&plan, project, &mut build::Control::inert())).collect();
    if let Some(history) = store::build_history(&plan, &mut build::Control::inert()) {
        outcomes.push(history);
    }
    let assembled_report = store::finish(&plan, projects.len(), outcomes, serial_report.wall);
    assert!(assembled_report.failures.is_empty(), "{:?}", assembled_report.failures);
    assert_eq!(assembled_report.projects_indexed, serial_report.projects_indexed);

    let serial_version_dir = version_dir(serial_cache.path());
    let assembled_version_dir = version_dir(assembled_cache.path());
    let mut names: Vec<String> = fs::read_dir(&serial_version_dir)
        .expect("a readable serial version directory")
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| Path::new(name).extension().is_some_and(|extension| extension.eq_ignore_ascii_case("shard")))
        .collect();
    names.sort();
    assert!(!names.is_empty(), "the fixture tree must produce at least one shard to compare");

    for name in names {
        let serial_bytes = fs::read(serial_version_dir.join(&name)).expect("a readable serial shard");
        let assembled_bytes = fs::read(assembled_version_dir.join(&name)).expect("a readable assembled shard");
        let serial_shard = Shard::parse(&serial_bytes).expect("a well-formed serial shard");
        let assembled_shard = Shard::parse(&assembled_bytes).expect("a well-formed assembled shard");

        assert_eq!(serial_shard.files(), assembled_shard.files(), "{name}: file stamps must match");
        assert_eq!(serial_shard.records().len(), assembled_shard.records().len(), "{name}: record counts must match");
        let serial_texts: Vec<&[u8]> = serial_shard.records().iter().filter_map(|record| serial_shard.text(record)).collect();
        let assembled_texts: Vec<&[u8]> =
            assembled_shard.records().iter().filter_map(|record| assembled_shard.text(record)).collect();
        assert_eq!(serial_texts, assembled_texts, "{name}: extracted text must be identical between the two builds");
    }
}

fn fork_points(conversation: &Conversation) -> usize {
    let mut count: usize = 0;
    let mut stack: Vec<NodeId> = conversation.roots().to_vec();
    let mut seen: HashSet<NodeId> = HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        if !conversation.alternates(id).is_empty() {
            count = count.saturating_add(1);
        }
        if let Some(node) = conversation.node(id) {
            stack.extend(node.children.iter().copied());
        }
    }
    count
}
