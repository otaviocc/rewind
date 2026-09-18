//! A session is a forest, not a list: fold the latch tail, coalesce assistant fragments,
//! resolve parents, sever cycles, and pick the default branch through every root.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use thiserror::Error;

use crate::domain::block::{Block, Content, Usage};
use crate::domain::diagnostics::{Defect, Diagnostics};
use crate::domain::latch::{CostStateLatch, ForkContextRefLatch, Latch};
use crate::domain::lines::Lines;
use crate::domain::record::{self, AssistantRecord, AttachmentRecord, Envelope, ParseError, Record, SystemRecord, UserRecord};

#[derive(Debug, Error)]
pub enum ThreadError {
    #[error("cannot read {0}")]
    Unreadable(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(u32);

impl NodeId {
    fn index(self) -> usize {
        usize::try_from(self.0).unwrap_or(usize::MAX)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Divider {
    SessionStart,
    Clear,
    Compacted,
    Detached,
}

#[derive(Debug, Clone)]
pub enum NodeKind {
    User(UserRecord),
    Assistant(AssistantTurn),
    System(SystemRecord),
    Attachment(AttachmentRecord),
}

#[derive(Debug, Clone)]
pub struct AssistantTurn {
    pub envelope: Envelope,
    pub model: Option<String>,
    pub content: Vec<Block>,
    pub usage: Option<Usage>,
    pub stop_reason: Option<String>,
    pub fragments: u32,
    pub attribution_agent: Option<String>,
    pub attribution_skill: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub timestamp: Option<Timestamp>,
    pub divider: Option<Divider>,
    pub kind: NodeKind,
}

impl Node {
    pub const fn is_sidechain(&self) -> bool {
        match &self.kind {
            NodeKind::User(record) => record.envelope.is_sidechain,
            NodeKind::Assistant(turn) => turn.envelope.is_sidechain,
            NodeKind::System(record) => record.envelope.is_sidechain,
            NodeKind::Attachment(record) => record.envelope.is_sidechain,
        }
    }

    pub fn uuid(&self) -> &str {
        match &self.kind {
            NodeKind::User(record) => &record.envelope.uuid,
            NodeKind::Assistant(turn) => &turn.envelope.uuid,
            NodeKind::System(record) => &record.envelope.uuid,
            NodeKind::Attachment(record) => &record.envelope.uuid,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SessionState {
    pub custom_title: Option<String>,
    pub ai_title: Option<String>,
    pub agent_name: Option<String>,
    pub last_prompt: Option<String>,
    pub leaf_uuid: Option<String>,
    pub cost_state: Option<CostStateLatch>,
    pub continued_in: Option<String>,
    pub summary: Option<String>,
    pub fork_context: Option<ForkContextRefLatch>,
}

#[derive(Debug, Clone)]
pub struct Conversation {
    nodes: Vec<Node>,
    ids: HashMap<Box<str>, NodeId>,
    results: HashMap<Box<str>, NodeId>,
    inline: HashMap<Box<str>, NodeId>,
    roots: Vec<NodeId>,
    thread: Vec<NodeId>,
    chosen: HashMap<NodeId, NodeId>,
    state: SessionState,
    diagnostics: Diagnostics,
}

impl Conversation {
    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id.index())
    }

    pub fn roots(&self) -> &[NodeId] {
        &self.roots
    }

    pub fn thread(&self) -> &[NodeId] {
        &self.thread
    }

    pub fn chosen(&self, parent: NodeId) -> Option<NodeId> {
        self.chosen.get(&parent).copied()
    }

    pub const fn state(&self) -> &SessionState {
        &self.state
    }

    pub const fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }

    pub fn id_of(&self, uuid: &str) -> Option<NodeId> {
        self.ids.get(uuid).copied()
    }

    pub fn result_of(&self, tool_use_id: &str) -> Option<&Node> {
        self.node(*self.results.get(tool_use_id)?)
    }

    pub fn inline_agent(&self, tool_use_id: &str) -> Option<NodeId> {
        self.inline.get(tool_use_id).copied()
    }

    pub fn inline_agents(&self) -> impl Iterator<Item = (&str, NodeId)> {
        self.inline.iter().map(|(id, node)| (&**id, *node))
    }

    pub fn is_sidechain(&self) -> bool {
        self.roots.first().and_then(|root| self.node(*root)).is_some_and(Node::is_sidechain)
    }

    pub fn is_turn(&self, id: NodeId) -> bool {
        self.node(id).is_some_and(|node| match &node.kind {
            NodeKind::Assistant(_) => true,
            NodeKind::User(record) => record.is_human_turn() || record.is_compact_summary,
            NodeKind::System(_) | NodeKind::Attachment(_) => false,
        })
    }

    pub fn alternates(&self, id: NodeId) -> Vec<NodeId> {
        let Some(node) = self.node(id) else { return Vec::new() };
        if node.children.len() < 2 {
            return Vec::new();
        }
        let turns: Vec<NodeId> = node.children.iter().copied().filter(|child| self.is_turn(*child)).collect();
        if turns.len() < 2 { Vec::new() } else { turns }
    }

    pub fn thread_with(&self, overrides: &HashMap<NodeId, NodeId>) -> Vec<NodeId> {
        if overrides.is_empty() {
            return self.thread.clone();
        }
        self.roots.iter().flat_map(|&root| self.descend(root, overrides)).collect()
    }

    fn descend(&self, from: NodeId, overrides: &HashMap<NodeId, NodeId>) -> Vec<NodeId> {
        let mut path = vec![from];
        let mut current = from;
        while let Some(node) = self.node(current) {
            let next = overrides
                .get(&current)
                .copied()
                .filter(|next| node.children.contains(next))
                .or_else(|| self.chosen.get(&current).copied())
                .or_else(|| newest_child(&self.nodes, node));
            let Some(next) = next else { break };
            path.push(next);
            current = next;
        }
        path
    }

    pub fn path_from(&self, root: NodeId) -> Vec<NodeId> {
        self.path_from_with(root, &HashMap::new())
    }

    pub fn path_from_with(&self, root: NodeId, overrides: &HashMap<NodeId, NodeId>) -> Vec<NodeId> {
        if overrides.is_empty() {
            let mut chosen = HashMap::new();
            let target = newest_childless(&self.nodes, root);
            return walk_up(&self.nodes, target, root, &mut chosen);
        }
        self.descend(root, overrides)
    }
}

struct Provisional {
    parent_key: Option<Box<str>>,
    line: u64,
}

pub fn build(path: &Path) -> Result<Conversation, ThreadError> {
    let mut lines = Lines::open(path).map_err(|_| ThreadError::Unreadable(path.to_path_buf()))?;
    let mut diagnostics = Diagnostics::new(path);
    let mut nodes: Vec<Node> = Vec::new();
    let mut provisional: Vec<Provisional> = Vec::new();
    let mut ids: HashMap<Box<str>, NodeId> = HashMap::new();
    let mut fragment_groups: HashMap<(String, Option<String>), usize> = HashMap::new();
    let mut fragment_highest: HashMap<usize, u32> = HashMap::new();
    let mut fragment_blocks: HashMap<usize, Vec<(u32, Vec<Block>)>> = HashMap::new();
    let mut state = SessionState::default();
    let mut line_number: u64 = 0;
    let mut line_offsets: Vec<u64> = Vec::new();

    while let Some(line) = lines.next_line().map_err(|_| ThreadError::Unreadable(path.to_path_buf()))? {
        let line = line.to_vec();
        line_number = line_number.saturating_add(1);
        line_offsets.push(lines.complete_offset());
        let parsed = record::parse(&line);
        if let Ok(record) = &parsed {
            for kind in record.unknown_block_kinds() {
                diagnostics.push(Defect::UnknownBlock { line: line_number, kind: kind.to_owned() });
            }
        }
        match parsed {
            Ok(Record::User(user)) => {
                let parent_key = user.envelope.parent_uuid.clone().map(Box::from);
                push_node(&mut nodes, &mut provisional, &mut ids, line_number, parent_key, NodeKind::User(*user));
            }
            Ok(Record::System(system)) => {
                let is_compact_boundary = system.subtype.as_deref() == Some("compact_boundary");
                let parent_key = if is_compact_boundary {
                    system.logical_parent_uuid.clone().or_else(|| system.envelope.parent_uuid.clone()).map(Box::from)
                } else {
                    system.envelope.parent_uuid.clone().map(Box::from)
                };
                let id = push_node(&mut nodes, &mut provisional, &mut ids, line_number, parent_key, NodeKind::System(*system));
                if is_compact_boundary && let Some(node) = nodes.get_mut(id.index()) {
                    node.divider = Some(Divider::Compacted);
                }
            }
            Ok(Record::Attachment(attachment)) => {
                let parent_key = attachment.envelope.parent_uuid.clone().map(Box::from);
                push_node(&mut nodes, &mut provisional, &mut ids, line_number, parent_key, NodeKind::Attachment(*attachment));
            }
            Ok(Record::Assistant(assistant)) => fold_assistant_fragment(
                *assistant,
                &mut nodes,
                &mut provisional,
                &mut ids,
                line_number,
                &mut fragment_groups,
                &mut fragment_highest,
                &mut fragment_blocks,
            ),
            Ok(Record::Summary(summary)) => state.summary = Some(summary.summary),
            Ok(Record::Latch(latch)) => fold_latch(latch, &mut state),
            Err(ParseError::NoType) => {
                diagnostics.push(Defect::Unparseable { line: line_number, message: "no top-level type".to_owned() });
            }
            Err(ParseError::UnknownType(kind)) => diagnostics.push(Defect::UnknownRecord { line: line_number, kind }),
            Err(ParseError::Json(error)) => {
                diagnostics.push(Defect::Unparseable { line: line_number, message: error.to_string() });
            }
        }
    }

    finalize_assistant_content(&mut nodes, fragment_blocks);

    diagnostics.absorb_line_notes(lines.notes(), |offset| line_of_offset(&line_offsets, offset));

    let mut roots = resolve_parents(&mut nodes, &provisional, &ids, &mut diagnostics);
    sever_cycles(&mut nodes, &mut roots, &provisional, &mut diagnostics);
    let inline = lift_inline_sidechains(&mut nodes);
    order_and_mark_roots(&mut nodes, &mut roots);
    let (thread, chosen) = choose_threads(&nodes, &roots, state.leaf_uuid.as_deref(), &ids);

    let results = index_results(&nodes);

    Ok(Conversation { nodes, ids, results, inline, roots, thread, chosen, state, diagnostics })
}

fn lift_inline_sidechains(nodes: &mut [Node]) -> HashMap<Box<str>, NodeId> {
    let mut lifted = HashMap::new();
    let starts: Vec<(NodeId, NodeId)> = nodes
        .iter()
        .filter(|node| node.is_sidechain())
        .filter_map(|node| {
            let parent = node.parent?;
            nodes.get(parent.index()).filter(|parent| !parent.is_sidechain()).map(|parent| (node.id, parent.id))
        })
        .collect();

    for (start, parent) in starts {
        let Some(tool_use_id) = spawning_call(nodes, parent) else { continue };
        restitch(nodes, start, parent);
        if let Some(node) = nodes.get_mut(parent.index()) {
            node.children.retain(|child| *child != start);
        }
        if let Some(node) = nodes.get_mut(start.index()) {
            node.parent = None;
        }
        lifted.insert(tool_use_id, start);
    }
    lifted
}

fn restitch(nodes: &mut [Node], start: NodeId, parent: NodeId) {
    let mut stack = vec![start];
    let mut rejoining = Vec::new();
    while let Some(id) = stack.pop() {
        let Some(node) = nodes.get(id.index()) else { continue };
        for &child in &node.children {
            if nodes.get(child.index()).is_some_and(Node::is_sidechain) {
                stack.push(child);
            } else {
                rejoining.push((id, child));
            }
        }
    }
    for (inside, child) in rejoining {
        if let Some(node) = nodes.get_mut(inside.index()) {
            node.children.retain(|found| *found != child);
        }
        if let Some(node) = nodes.get_mut(child.index()) {
            node.parent = Some(parent);
        }
        if let Some(node) = nodes.get_mut(parent.index()) {
            node.children.push(child);
        }
    }
}

fn spawning_call(nodes: &[Node], parent: NodeId) -> Option<Box<str>> {
    let NodeKind::Assistant(turn) = &nodes.get(parent.index())?.kind else { return None };
    turn.content.iter().find_map(|block| match block {
        Block::ToolUse { id, name, .. } if name == "Task" || name == "Agent" => Some(Box::from(id.as_str())),
        _ => None,
    })
}

fn index_results(nodes: &[Node]) -> HashMap<Box<str>, NodeId> {
    let mut results = HashMap::new();
    for node in nodes {
        let NodeKind::User(record) = &node.kind else { continue };
        let Content::Blocks(blocks) = &record.message.content else { continue };
        for block in blocks {
            if let Block::ToolResult { tool_use_id: Some(id), .. } = block {
                results.insert(Box::from(id.as_str()), node.id);
            }
        }
    }
    results
}

fn push_node(
    nodes: &mut Vec<Node>,
    provisional: &mut Vec<Provisional>,
    ids: &mut HashMap<Box<str>, NodeId>,
    line: u64,
    parent_key: Option<Box<str>>,
    kind: NodeKind,
) -> NodeId {
    let id = NodeId(u32::try_from(nodes.len()).unwrap_or(u32::MAX));
    let uuid: Box<str> = Box::from(node_uuid(&kind));
    let timestamp = node_timestamp(&kind);
    nodes.push(Node { id, parent: None, children: Vec::new(), timestamp, divider: None, kind });
    provisional.push(Provisional { parent_key, line });
    ids.insert(uuid, id);
    id
}

fn node_uuid(kind: &NodeKind) -> &str {
    match kind {
        NodeKind::User(record) => &record.envelope.uuid,
        NodeKind::Assistant(turn) => &turn.envelope.uuid,
        NodeKind::System(record) => &record.envelope.uuid,
        NodeKind::Attachment(record) => &record.envelope.uuid,
    }
}

const fn node_timestamp(kind: &NodeKind) -> Option<Timestamp> {
    match kind {
        NodeKind::User(record) => record.envelope.timestamp,
        NodeKind::Assistant(turn) => turn.envelope.timestamp,
        NodeKind::System(record) => record.envelope.timestamp,
        NodeKind::Attachment(record) => record.envelope.timestamp,
    }
}

#[allow(clippy::too_many_arguments)]
fn fold_assistant_fragment(
    record: AssistantRecord,
    nodes: &mut Vec<Node>,
    provisional: &mut Vec<Provisional>,
    ids: &mut HashMap<Box<str>, NodeId>,
    line: u64,
    fragment_groups: &mut HashMap<(String, Option<String>), usize>,
    fragment_highest: &mut HashMap<usize, u32>,
    fragment_blocks: &mut HashMap<usize, Vec<(u32, Vec<Block>)>>,
) {
    let Some(message_id) = record.message.id.clone() else {
        let parent_key = record.envelope.parent_uuid.clone().map(Box::from);
        let turn = AssistantTurn {
            envelope: record.envelope.clone(),
            model: record.message.model.clone(),
            content: Vec::new(),
            usage: record.message.usage.clone(),
            stop_reason: record.message.stop_reason.clone(),
            fragments: 1,
            attribution_agent: record.attribution_agent.clone(),
            attribution_skill: record.attribution_skill.clone(),
        };
        let id = push_node(nodes, provisional, ids, line, parent_key, NodeKind::Assistant(turn));
        let index = id.index();
        fragment_blocks.insert(index, vec![(0, as_blocks(record.message.content))]);
        return;
    };

    let key = (message_id, record.request_id.clone());
    let block_index = record.api_block_index.unwrap_or(0);

    if let Some(index) = fragment_groups.get(&key).copied() {
        if let Some(node) = nodes.get_mut(index) {
            ids.insert(Box::from(record.envelope.uuid.as_str()), node.id);
            node.timestamp = record.envelope.timestamp;
            if let NodeKind::Assistant(turn) = &mut node.kind {
                turn.fragments = turn.fragments.saturating_add(1);
                let highest = fragment_highest.entry(index).or_insert(0);
                if block_index >= *highest {
                    *highest = block_index;
                    turn.model.clone_from(&record.message.model);
                    turn.usage.clone_from(&record.message.usage);
                    turn.stop_reason.clone_from(&record.message.stop_reason);
                    turn.attribution_agent.clone_from(&record.attribution_agent);
                    turn.attribution_skill.clone_from(&record.attribution_skill);
                }
            }
        }
        fragment_blocks.entry(index).or_default().push((block_index, as_blocks(record.message.content)));
    } else {
        let parent_key = record.envelope.parent_uuid.clone().map(Box::from);
        let turn = AssistantTurn {
            envelope: record.envelope.clone(),
            model: record.message.model.clone(),
            content: Vec::new(),
            usage: record.message.usage.clone(),
            stop_reason: record.message.stop_reason.clone(),
            fragments: 1,
            attribution_agent: record.attribution_agent.clone(),
            attribution_skill: record.attribution_skill.clone(),
        };
        let id = push_node(nodes, provisional, ids, line, parent_key, NodeKind::Assistant(turn));
        let index = id.index();
        fragment_groups.insert(key, index);
        fragment_highest.insert(index, block_index);
        fragment_blocks.insert(index, vec![(block_index, as_blocks(record.message.content))]);
    }
}

fn as_blocks(content: Content) -> Vec<Block> {
    match content {
        Content::Blocks(blocks) => blocks,
        Content::Text(text) => vec![Block::Text { text }],
    }
}

fn finalize_assistant_content(nodes: &mut [Node], fragment_blocks: HashMap<usize, Vec<(u32, Vec<Block>)>>) {
    for (index, mut fragments) in fragment_blocks {
        fragments.sort_by_key(|(block_index, _)| *block_index);
        let flattened: Vec<Block> = fragments.into_iter().flat_map(|(_, blocks)| blocks).collect();
        if let Some(node) = nodes.get_mut(index)
            && let NodeKind::Assistant(turn) = &mut node.kind
        {
            turn.content = flattened;
        }
    }
}

fn fold_latch(latch: Latch, state: &mut SessionState) {
    match latch {
        Latch::CustomTitle(inner) => state.custom_title = Some(inner.custom_title),
        Latch::AiTitle(inner) => state.ai_title = Some(inner.ai_title),
        Latch::AgentName(inner) => state.agent_name = Some(inner.agent_name),
        Latch::LastPrompt(inner) => {
            state.last_prompt = inner.last_prompt;
            state.leaf_uuid = inner.leaf_uuid;
        }
        Latch::CostState(inner) => state.cost_state = Some(inner),
        Latch::ContinuedIn(inner) => state.continued_in = Some(inner.continued_in_session_id),
        Latch::ForkContextRef(inner) => state.fork_context = Some(inner),
        Latch::Known { .. } => {}
    }
}

fn resolve_parents(
    nodes: &mut [Node],
    provisional: &[Provisional],
    ids: &HashMap<Box<str>, NodeId>,
    diagnostics: &mut Diagnostics,
) -> Vec<NodeId> {
    let mut roots = Vec::new();

    for (index, entry) in provisional.iter().enumerate() {
        let resolved = resolve_one_parent(entry, index, nodes, ids, diagnostics);
        if let Some(node) = nodes.get_mut(index) {
            node.parent = resolved;
        }
        if resolved.is_none()
            && let Some(node) = nodes.get(index)
        {
            roots.push(node.id);
        }
    }

    for index in 0..nodes.len() {
        let Some(parent_id) = nodes.get(index).and_then(|node| node.parent) else { continue };
        let Some(child_id) = nodes.get(index).map(|node| node.id) else { continue };
        if let Some(parent_node) = nodes.get_mut(parent_id.index()) {
            parent_node.children.push(child_id);
        }
    }

    roots
}

fn resolve_one_parent(
    entry: &Provisional,
    index: usize,
    nodes: &mut [Node],
    ids: &HashMap<Box<str>, NodeId>,
    diagnostics: &mut Diagnostics,
) -> Option<NodeId> {
    let key = entry.parent_key.as_ref()?;
    if let Some(&parent_id) = ids.get(key.as_ref()) {
        return Some(parent_id);
    }
    if let Some(node) = nodes.get_mut(index) {
        node.divider = Some(Divider::Detached);
    }
    diagnostics.push(Defect::OrphanedParent { line: entry.line, uuid: key.to_string() });
    None
}

fn sever_cycles(nodes: &mut [Node], roots: &mut Vec<NodeId>, provisional: &[Provisional], diagnostics: &mut Diagnostics) {
    let mut visited = vec![false; nodes.len()];
    mark_reachable(nodes, roots.iter().copied(), &mut visited);

    while let Some(next) = visited.iter().position(|&seen| !seen) {
        let node_id = NodeId(u32::try_from(next).unwrap_or(u32::MAX));
        let old_parent = nodes.get_mut(next).and_then(|node| {
            node.divider = Some(Divider::Detached);
            node.parent.take()
        });
        if let Some(old_parent) = old_parent
            && let Some(parent_node) = nodes.get_mut(old_parent.index())
        {
            parent_node.children.retain(|&child| child != node_id);
        }
        let line = provisional.get(next).map_or(0, |entry| entry.line);
        let uuid = nodes.get(next).map(Node::uuid).unwrap_or_default().to_owned();
        diagnostics.push(Defect::SeveredCycle { line, uuid });
        roots.push(node_id);
        mark_reachable(nodes, std::iter::once(node_id), &mut visited);
    }
}

fn mark_reachable(nodes: &[Node], starts: impl Iterator<Item = NodeId>, visited: &mut [bool]) {
    let mut queue: VecDeque<NodeId> = VecDeque::new();
    for id in starts {
        if let Some(seen) = visited.get_mut(id.index())
            && !*seen
        {
            *seen = true;
            queue.push_back(id);
        }
    }
    while let Some(id) = queue.pop_front() {
        let Some(children) = nodes.get(id.index()).map(|node| node.children.clone()) else { continue };
        for child in children {
            if let Some(seen) = visited.get_mut(child.index())
                && !*seen
            {
                *seen = true;
                queue.push_back(child);
            }
        }
    }
}

fn order_and_mark_roots(nodes: &mut [Node], roots: &mut [NodeId]) {
    roots.sort_by_key(|id| nodes.get(id.index()).and_then(|node| node.timestamp));
    let mut assigned_start = false;
    for &id in roots.iter() {
        let already_detached = nodes.get(id.index()).is_some_and(|node| node.divider == Some(Divider::Detached));
        if already_detached {
            continue;
        }
        if let Some(node) = nodes.get_mut(id.index()) {
            node.divider = Some(if assigned_start { Divider::Clear } else { Divider::SessionStart });
        }
        assigned_start = true;
    }
}

fn choose_threads(
    nodes: &[Node],
    roots: &[NodeId],
    leaf_uuid: Option<&str>,
    ids: &HashMap<Box<str>, NodeId>,
) -> (Vec<NodeId>, HashMap<NodeId, NodeId>) {
    let mut thread = Vec::new();
    let mut chosen = HashMap::new();
    let leaf_id = leaf_uuid.and_then(|uuid| ids.get(uuid)).copied();

    for &root in roots {
        let target =
            leaf_id.filter(|&leaf| ancestor_chain_includes(nodes, leaf, root)).unwrap_or_else(|| newest_childless(nodes, root));
        let path = walk_up(nodes, target, root, &mut chosen);
        thread.extend(path);
    }

    (thread, chosen)
}

fn ancestor_chain_includes(nodes: &[Node], leaf: NodeId, root: NodeId) -> bool {
    let mut current = leaf;
    loop {
        if current == root {
            return true;
        }
        let Some(parent) = nodes.get(current.index()).and_then(|node| node.parent) else { return false };
        current = parent;
    }
}

fn newest_child(nodes: &[Node], node: &Node) -> Option<NodeId> {
    node.children.iter().copied().max_by_key(|id| nodes.get(id.index()).and_then(|child| child.timestamp))
}

fn newest_childless(nodes: &[Node], root: NodeId) -> NodeId {
    let mut stack = vec![root];
    let mut leaves = Vec::new();
    while let Some(id) = stack.pop() {
        let Some(node) = nodes.get(id.index()) else { continue };
        if node.children.is_empty() {
            leaves.push(id);
        } else {
            stack.extend(node.children.iter().copied());
        }
    }
    leaves.into_iter().max_by_key(|id| nodes.get(id.index()).and_then(|node| node.timestamp)).unwrap_or(root)
}

fn walk_up(nodes: &[Node], leaf: NodeId, root: NodeId, chosen: &mut HashMap<NodeId, NodeId>) -> Vec<NodeId> {
    let mut path = vec![leaf];
    let mut current = leaf;
    while current != root {
        let Some(parent) = nodes.get(current.index()).and_then(|node| node.parent) else { break };
        let forks = nodes.get(parent.index()).map_or(0, |node| node.children.len());
        if forks > 1 {
            chosen.insert(parent, current);
        }
        path.push(parent);
        current = parent;
    }
    path.reverse();
    path
}

fn line_of_offset(line_offsets: &[u64], offset: u64) -> u64 {
    let position = line_offsets.iter().position(|&complete| complete > offset).unwrap_or(line_offsets.len());
    u64::try_from(position).unwrap_or(u64::MAX).saturating_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use tempfile::TempDir;

    fn write(lines: &[&str]) -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("a temp dir");
        let path = dir.path().join("session.jsonl");
        let mut content = lines.join("\n");
        content.push('\n');
        fs::write(&path, content).expect("a writable temp file");
        (dir, path)
    }

    #[test]
    fn a_linear_session_is_one_root_and_one_thread() {
        let (_dir, path) = write(&[
            r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#,
            r#"{"parentUuid":"u1","isSidechain":false,"message":{"model":"m","id":"msg1","role":"assistant","content":[{"type":"text","text":"hi back"}]},"type":"assistant","uuid":"a1","timestamp":"2026-01-01T00:01:00Z","sessionId":"s1"}"#,
        ]);
        let conversation = build(&path).expect("a conversation");
        assert_eq!(conversation.roots().len(), 1);
        assert_eq!(conversation.thread().len(), 2);
        assert_eq!(conversation.diagnostics().count(), 0);
        let root = conversation.node(conversation.roots()[0]).expect("the root");
        assert_eq!(root.divider, Some(Divider::SessionStart));
    }

    #[test]
    fn an_unresolvable_parent_becomes_a_detached_root_with_a_diagnostic() {
        let (_dir, path) = write(&[
            r#"{"parentUuid":"missing","isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#,
        ]);
        let conversation = build(&path).expect("a conversation");
        assert_eq!(conversation.roots().len(), 1);
        let root = conversation.node(conversation.roots()[0]).expect("the root");
        assert_eq!(root.divider, Some(Divider::Detached));
        assert_eq!(conversation.diagnostics().count(), 1);
        assert!(matches!(conversation.diagnostics().defects().first(), Some(Defect::OrphanedParent { .. })));
    }

    #[test]
    fn an_unknown_block_in_an_assistant_record_is_counted_and_named() {
        let (_dir, path) = write(&[
            r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#,
            r#"{"parentUuid":"u1","isSidechain":false,"message":{"model":"m","id":"msg1","role":"assistant","content":[{"type":"text","text":"hi"},{"type":"server_tool_use","id":"x","name":"web_search"}]},"type":"assistant","uuid":"a1","timestamp":"2026-01-01T00:01:00Z","sessionId":"s1"}"#,
        ]);
        let conversation = build(&path).expect("a conversation");
        assert_eq!(conversation.diagnostics().defects(), [Defect::UnknownBlock { line: 2, kind: "server_tool_use".to_owned() }]);
    }

    #[test]
    fn an_unknown_block_in_a_later_fragment_is_reported_at_its_own_line() {
        let (_dir, path) = write(&[
            r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#,
            r#"{"parentUuid":"u1","isSidechain":false,"message":{"model":"m","id":"msg1","role":"assistant","content":[{"type":"text","text":"one"}]},"apiBlockIndex":0,"requestId":"r1","type":"assistant","uuid":"a1","timestamp":"2026-01-01T00:01:00Z","sessionId":"s1"}"#,
            r#"{"parentUuid":"a1","isSidechain":false,"message":{"model":"m","id":"msg1","role":"assistant","content":[{"type":"web_search_result","content":[]}]},"apiBlockIndex":1,"requestId":"r1","type":"assistant","uuid":"a2","timestamp":"2026-01-01T00:02:00Z","sessionId":"s1"}"#,
        ]);
        let conversation = build(&path).expect("a conversation");
        assert_eq!(
            conversation.diagnostics().defects(),
            [Defect::UnknownBlock { line: 3, kind: "web_search_result".to_owned() }]
        );
    }

    #[test]
    fn a_two_node_cycle_is_severed_into_a_detached_root() {
        let (_dir, path) = write(&[
            r#"{"parentUuid":"u2","isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#,
            r#"{"parentUuid":"u1","isSidechain":false,"message":{"model":"m","id":"msg1","role":"assistant","content":[{"type":"text","text":"hi"}]},"type":"assistant","uuid":"u2","timestamp":"2026-01-01T00:01:00Z","sessionId":"s1"}"#,
        ]);
        let conversation = build(&path).expect("a conversation");
        assert_eq!(conversation.roots().len(), 1);
        let root = conversation.node(conversation.roots()[0]).expect("the severed root");
        assert_eq!(root.divider, Some(Divider::Detached));
        assert_eq!(conversation.diagnostics().count(), 1);
        assert!(matches!(conversation.diagnostics().defects().first(), Some(Defect::SeveredCycle { .. })));
    }

    #[test]
    fn three_fragments_coalesce_into_one_node_with_the_highest_usage() {
        let (_dir, path) = write(&[
            r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#,
            r#"{"parentUuid":"u1","isSidechain":false,"message":{"model":"m","id":"msg1","role":"assistant","content":[{"type":"text","text":"one"}],"usage":{"input_tokens":1,"output_tokens":1}},"apiBlockIndex":0,"requestId":"r1","type":"assistant","uuid":"a1","timestamp":"2026-01-01T00:01:00Z","sessionId":"s1"}"#,
            r#"{"parentUuid":"a1","isSidechain":false,"message":{"model":"m","id":"msg1","role":"assistant","content":[{"type":"text","text":"two"}],"usage":{"input_tokens":1,"output_tokens":2}},"apiBlockIndex":1,"requestId":"r1","type":"assistant","uuid":"a2","timestamp":"2026-01-01T00:02:00Z","sessionId":"s1"}"#,
            r#"{"parentUuid":"a2","isSidechain":false,"message":{"model":"m","id":"msg1","role":"assistant","content":[{"type":"text","text":"three"}],"usage":{"input_tokens":1,"output_tokens":3}},"apiBlockIndex":2,"requestId":"r1","type":"assistant","uuid":"a3","timestamp":"2026-01-01T00:03:00Z","sessionId":"s1"}"#,
            r#"{"parentUuid":"a3","isSidechain":false,"message":{"role":"user","content":"thanks"},"type":"user","origin":{"kind":"human"},"uuid":"u2","timestamp":"2026-01-01T00:04:00Z","sessionId":"s1"}"#,
        ]);
        let conversation = build(&path).expect("a conversation");
        assert_eq!(conversation.thread().len(), 3, "one user, one coalesced assistant, one user");
        let assistant_id = conversation.id_of("a2").expect("fragment 2's uuid resolves to the coalesced node");
        let assistant = conversation.node(assistant_id).expect("the coalesced node");
        let NodeKind::Assistant(turn) = &assistant.kind else { panic!("expected an assistant node") };
        assert_eq!(turn.fragments, 3);
        assert_eq!(
            turn.usage.as_ref().map(|usage| usage.output_tokens),
            Some(3),
            "usage must come from apiBlockIndex 2, not summed"
        );
        assert_eq!(turn.content.len(), 3);
        let last_child = conversation.id_of("u2").expect("the record parenting off fragment 3");
        assert_eq!(conversation.node(last_child).and_then(|node| node.parent), Some(assistant_id));
    }

    #[test]
    fn a_retry_with_a_different_request_id_is_a_sibling_not_a_merge() {
        let (_dir, path) = write(&[
            r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#,
            r#"{"parentUuid":"u1","isSidechain":false,"message":{"model":"m","id":"msg1","role":"assistant","content":[{"type":"text","text":"one"}]},"apiBlockIndex":0,"requestId":"r1","type":"assistant","uuid":"a1","timestamp":"2026-01-01T00:01:00Z","sessionId":"s1"}"#,
            r#"{"parentUuid":"u1","isSidechain":false,"message":{"model":"m","id":"msg1","role":"assistant","content":[{"type":"text","text":"retry"}]},"apiBlockIndex":0,"requestId":"r2","type":"assistant","uuid":"a2","timestamp":"2026-01-01T00:02:00Z","sessionId":"s1"}"#,
        ]);
        let conversation = build(&path).expect("a conversation");
        let root = conversation.roots().first().copied().expect("a root");
        assert_eq!(conversation.node(root).map(|node| node.children.len()), Some(2), "the retry is a sibling, not a merge");
    }

    #[test]
    fn a_compaction_boundary_stays_in_one_thread_and_carries_the_compacted_divider() {
        let (_dir, path) = write(&[
            r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#,
            r#"{"parentUuid":null,"isSidechain":false,"logicalParentUuid":"u1","type":"system","subtype":"compact_boundary","content":"Conversation compacted","uuid":"c1","timestamp":"2026-01-01T00:01:00Z","sessionId":"s1"}"#,
            r#"{"parentUuid":"c1","isSidechain":false,"message":{"role":"user","content":"continuing"},"type":"user","origin":{"kind":"human"},"uuid":"u2","timestamp":"2026-01-01T00:02:00Z","sessionId":"s1"}"#,
        ]);
        let conversation = build(&path).expect("a conversation");
        assert_eq!(conversation.roots().len(), 1, "the compaction boundary must not create a second root");
        assert_eq!(conversation.thread().len(), 3);
        let boundary = conversation.id_of("c1").expect("the boundary node");
        assert_eq!(conversation.node(boundary).and_then(|node| node.divider), Some(Divider::Compacted));
        assert_eq!(conversation.node(boundary).and_then(|node| node.parent), conversation.id_of("u1"));
    }

    #[test]
    fn a_clear_produces_a_second_root_after_the_first() {
        let (_dir, path) = write(&[
            r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"first"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#,
            r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"after clear"},"type":"user","origin":{"kind":"human"},"uuid":"u2","timestamp":"2026-01-01T00:05:00Z","sessionId":"s1"}"#,
        ]);
        let conversation = build(&path).expect("a conversation");
        assert_eq!(conversation.roots().len(), 2);
        let first = conversation.node(conversation.roots()[0]).expect("the first root");
        let second = conversation.node(conversation.roots()[1]).expect("the second root");
        assert_eq!(first.divider, Some(Divider::SessionStart));
        assert_eq!(second.divider, Some(Divider::Clear));
    }

    #[test]
    fn the_default_branch_follows_the_last_prompt_leaf_uuid() {
        let (_dir, path) = write(&[
            r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#,
            r#"{"parentUuid":"u1","isSidechain":false,"message":{"role":"user","content":"branch a"},"type":"user","origin":{"kind":"human"},"uuid":"u2","timestamp":"2026-01-01T00:01:00Z","sessionId":"s1"}"#,
            r#"{"parentUuid":"u1","isSidechain":false,"message":{"role":"user","content":"branch b"},"type":"user","origin":{"kind":"human"},"uuid":"u3","timestamp":"2026-01-01T00:02:00Z","sessionId":"s1"}"#,
            r#"{"type":"last-prompt","lastPrompt":"branch a","leafUuid":"u2","sessionId":"s1"}"#,
        ]);
        let conversation = build(&path).expect("a conversation");
        let branch_a = conversation.id_of("u2").expect("branch a");
        let branch_b = conversation.id_of("u3").expect("branch b");
        assert!(conversation.thread().contains(&branch_a));
        assert!(!conversation.thread().contains(&branch_b), "the sibling not on the default path stays out of thread");
        let root = conversation.roots().first().copied().expect("a root");
        assert_eq!(conversation.chosen(root), Some(branch_a));
    }

    #[test]
    fn a_root_with_no_reachable_leaf_uuid_falls_back_to_the_newest_childless_node() {
        let (_dir, path) = write(&[
            r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#,
            r#"{"parentUuid":"u1","isSidechain":false,"message":{"role":"user","content":"older"},"type":"user","origin":{"kind":"human"},"uuid":"u2","timestamp":"2026-01-01T00:01:00Z","sessionId":"s1"}"#,
            r#"{"parentUuid":"u1","isSidechain":false,"message":{"role":"user","content":"newer"},"type":"user","origin":{"kind":"human"},"uuid":"u3","timestamp":"2026-01-01T00:02:00Z","sessionId":"s1"}"#,
        ]);
        let conversation = build(&path).expect("a conversation");
        let newer = conversation.id_of("u3").expect("the newer sibling");
        assert!(conversation.thread().contains(&newer));
    }

    #[test]
    fn the_legacy_summary_record_has_no_uuid_and_folds_into_session_state() {
        let (_dir, path) = write(&[r#"{"type":"summary","summary":"Legacy summary","leafUuid":"a1"}"#]);
        let conversation = build(&path).expect("a conversation");
        assert_eq!(conversation.state().summary.as_deref(), Some("Legacy summary"));
        assert!(conversation.roots().is_empty());
    }

    #[test]
    fn a_custom_title_latch_folds_into_session_state() {
        let (_dir, path) = write(&[r#"{"type":"custom-title","customTitle":"Renamed","sessionId":"s1"}"#]);
        let conversation = build(&path).expect("a conversation");
        assert_eq!(conversation.state().custom_title.as_deref(), Some("Renamed"));
    }

    #[test]
    fn a_deep_near_linear_chain_does_not_overflow_the_stack() {
        let mut lines: Vec<String> = vec![
            r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","origin":{"kind":"human"},"uuid":"n0","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#.to_owned(),
        ];
        for index in 1..4000u32 {
            lines.push(format!(
                r#"{{"parentUuid":"n{prev}","isSidechain":false,"message":{{"role":"user","content":"x"}},"type":"user","origin":{{"kind":"human"}},"uuid":"n{index}","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}}"#,
                prev = index.saturating_sub(1)
            ));
        }
        let borrowed: Vec<&str> = lines.iter().map(String::as_str).collect();
        let (_dir, path) = write(&borrowed);
        let conversation = build(&path).expect("a conversation");
        assert_eq!(conversation.thread().len(), 4000);
    }
}
