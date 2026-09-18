//! Conversation forest assembly against the fixture tree: the exit criteria in #5.

#![allow(clippy::expect_used)]

mod common;

use std::path::PathBuf;

use common::fixtures;
use rewind::domain::diagnostics::Defect;
use rewind::domain::thread::{self, Divider, NodeKind};

const HOLODECK: &str = "-Users-fixture-Developer-holodeck";
const COMPACTION: &str = "22222222-2222-4222-8222-222222222222.jsonl";
const FRAGMENTED: &str = "33333333-3333-4333-8333-333333333333.jsonl";
const CYCLIC: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa.jsonl";

fn fixture(name: &str) -> PathBuf {
    fixtures().join("projects").join(HOLODECK).join(name)
}

#[test]
fn a_compacted_session_stays_one_connected_thread_with_a_compacted_divider() {
    let conversation = thread::build(&fixture(COMPACTION)).expect("a conversation");

    assert_eq!(conversation.roots().len(), 2, "session start and /clear, not three");

    let dividers: Vec<Option<Divider>> =
        conversation.roots().iter().filter_map(|&id| conversation.node(id)).map(|node| node.divider).collect();
    assert_eq!(dividers, [Some(Divider::SessionStart), Some(Divider::Clear)]);

    let boundary = conversation.id_of("a2222222-0000-4000-8000-000000000005").expect("the compaction boundary node");
    let boundary_node = conversation.node(boundary).expect("the boundary");
    assert_eq!(boundary_node.divider, Some(Divider::Compacted));
    assert_eq!(
        boundary_node.parent,
        conversation.id_of("a2222222-0000-4000-8000-000000000004"),
        "the boundary's effective parent is logicalParentUuid, not the null parentUuid on the record"
    );

    let uuids = [
        "a2222222-0000-4000-8000-000000000001",
        "a2222222-0000-4000-8000-000000000002",
        "a2222222-0000-4000-8000-000000000003",
        "a2222222-0000-4000-8000-000000000004",
        "a2222222-0000-4000-8000-000000000005",
        "a2222222-0000-4000-8000-000000000051",
        "a2222222-0000-4000-8000-000000000006",
        "a2222222-0000-4000-8000-000000000007",
        "a2222222-0000-4000-8000-000000000008",
    ];
    let first_root_path: Vec<_> = uuids.iter().filter_map(|uuid| conversation.id_of(uuid)).collect();
    assert_eq!(first_root_path.len(), uuids.len(), "every node of the first root must resolve");
    let thread_prefix = &conversation.thread()[..uuids.len()];
    assert_eq!(thread_prefix, first_root_path.as_slice(), "root 1's own thread must not split at the compaction boundary");

    let summary = conversation.id_of("a2222222-0000-4000-8000-000000000051").expect("the compact summary");
    let summary_node = conversation.node(summary).expect("the summary");
    let NodeKind::User(record) = &summary_node.kind else { panic!("not a user record") };
    assert!(record.is_compact_summary, "the record after a boundary is the summary the compaction wrote");
    assert!(record.is_human_turn(), "and it passes is_human_turn, which is why the render guard is separate");
}

#[test]
fn the_three_fragment_message_coalesces_and_the_retry_stays_a_sibling() {
    let conversation = thread::build(&fixture(FRAGMENTED)).expect("a conversation");

    let coalesced = conversation.id_of("a3333333-0000-4000-8000-000000000002").expect("fragment 1's uuid");
    assert_eq!(
        conversation.id_of("a3333333-0000-4000-8000-000000000003"),
        Some(coalesced),
        "fragment 2's uuid must resolve to the same coalesced node"
    );
    assert_eq!(
        conversation.id_of("a3333333-0000-4000-8000-000000000004"),
        Some(coalesced),
        "fragment 3's uuid must resolve to the same coalesced node"
    );

    let node = conversation.node(coalesced).expect("the coalesced node");
    let NodeKind::Assistant(turn) = &node.kind else { panic!("expected an assistant node") };
    assert_eq!(turn.fragments, 3);
    assert_eq!(
        turn.usage.as_ref().map(|usage| usage.output_tokens),
        Some(245),
        "usage must be fragment 3's own (apiBlockIndex 2), the highest, never summed (1+2+3 = 563)"
    );

    let attaches_off_fragment_three =
        conversation.id_of("a3333333-0000-4000-8000-000000000005").expect("the record parenting off fragment 3");
    assert_eq!(conversation.node(attaches_off_fragment_three).and_then(|n| n.parent), Some(coalesced));

    let retry = conversation.id_of("a3333333-0000-4000-8000-000000000006").expect("the retry");
    let user_root_child = conversation.node(retry).and_then(|n| n.parent);
    assert_eq!(
        user_root_child,
        conversation.id_of("a3333333-0000-4000-8000-000000000001"),
        "the retry parents off the human turn directly"
    );
    assert_ne!(retry, coalesced, "the retry must not merge into the original fragment group");

    let branch_a = conversation.id_of("a3333333-0000-4000-8000-000000000007").expect("the unreached fork sibling");
    let branch_b = conversation.id_of("a3333333-0000-4000-8000-000000000008").expect("the leafUuid fork sibling");
    assert!(conversation.thread().contains(&branch_b), "leafUuid points through 0008");
    assert!(!conversation.thread().contains(&branch_a), "0007 is reachable but not on the default thread");
}

#[test]
fn the_cyclic_fixture_terminates_with_exactly_one_severed_cycle_defect() {
    let conversation = thread::build(&fixture(CYCLIC)).expect("a conversation");

    assert_eq!(conversation.diagnostics().count(), 1);
    assert!(matches!(conversation.diagnostics().defects().first(), Some(Defect::SeveredCycle { .. })));

    assert_eq!(conversation.roots().len(), 1, "one of the two mutually-parented records is promoted to a root");
    let root = conversation.roots().first().copied().expect("a root");
    assert_eq!(conversation.node(root).and_then(|node| node.divider), Some(Divider::Detached));
}
