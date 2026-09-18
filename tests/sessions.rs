//! Session discovery against the fixture tree: titles, counts, branches and chaining.

#![allow(clippy::expect_used)]

mod common;

use common::fixture_tree;
use rewind::domain::session::{TitleSource, discover};

#[test]
fn the_custom_title_record_wins_even_though_an_ai_title_follows_it() {
    let tree = fixture_tree();
    let holodeck = tree.claude_dir().join("projects").join(tree.project_dir("Developer/holodeck"));
    let sessions = discover(&holodeck);
    let session = sessions
        .iter()
        .find(|session| session.id == "11111111-1111-4111-8111-111111111111")
        .expect("the baseline session was discovered");

    assert_eq!(session.title, "custom-title record wins");
    assert_eq!(session.title_source, TitleSource::CustomTitle);
    assert_eq!(session.git_branch.as_deref(), Some("main"));
    assert_eq!(session.slug.as_deref(), Some("read-the-grid-scanner-back-to-me-velvet-pumpkin"));
    assert_eq!(session.records, 39);
    assert_eq!(session.messages, 8, "1 human turn and 7 coalesced assistant messages");
    assert!(session.first_activity.is_some());
    assert!(session.last_activity.is_some());
    assert!(session.first_activity <= session.last_activity);
}

#[test]
fn fragmentation_retry_and_fork_all_coalesce_correctly() {
    let tree = fixture_tree();
    let holodeck = tree.claude_dir().join("projects").join(tree.project_dir("Developer/holodeck"));
    let sessions = discover(&holodeck);
    let session = sessions
        .iter()
        .find(|session| session.id == "33333333-3333-4333-8333-333333333333")
        .expect("the fragmentation session was discovered");

    assert_eq!(session.records, 14);
    assert_eq!(
        session.messages, 8,
        "4 human turns (including the fork) and 4 coalesced assistant messages (the retry is a distinct message)"
    );
}

#[test]
fn a_session_with_no_title_latch_at_all_falls_back_to_the_first_human_message() {
    let tree = fixture_tree();
    let jeffries = tree.claude_dir().join("projects").join(tree.project_dir("Developer/jeffries-tube"));
    let sessions = discover(&jeffries);
    let session = sessions.first().expect("the jeffries-tube session was discovered");

    assert_eq!(session.title, "is this project even listed");
    assert_eq!(session.title_source, TitleSource::FirstMessage);
}

#[test]
fn an_ai_title_is_the_source_for_a_minimal_single_exchange_session() {
    let tree = fixture_tree();
    let nomad = tree.claude_dir().join("projects").join(tree.project_dir("Developer/nomad"));
    let sessions = discover(&nomad);
    let session = sessions.first().expect("the nomad session was discovered");

    assert_eq!(session.title, "Abandoned probe");
    assert_eq!(session.title_source, TitleSource::AiTitle);
}

#[test]
fn continued_in_is_followed_to_a_successor_session_id() {
    let tree = fixture_tree();
    let warp_core = tree.claude_dir().join("projects").join(tree.project_dir("Developer/warp-core"));
    let sessions = discover(&warp_core);
    let session = sessions.first().expect("the warp-core session was discovered");

    assert_eq!(session.continued_in.as_deref(), Some("55550000-0000-4000-8000-000000000000"));
}

#[test]
fn a_project_with_no_transcripts_discovers_no_sessions() {
    let tree = fixture_tree();
    let shuttlebay = tree.claude_dir().join("projects").join(tree.project_dir("Developer/shuttlebay"));
    assert!(discover(&shuttlebay).is_empty());
}
