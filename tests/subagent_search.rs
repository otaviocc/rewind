//! A subagent transcript is now part of the corpus: a term found nowhere but a subagent
//! transcript is searchable, and the hit resolves back to the right project, session and
//! agent.

#![allow(clippy::expect_used)]

mod common;

use common::fixture_tree;
use rewind::domain::cache::shard::Kind;
use rewind::domain::cache::store;
use rewind::domain::search::{corpus, engine, query, resolve};

#[test]
fn a_term_found_only_in_a_subagent_transcript_is_indexed_and_resolves_to_it() {
    let tree = fixture_tree();
    let cache = tempfile::TempDir::new().expect("a temporary cache directory");

    let report = store::rebuild(&tree.claude_dir(), cache.path());
    assert!(report.failures.is_empty(), "{:?}", report.failures);

    let corpus = corpus::load(cache.path());
    let parsed = query::parse(r#""over a full grid""#);
    let hits = engine::search(&corpus, &parsed, 0);

    let hit = hits
        .iter()
        .find(|hit| hit.kind == Kind::Subagent)
        .expect("a phrase confined to the fixture's subagent transcripts must produce a subagent hit");

    let opened = resolve::resolve_transcript(&tree.claude_dir(), hit).expect("a resolvable subagent hit");
    assert_eq!(opened.project_directory, tree.project_dir("Developer/holodeck"));
    assert_eq!(opened.session_id, "11111111-1111-4111-8111-111111111111");
    assert!(opened.agent_id.is_some(), "a subagent hit must carry the agent id, not just the session");
}

#[test]
fn the_dangling_meta_with_no_transcript_produces_no_shard_record() {
    let tree = fixture_tree();
    let cache = tempfile::TempDir::new().expect("a temporary cache directory");

    store::rebuild(&tree.claude_dir(), cache.path());

    let corpus = corpus::load(cache.path());
    let holodeck = corpus
        .shards
        .iter()
        .find(|shard| shard.directory.as_deref() == Some(tree.project_dir("Developer/holodeck").as_str()))
        .expect("the holodeck project shard");
    let shard = rewind::domain::cache::shard::Shard::parse(&holodeck.bytes).expect("a well-formed shard");

    let dangling_name = "11111111-1111-4111-8111-111111111111/subagents/agent-c3d4e5f607182934a.jsonl";
    assert!(
        !shard.files().iter().any(|stamp| stamp.path == dangling_name),
        "a .meta.json with no transcript beside it must not become a shard file"
    );
}
