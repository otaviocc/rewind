//! Live badges against the fixture tree: a rewritten pid genuinely alive, a stale one dead.

#![allow(clippy::expect_used)]

mod common;

use common::{LIVE_SESSION_ID, fixture_tree};
use rewind::domain::live::scan;

#[test]
fn the_rewritten_pid_is_alive_and_the_stale_one_is_not() {
    let tree = fixture_tree();
    let live = scan(&tree.claude_dir());

    let ours = live.iter().find(|entry| entry.session_id == LIVE_SESSION_ID);
    assert!(ours.is_some(), "the rewritten pid should be reliably alive: {live:?}");

    let stale = live.iter().find(|entry| entry.session_id == "33333333-3333-4333-8333-333333333333");
    assert!(stale.is_none(), "a dead pid must not carry a live badge: {live:?}");
}

#[test]
fn the_opaque_key_blob_beside_the_live_pid_is_not_mistaken_for_a_second_session() {
    let tree = fixture_tree();
    let live = scan(&tree.claude_dir());
    assert_eq!(live.len(), 1, "{live:?}");
}
