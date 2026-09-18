//! Subagent discovery against the fixture tree: the four ways an agent can be reached, and the
//! shapes that must not become errors.

#![allow(clippy::expect_used)]

mod common;

use std::path::PathBuf;

use common::fixtures;
use rewind::domain::subagent::{self, Agents};
use rewind::domain::thread;

const HOLODECK: &str = "-Users-fixture-Developer-holodeck";
const BASELINE: &str = "11111111-1111-4111-8111-111111111111";
const MARKDOWN: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

const JOINED: &str = "a1b2c3d4e5f607182";
const FORKED: &str = "b2c3d4e5f60718293";
const DANGLING: &str = "c3d4e5f607182934a";
const BY_RESULT: &str = "d4e5f60718293a4b5";
const NESTED: &str = "e5f60718293a4b5c6";

fn session_path(session: &str) -> PathBuf {
    fixtures().join("projects").join(HOLODECK).join(format!("{session}.jsonl"))
}

fn baseline() -> Agents {
    subagent::discover(&session_path(BASELINE))
}

#[test]
fn every_meta_in_the_directory_is_discovered_including_the_one_with_no_transcript() {
    let agents = baseline();
    let mut ids: Vec<&str> = agents.all().iter().map(|agent| &*agent.id).collect();
    ids.sort_unstable();
    assert_eq!(ids, [JOINED, FORKED, DANGLING, BY_RESULT, NESTED]);
}

#[test]
fn a_session_with_no_subagents_directory_discovers_none_rather_than_failing() {
    let agents = subagent::discover(&session_path(MARKDOWN));
    assert!(agents.is_empty(), "the markdown session spawned nothing");
}

#[test]
fn the_tool_use_id_reaches_the_agent_it_names() {
    let agents = baseline();
    let found = agents.spawned_by("toolu_01FixtureAgentExplore000", None).expect("the joined agent");
    assert_eq!(&*found.id, JOINED);
    assert_eq!(&*found.kind, "Explore");
    assert_eq!(found.description.as_deref(), Some("Trace the grid scanner"));
    assert_eq!(found.depth, 1);
    assert!(found.enterable());
}

#[test]
fn a_meta_with_no_tool_use_id_is_reached_only_by_the_results_agent_id() {
    let agents = baseline();
    assert_eq!(agents.spawned_by("toolu_01FixtureAgentFork0000", None), None, "nothing in the meta points back");
    let found = agents.spawned_by("toolu_01FixtureAgentFork0000", Some(BY_RESULT)).expect("reached by the result");
    assert_eq!(&*found.id, BY_RESULT);
    assert!(found.is_forked_skill(), "the forked-skill sidecar is read");
    assert_eq!(found.label(), "code-review", "a forked skill is named by its skill, not its agentType");
}

#[test]
fn a_meta_whose_agent_was_killed_before_it_wrote_anything_is_not_enterable() {
    let agents = baseline();
    let found = agents.by_id(DANGLING).expect("the dangling meta");
    assert!(found.stopped_by_user);
    assert!(!found.enterable(), "there is no transcript beside it");
    assert_eq!(found.description.as_deref(), Some("Killed before it wrote a transcript"), "it still renders its metadata");
}

#[test]
fn a_depth_two_agent_is_a_child_of_its_parent_agent_and_not_of_the_session() {
    let agents = baseline();
    let nested = agents.by_id(NESTED).expect("the nested agent");
    assert_eq!(nested.depth, 2);
    assert_eq!(nested.parent.as_deref(), Some(JOINED));

    assert!(!agents.children_of(None).any(|agent| &*agent.id == NESTED), "a depth-2 agent must not hang off the session");
    let children: Vec<&str> = agents.children_of(Some(JOINED)).map(|agent| &*agent.id).collect();
    assert_eq!(children, [NESTED]);
}

#[test]
fn a_nested_agents_tool_use_id_resolves_inside_its_parents_transcript_and_nowhere_else() {
    let agents = baseline();
    let nested = agents.by_id(NESTED).expect("the nested agent");
    let tool_use_id = nested.tool_use_id.as_deref().expect("a toolUseId");

    let session = thread::build(&session_path(BASELINE)).expect("the session");
    assert!(session.result_of(tool_use_id).is_none(), "the session has no such call");

    let parent = subagent::transcript_path(&session_path(BASELINE), JOINED);
    let inside = thread::build(&parent).expect("the parent agent");
    assert!(inside.result_of(tool_use_id).is_some(), "the parent agent is where the call lives");
}

#[test]
fn every_subagent_transcript_builds_as_its_own_conversation_with_no_defects() {
    let agents = baseline();
    for agent in agents.all() {
        let Some(path) = agent.transcript.as_deref() else { continue };
        let conversation = thread::build(path).expect("a built conversation");
        assert_eq!(conversation.diagnostics().count(), 0, "{} reported defects", agent.id);
        assert_eq!(conversation.roots().len(), 1, "{} is not one thread", agent.id);
        assert!(!conversation.thread().is_empty(), "{} rendered nothing", agent.id);
    }
}

#[test]
fn a_forked_skills_transcript_names_the_point_it_forked_from() {
    let path = subagent::transcript_path(&session_path(BASELINE), FORKED);
    let conversation = thread::build(&path).expect("the forked skill");
    let fork = conversation.state().fork_context.as_ref().expect("a fork-context-ref");
    assert_eq!(fork.agent_id.as_deref(), Some(FORKED));
    assert_eq!(fork.parent_session_id.as_deref(), Some(BASELINE));
    assert_eq!(fork.parent_last_uuid.as_deref(), Some("a1111111-0000-4000-8000-000000000012"));
    assert_eq!(fork.context_length, Some(757));
}

