//! The agents a session spawned: their metadata sidecars, and the two keys that join one back to
//! the call that spawned it.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const SUBAGENT_DIR: &str = "subagents";
const PREFIX: &str = "agent-";
const META_SUFFIX: &str = ".meta.json";
const TRANSCRIPT_SUFFIX: &str = ".jsonl";
const FORKED_SKILL_SUFFIX: &str = ".forked-skill.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub id: Box<str>,
    pub kind: Box<str>,
    pub description: Option<Box<str>>,
    pub name: Option<Box<str>>,
    pub depth: u32,
    pub tool_use_id: Option<Box<str>>,
    pub parent: Option<Box<str>>,
    pub skill: Option<Box<str>>,
    pub stopped_by_user: bool,
    pub transcript: Option<PathBuf>,
}

impl Agent {
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.kind)
    }

    pub const fn is_forked_skill(&self) -> bool {
        self.skill.is_some()
    }

    pub const fn enterable(&self) -> bool {
        self.transcript.is_some()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Agents {
    spawned: Vec<Agent>,
    by_id: HashMap<Box<str>, usize>,
    by_tool_use_id: HashMap<Box<str>, usize>,
}

impl Agents {
    pub fn all(&self) -> &[Agent] {
        &self.spawned
    }

    pub const fn is_empty(&self) -> bool {
        self.spawned.is_empty()
    }

    pub fn by_id(&self, id: &str) -> Option<&Agent> {
        self.spawned.get(*self.by_id.get(id)?)
    }

    pub fn by_tool_use_id(&self, tool_use_id: &str) -> Option<&Agent> {
        self.spawned.get(*self.by_tool_use_id.get(tool_use_id)?)
    }

    pub fn spawned_by(&self, tool_use_id: &str, result_agent_id: Option<&str>) -> Option<&Agent> {
        self.by_tool_use_id(tool_use_id).or_else(|| self.by_id(result_agent_id?))
    }

    pub fn children_of(&self, parent: Option<&str>) -> impl Iterator<Item = &Agent> {
        self.spawned.iter().filter(move |agent| agent.parent.as_deref() == parent)
    }

    fn push(&mut self, agent: Agent) {
        let index = self.spawned.len();
        self.by_id.insert(agent.id.clone(), index);
        if let Some(tool_use_id) = agent.tool_use_id.clone() {
            self.by_tool_use_id.insert(tool_use_id, index);
        }
        self.spawned.push(agent);
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    #[serde(default)]
    agent_type: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    spawn_depth: Option<u32>,
    #[serde(default)]
    tool_use_id: Option<String>,
    #[serde(default)]
    parent_agent_id: Option<String>,
    #[serde(default)]
    stopped_by_user: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct ForkedSkill {
    #[serde(rename = "skillName", default)]
    skill_name: Option<String>,
}

pub fn subagent_dir(transcript: &Path) -> PathBuf {
    transcript.with_extension("").join(SUBAGENT_DIR)
}

pub fn transcript_path(transcript: &Path, id: &str) -> PathBuf {
    subagent_dir(transcript).join(format!("{PREFIX}{id}{TRANSCRIPT_SUFFIX}"))
}

pub fn discover(transcript: &Path) -> Agents {
    let dir = subagent_dir(transcript);
    let Ok(entries) = fs::read_dir(&dir) else { return Agents::default() };

    let mut ids: Vec<Box<str>> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stem = name.strip_suffix(META_SUFFIX)?;
            Some(Box::from(stem.strip_prefix(PREFIX).unwrap_or(stem)))
        })
        .collect();
    ids.sort_unstable();

    let mut agents = Agents::default();
    for id in ids {
        if let Some(agent) = read_one(&dir, &id) {
            agents.push(agent);
        }
    }
    agents
}

fn read_one(dir: &Path, id: &str) -> Option<Agent> {
    let meta: Meta = serde_json::from_str(&fs::read_to_string(dir.join(format!("{PREFIX}{id}{META_SUFFIX}"))).ok()?).ok()?;
    let transcript = dir.join(format!("{PREFIX}{id}{TRANSCRIPT_SUFFIX}"));
    let skill = fs::read_to_string(dir.join(format!("{PREFIX}{id}{FORKED_SKILL_SUFFIX}")))
        .ok()
        .and_then(|text| serde_json::from_str::<ForkedSkill>(&text).ok())
        .and_then(|forked| forked.skill_name)
        .map(Box::from);

    Some(Agent {
        id: Box::from(id),
        kind: meta.agent_type.map_or_else(|| Box::from("agent"), Box::from),
        description: meta.description.map(Box::from),
        name: meta.name.map(Box::from),
        depth: meta.spawn_depth.unwrap_or(1),
        tool_use_id: meta.tool_use_id.map(Box::from),
        parent: meta.parent_agent_id.map(Box::from),
        skill,
        stopped_by_user: meta.stopped_by_user,
        transcript: transcript.is_file().then_some(transcript),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, tool_use_id: Option<&str>, parent: Option<&str>) -> Agent {
        Agent {
            id: Box::from(id),
            kind: Box::from("Explore"),
            description: None,
            name: None,
            depth: if parent.is_some() { 2 } else { 1 },
            tool_use_id: tool_use_id.map(Box::from),
            parent: parent.map(Box::from),
            skill: None,
            stopped_by_user: false,
            transcript: None,
        }
    }

    #[test]
    fn a_subagent_lives_beside_the_transcript_and_not_where_a_record_says() {
        let transcript = Path::new("/store/projects/-encoded/11111111.jsonl");
        assert_eq!(subagent_dir(transcript), PathBuf::from("/store/projects/-encoded/11111111/subagents"));
        assert_eq!(
            transcript_path(transcript, "a1b2c3d4e5f607182"),
            PathBuf::from("/store/projects/-encoded/11111111/subagents/agent-a1b2c3d4e5f607182.jsonl")
        );
    }

    #[test]
    fn a_session_with_no_subagents_directory_discovers_none_rather_than_failing() {
        let agents = discover(Path::new("/nonexistent-rewind-test/11111111.jsonl"));
        assert!(agents.is_empty());
        assert_eq!(agents.all().len(), 0);
        assert_eq!(agents.by_id("a1b2c3d4e5f607182"), None);
    }

    #[test]
    fn a_call_resolves_by_its_tool_use_id_first_and_by_the_results_agent_id_second() {
        let mut agents = Agents::default();
        agents.push(agent("aaa11111111111111", Some("toolu_01A"), None));
        agents.push(agent("bbb22222222222222", None, None));

        assert_eq!(agents.spawned_by("toolu_01A", None).map(|found| &*found.id), Some("aaa11111111111111"));
        assert_eq!(
            agents.spawned_by("toolu_unknown", Some("bbb22222222222222")).map(|found| &*found.id),
            Some("bbb22222222222222"),
            "the meta has no toolUseId, so only the result's agentId reaches it"
        );
        assert_eq!(agents.spawned_by("toolu_unknown", None), None);
    }

    #[test]
    fn the_tool_use_id_wins_when_the_two_keys_disagree() {
        let mut agents = Agents::default();
        agents.push(agent("aaa11111111111111", Some("toolu_01A"), None));
        agents.push(agent("bbb22222222222222", Some("toolu_01B"), None));
        assert_eq!(
            agents.spawned_by("toolu_01A", Some("bbb22222222222222")).map(|found| &*found.id),
            Some("aaa11111111111111"),
            "they agree in every observed case, so the order only has to be stated, not clever"
        );
    }

    #[test]
    fn nesting_is_read_from_the_parent_agent_id_and_never_from_the_session() {
        let mut agents = Agents::default();
        agents.push(agent("aaa11111111111111", Some("toolu_01A"), None));
        agents.push(agent("bbb22222222222222", Some("toolu_01B"), Some("aaa11111111111111")));

        let top: Vec<&str> = agents.children_of(None).map(|found| &*found.id).collect();
        assert_eq!(top, ["aaa11111111111111"], "a depth-2 agent is not a child of the session");
        let nested: Vec<&str> = agents.children_of(Some("aaa11111111111111")).map(|found| &*found.id).collect();
        assert_eq!(nested, ["bbb22222222222222"]);
    }

    #[test]
    fn a_forked_skill_is_labelled_by_its_name_and_everything_else_by_its_type() {
        let mut forked = agent("ccc33333333333333", None, None);
        forked.name = Some(Box::from("code-review"));
        forked.skill = Some(Box::from("code-review"));
        assert_eq!(forked.label(), "code-review");
        assert!(forked.is_forked_skill());

        let plain = agent("ddd44444444444444", None, None);
        assert_eq!(plain.label(), "Explore");
        assert!(!plain.is_forked_skill());
    }

    #[test]
    fn an_agent_with_no_transcript_beside_its_meta_is_not_enterable() {
        let mut agent = agent("eee55555555555555", None, None);
        assert!(!agent.enterable(), "a meta whose agent was killed before it wrote anything");
        agent.transcript = Some(PathBuf::from("/store/x/subagents/agent-eee55555555555555.jsonl"));
        assert!(agent.enterable());
    }
}