#[test]
fn a_subagents_assistant_records_carry_the_attribution_that_labels_them() {
    let path = subagent::transcript_path(&session_path(BASELINE), FORKED);
    let conversation = thread::build(&path).expect("the forked skill");
    let attribution: Vec<(Option<&str>, Option<&str>)> = conversation
        .thread()
        .iter()
        .filter_map(|id| conversation.node(*id))
        .filter_map(|node| match &node.kind {
            rewind::domain::thread::NodeKind::Assistant(turn) => {
                Some((turn.attribution_agent.as_deref(), turn.attribution_skill.as_deref()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(attribution, [(None, Some("code-review"))], "a forked skill labels itself by skill, not by agent");
}

fn visible(session: &std::path::Path, agents: &Agents) -> std::collections::BTreeSet<String> {
    let Ok(conversation) = thread::build(session) else { return std::collections::BTreeSet::new() };
    let expanded = rewind::render::Expanded::new();
    let outputs = rewind::render::Outputs::new();
    let ctx = rewind::render::Ctx { width: 100, expanded: &expanded, outputs: &outputs, agents };
    rewind::render::message::transcript(&conversation, &ctx)
        .anchors
        .iter()
        .filter_map(|anchor| anchor.agent.as_deref().map(str::to_owned))
        .collect()
}

#[test]
fn no_agent_the_session_can_reach_is_left_out_of_the_rendered_transcript() {
    let session = session_path(BASELINE);
    let agents = baseline();
    let visible = visible(&session, &agents);
    let expected: std::collections::BTreeSet<String> = agents
        .all()
        .iter()
        .filter(|agent| agent.enterable() && agent.parent.is_none())
        .map(|agent| agent.id.to_string())
        .collect();
    assert_eq!(visible, expected, "every top-level agent with a transcript is either on a call or in the tail");
    assert!(!visible.contains(NESTED), "a nested agent is reached by drilling into its parent, not from here");
    assert!(!visible.contains(DANGLING), "an agent with no transcript is rendered but not enterable");
}

#[test]
#[ignore = "reads the developer's real ~/.claude, not the fixture tree"]
fn no_agent_in_the_real_store_is_left_out_of_the_transcript_that_could_reach_it() {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return };
    let projects = home.join(".claude").join("projects");
    if !projects.is_dir() {
        return;
    }

    let mut sessions = Vec::new();
    collect(&projects, &mut sessions);

    let (mut total, mut on_a_call, mut nested, mut no_transcript) = (0_usize, 0_usize, 0_usize, 0_usize);
    let mut invisible: Vec<String> = Vec::new();
    let mut defects: Vec<String> = Vec::new();
    for session in sessions {
        let agents = subagent::discover(&session);
        if agents.is_empty() {
            continue;
        }
        let Ok(conversation) = thread::build(&session) else { continue };
        let calls = agent_calls(&conversation);
        let visible = visible(&session, &agents);
        for agent in agents.all() {
            total = total.saturating_add(1);
            if agent.parent.is_some() {
                nested = nested.saturating_add(1);
                continue;
            }
            if !agent.enterable() {
                no_transcript = no_transcript.saturating_add(1);
                continue;
            }
            if calls.iter().any(|(id, result)| agents.spawned_by(id, result.as_deref()).is_some_and(|f| f.id == agent.id)) {
                on_a_call = on_a_call.saturating_add(1);
            }
            if !visible.contains(&*agent.id) {
                invisible.push(format!("{} ({})", agent.id, agent.kind));
            }
            if let Some(path) = agent.transcript.as_deref() {
                match thread::build(path) {
                    Ok(built) if built.diagnostics().count() > 0 => {
                        defects.push(format!("{}: {} defects", agent.id, built.diagnostics().count()));
                    }
                    Ok(_) => {}
                    Err(error) => defects.push(format!("{}: {error}", agent.id)),
                }
            }
        }
    }

    println!("{total} agents: {on_a_call} on a call in the default thread, {nested} nested, {no_transcript} with no transcript");
    assert!(total > 0, "no subagent was discovered in the real store at all");
    assert!(invisible.is_empty(), "these agents have a transcript and appear nowhere in the rendered session: {invisible:#?}");
    assert!(defects.is_empty(), "subagent transcripts that did not build cleanly: {defects:#?}");
}

fn agent_calls(conversation: &rewind::domain::thread::Conversation) -> Vec<(String, Option<String>)> {
    let mut found = Vec::new();
    for id in conversation.thread() {
        let Some(node) = conversation.node(*id) else { continue };
        let rewind::domain::thread::NodeKind::Assistant(turn) = &node.kind else { continue };
        for block in &turn.content {
            let rewind::domain::block::Block::ToolUse { id, name, .. } = block else { continue };
            if name != "Agent" && name != "Task" {
                continue;
            }
            let result = conversation
                .result_of(id)
                .and_then(|node| rewind::domain::tool::Outcome::of(node, id))
                .and_then(|outcome| outcome.detail)
                .and_then(|detail| detail.get("agentId"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            found.push((id.clone(), result));
        }
    }
    found
}

fn collect(dir: &std::path::Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "jsonl") {
            found.push(path);
        }
    }
}
