//! The shell's state, and the reducer that is the only way to change it.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::layout::Size;

use crate::ctx::Ctx;
use crate::domain::diagnostics::Diagnostics;
use crate::domain::project::{Project, ProjectError};
use crate::domain::session::Session;
use crate::domain::subagent::{Agent, Agents};
use crate::domain::thread::{Conversation, NodeId, NodeKind, ThreadError};
use crate::domain::tool;
use crate::render::line::RenderedLine;
use crate::render::message::{self, Anchor, Position, Transcript};
use crate::render::{Branches, Ctx as RenderCtx, Expanded, Outputs, Overflow};
use crate::ui::input::{Action, Motion};
use crate::ui::{Options, columns, diagnostics, listing};

pub const CHROME_ROWS: u16 = 5;
pub const DEBOUNCE: Duration = Duration::from_millis(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Projects,
    Sessions,
    Conversation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Browse,
    Focus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pane {
    pub selected: usize,
    pub top: usize,
}

#[derive(Debug, Clone)]
pub struct Rendered {
    conversation: Conversation,
    path: PathBuf,
    agents: Agents,
    root: Option<NodeId>,
    transcript: Transcript,
    wrapped_at: u16,
    revision: u64,
}

impl Rendered {
    fn new(conversation: Conversation, path: PathBuf, agents: Agents, width: u16, view: &View) -> Self {
        let transcript = message::transcript(&conversation, &view.ctx(usize::from(width), &agents, None));
        Self { conversation, path, agents, root: None, transcript, wrapped_at: width, revision: view.revision }
    }

    fn rooted(&self, root: NodeId, width: u16, view: &View) -> Self {
        let mut rendered = Self {
            conversation: self.conversation.clone(),
            path: self.path.clone(),
            agents: self.agents.clone(),
            root: Some(root),
            transcript: Transcript::default(),
            wrapped_at: width,
            revision: view.revision,
        };
        rendered.rewrap(width, view);
        rendered
    }

    fn rewrap(&mut self, width: u16, view: &View) {
        self.transcript = message::transcript(&self.conversation, &view.ctx(usize::from(width), &self.agents, self.root));
        self.wrapped_at = width;
        self.revision = view.revision;
    }

    pub fn lines(&self) -> &[RenderedLine] {
        &self.transcript.lines
    }

    pub fn anchors(&self) -> &[Anchor] {
        &self.transcript.anchors
    }

    pub const fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    pub const fn conversation(&self) -> &Conversation {
        &self.conversation
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub const fn agents(&self) -> &Agents {
        &self.agents
    }

    fn walk(&self, branches: &Branches) -> Vec<NodeId> {
        self.root.map_or_else(|| self.conversation.thread_with(branches), |root| self.conversation.path_from_with(root, branches))
    }

    pub fn turns(&self) -> usize {
        let walk = self.root.map_or_else(|| self.conversation.thread().to_vec(), |root| self.conversation.path_from(root));
        walk.iter()
            .filter_map(|id| self.conversation.node(*id))
            .filter(|node| match &node.kind {
                NodeKind::Assistant(_) => true,
                NodeKind::User(record) => record.is_human_turn(),
                NodeKind::System(_) | NodeKind::Attachment(_) => false,
            })
            .count()
    }
}

#[derive(Debug, Clone, Default)]
struct View {
    expanded: Expanded,
    outputs: Outputs,
    branches: Branches,
    injections: bool,
    revision: u64,
}

impl View {
    const fn ctx<'a>(&'a self, width: usize, agents: &'a Agents, root: Option<NodeId>) -> RenderCtx<'a> {
        RenderCtx {
            width,
            expanded: &self.expanded,
            outputs: &self.outputs,
            agents,
            root,
            branches: &self.branches,
            injections: self.injections,
        }
    }

    const fn bump(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }
}

#[derive(Debug, Clone)]
pub enum Loadable<T> {
    Loading,
    Ready(T),
    Failed(String),
}

struct Frame {
    conversation: Loadable<Rendered>,
    pane: Pane,
    view: View,
    call_cursor: Option<usize>,
    label: Option<Box<str>>,
}

#[derive(Debug, Clone, Default)]
struct Anchored {
    position: Option<Position>,
    cursor: Option<Box<str>>,
    row: Option<usize>,
}

pub struct App {
    pub ctx: Ctx,
    pub quit: bool,
    claude_dir: PathBuf,
    projects: Loadable<Vec<Project>>,
    sessions: Loadable<Vec<Session>>,
    conversation: Loadable<Rendered>,
    projects_pane: Pane,
    sessions_pane: Pane,
    conversation_pane: Pane,
    focused: Column,
    pre_focus: Column,
    mode: Mode,
    area: Size,
    generation: u64,
    conversation_generation: u64,
    pending_project: Option<String>,
    pending_session: Option<String>,
    pending_session_load: Option<(PathBuf, u64)>,
    pending_conversation_load: Option<(PathBuf, u64)>,
    pending_tool_output: VecDeque<(Box<str>, PathBuf, u64)>,
    pending_conversation_path: Option<PathBuf>,
    conversation_due: Option<Instant>,
    view: View,
    call_cursor: Option<usize>,
    stack: Vec<Frame>,
    label: Option<Box<str>>,
    pending_subagent_load: Option<(Box<str>, PathBuf, u64)>,
    drift: BTreeMap<PathBuf, Diagnostics>,
    diagnostics_open: bool,
    diagnostics_pane: Pane,
}

impl App {
    pub fn new(ctx: Ctx, options: &Options, area: Size) -> Self {
        Self {
            ctx,
            quit: false,
            claude_dir: options.claude_dir.clone(),
            projects: Loadable::Loading,
            sessions: Loadable::Loading,
            conversation: Loadable::Loading,
            projects_pane: Pane::default(),
            sessions_pane: Pane::default(),
            conversation_pane: Pane::default(),
            focused: Column::Projects,
            pre_focus: Column::Projects,
            mode: Mode::Browse,
            area,
            generation: 0,
            conversation_generation: 0,
            pending_project: options.project.clone(),
            pending_session: options.session.clone(),
            pending_session_load: None,
            pending_conversation_load: None,
            pending_tool_output: VecDeque::new(),
            pending_conversation_path: None,
            conversation_due: None,
            view: View::default(),
            call_cursor: None,
            stack: Vec::new(),
            label: None,
            pending_subagent_load: None,
            drift: BTreeMap::new(),
            diagnostics_open: false,
            diagnostics_pane: Pane::default(),
        }
    }

    pub fn claude_dir(&self) -> &Path {
        &self.claude_dir
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn projects(&self) -> &Loadable<Vec<Project>> {
        &self.projects
    }

    pub const fn sessions(&self) -> &Loadable<Vec<Session>> {
        &self.sessions
    }

    pub const fn conversation(&self) -> &Loadable<Rendered> {
        &self.conversation
    }

    pub const fn conversation_generation(&self) -> u64 {
        self.conversation_generation
    }

    pub fn lines(&self) -> &[RenderedLine] {
        match &self.conversation {
            Loadable::Ready(rendered) => rendered.lines(),
            Loadable::Loading | Loadable::Failed(_) => &[],
        }
    }

    pub fn anchors(&self) -> &[Anchor] {
        match &self.conversation {
            Loadable::Ready(rendered) => rendered.anchors(),
            Loadable::Loading | Loadable::Failed(_) => &[],
        }
    }

    pub const fn call_cursor(&self) -> Option<usize> {
        self.call_cursor
    }

    pub const fn depth(&self) -> usize {
        self.stack.len()
    }

    pub fn trail(&self) -> Vec<&str> {
        self.stack.iter().filter_map(|frame| frame.label.as_deref()).chain(self.label.as_deref()).collect()
    }

    pub fn cursor_line(&self) -> Option<usize> {
        let cursor = self.call_cursor?;
        self.anchors().get(cursor).map(|anchor| anchor.line)
    }

    fn anchored(&self) -> Anchored {
        let Loadable::Ready(rendered) = &self.conversation else { return Anchored::default() };
        let cursor = self.call_cursor.and_then(|cursor| self.anchors().get(cursor)).map(|anchor| anchor.id.clone());
        Anchored {
            position: rendered.transcript().position(self.conversation_pane.top),
            row: cursor.as_ref().and_then(|_| self.cursor_line()).map(|line| line.saturating_sub(self.conversation_pane.top)),
            cursor,
        }
    }

    fn restore(&mut self, anchored: &Anchored) {
        self.call_cursor = anchored
            .cursor
            .as_ref()
            .and_then(|key| self.anchors().iter().position(|anchor| anchor.id == *key))
            .or_else(|| self.call_cursor.filter(|_| !self.anchors().is_empty()));
        self.clamp_call_cursor();

        let last = self.last(Column::Conversation);
        if let (Some(row), Some(line)) = (anchored.row, self.cursor_line()) {
            self.conversation_pane.top = line.saturating_sub(row).min(last);
            return;
        }
        let Loadable::Ready(rendered) = &self.conversation else { return };
        if let Some(line) = anchored.position.and_then(|position| rendered.transcript().line_of(position)) {
            self.conversation_pane.top = line.min(last);
            return;
        }
        self.conversation_pane.top = self.conversation_pane.top.min(last);
    }

    pub const fn focused(&self) -> Column {
        self.focused
    }

    pub const fn mode(&self) -> Mode {
        self.mode
    }

    pub const fn area(&self) -> Size {
        self.area
    }

    pub const fn pane(&self, column: Column) -> Pane {
        match column {
            Column::Projects => self.projects_pane,
            Column::Sessions => self.sessions_pane,
            Column::Conversation => self.conversation_pane,
        }
    }

    pub fn selected_project(&self) -> Option<&Project> {
        let Loadable::Ready(projects) = &self.projects else { return None };
        projects.get(self.projects_pane.selected)
    }

    pub fn selected_session(&self) -> Option<&Session> {
        let Loadable::Ready(sessions) = &self.sessions else { return None };
        sessions.get(self.sessions_pane.selected)
    }

    pub const fn take_session_load(&mut self) -> Option<(PathBuf, u64)> {
        self.pending_session_load.take()
    }

    pub fn take_tool_output(&mut self) -> Option<(Box<str>, PathBuf, u64)> {
        self.pending_tool_output.pop_front()
    }

    pub const fn take_subagent_load(&mut self) -> Option<(Box<str>, PathBuf, u64)> {
        self.pending_subagent_load.take()
    }

    pub fn set_subagent(&mut self, generation: u64, result: Result<Box<Conversation>, ThreadError>, path: PathBuf) {
        if generation != self.conversation_generation {
            return;
        }
        let width = columns::conversation_width(self.area, self.mode);
        let agents = self.inherited_agents();
        self.conversation = match result {
            Ok(conversation) => {
                self.record_drift(&conversation);
                Loadable::Ready(Rendered::new(*conversation, path, agents, width, &self.view))
            }
            Err(error) => Loadable::Failed(error.to_string()),
        };
    }

    fn record_drift(&mut self, conversation: &Conversation) {
        let diagnostics = conversation.diagnostics();
        if diagnostics.count() > 0 {
            self.drift.insert(diagnostics.path().to_path_buf(), diagnostics.clone());
        } else {
            self.drift.remove(diagnostics.path());
        }
    }

    pub const fn drift(&self) -> &BTreeMap<PathBuf, Diagnostics> {
        &self.drift
    }

    pub fn unreadable(&self) -> usize {
        self.drift.values().map(Diagnostics::count).fold(0, usize::saturating_add)
    }

    fn inherited_agents(&self) -> Agents {
        match self.stack.last().map(|frame| &frame.conversation) {
            Some(Loadable::Ready(parent)) => parent.agents().clone(),
            _ => Agents::default(),
        }
    }

    pub fn subagent_status(&self) -> Option<String> {
        let label = self.label.as_deref()?;
        let Loadable::Ready(rendered) = &self.conversation else { return Some(format!("{label} · loading")) };
        let turns = rendered.turns();
        let plural = if turns == 1 { "msg" } else { "msgs" };
        Some(format!("{label} · {turns} {plural} · depth {}", self.depth()))
    }

    pub fn set_tool_output(&mut self, generation: u64, id: Box<str>, result: Result<Vec<String>, String>) {
        if generation != self.conversation_generation {
            return;
        }
        let overflow = match result {
            Ok(lines) => Overflow::Lines(Arc::new(lines)),
            Err(error) => Overflow::Failed(error),
        };
        self.view.outputs.insert(id, overflow);
        self.view.bump();
    }

    pub const fn conversation_due(&self) -> Option<Instant> {
        self.conversation_due
    }

    pub fn take_conversation_load(&mut self, now: Instant) -> Option<(PathBuf, u64)> {
        if self.conversation_due.is_none_or(|due| now < due) {
            return None;
        }
        self.conversation_due = None;
        self.pending_conversation_load.take()
    }

    pub fn set_conversation(&mut self, generation: u64, result: Result<Box<Conversation>, ThreadError>, agents: Agents) {
        if generation != self.conversation_generation {
            return;
        }
        let width = columns::conversation_width(self.area, self.mode);
        let path = self.pending_conversation_path.take().unwrap_or_default();
        self.conversation = match result {
            Ok(conversation) => {
                self.record_drift(&conversation);
                Loadable::Ready(Rendered::new(*conversation, path, agents, width, &self.view))
            }
            Err(error) => Loadable::Failed(error.to_string()),
        };
    }

    pub fn reflow(&mut self) {
        let width = columns::conversation_width(self.area, self.mode);
        let revision = self.view.revision;
        let stale = match &self.conversation {
            Loadable::Ready(rendered) => rendered.wrapped_at != width || rendered.revision != revision,
            Loadable::Loading | Loadable::Failed(_) => false,
        };
        if !stale {
            return;
        }
        self.rerender();
    }

    pub fn set_projects(&mut self, generation: u64, result: Result<Vec<Project>, ProjectError>) {
        if generation != self.generation {
            return;
        }
        self.projects = match result {
            Ok(projects) => Loadable::Ready(projects),
            Err(error) => Loadable::Failed(error.to_string()),
        };
        if let Some(wanted) = self.pending_project.take()
            && let Loadable::Ready(projects) = &self.projects
            && let Some(index) = projects.iter().position(|project| matches_project(project, &wanted))
        {
            self.projects_pane.selected = index;
        }
        self.request_sessions_for_selection();
    }

    pub fn set_sessions(&mut self, generation: u64, sessions: Vec<Session>) {
        if generation != self.generation {
            return;
        }
        self.sessions_pane = Pane::default();
        if let Some(wanted) = self.pending_session.take()
            && let Some(index) = sessions.iter().position(|session| session.id == wanted)
        {
            self.sessions_pane.selected = index;
        }
        self.drift = sessions
            .iter()
            .filter(|session| session.diagnostics.count() > 0)
            .map(|session| (session.path.clone(), session.diagnostics.clone()))
            .collect();
        self.sessions = Loadable::Ready(sessions);
        self.request_conversation_for_selection();
    }

    pub const fn diagnostics_open(&self) -> bool {
        self.diagnostics_open
    }

    pub const fn diagnostics_top(&self) -> usize {
        self.diagnostics_pane.top
    }

    pub fn diagnostics_lines(&self) -> Vec<RenderedLine> {
        diagnostics::lines(&self.drift, usize::from(diagnostics::inner(self.diagnostics_area()).width))
    }

    const fn diagnostics_area(&self) -> Size {
        Size::new(self.area.width, self.area.height.saturating_sub(CHROME_ROWS))
    }

    fn toggle_diagnostics(&mut self) {
        self.diagnostics_open = !self.diagnostics_open;
        self.diagnostics_pane = Pane::default();
    }

    fn scroll_diagnostics(&mut self, motion: Motion) {
        let last = self.diagnostics_lines().len().saturating_sub(1);
        let height = usize::from(diagnostics::inner(self.diagnostics_area()).height);
        self.diagnostics_pane.top = listing::scroll_target(motion, self.diagnostics_pane.top, last, height);
    }

    pub fn apply(&mut self, action: Action) {
        if self.diagnostics_open {
            match action {
                Action::Quit => self.quit = true,
                Action::Resize(size) => self.area = size,
                Action::ToggleDiagnostics | Action::Ascend => self.toggle_diagnostics(),
                Action::Move(motion) => self.scroll_diagnostics(motion),
                Action::Focus { .. }
                | Action::Descend
                | Action::ToggleFocusMode
                | Action::NextCall { .. }
                | Action::ToggleCall
                | Action::ToggleAllCalls
                | Action::CycleBranch
                | Action::ToggleInjections => {}
            }
            return;
        }
        match action {
            Action::Quit => self.quit = true,
            Action::Resize(size) => self.area = size,
            Action::ToggleFocusMode => self.toggle_focus_mode(),
            Action::Focus { forward } => self.move_focus(forward),
            Action::Descend => self.descend(),
            Action::Ascend => self.ascend(),
            Action::Move(motion) => self.move_selection(motion),
            Action::NextCall { forward } => self.move_call_cursor(forward),
            Action::ToggleCall => self.toggle_call(),
            Action::ToggleAllCalls => self.toggle_all_calls(),
            Action::CycleBranch => self.cycle_branch(),
            Action::ToggleInjections => self.toggle_injections(),
            Action::ToggleDiagnostics => self.toggle_diagnostics(),
        }
    }

    fn toggle_injections(&mut self) {
        self.view.injections = !self.view.injections;
        self.view.bump();
        self.rerender();
    }

    fn cycle_branch(&mut self) {
        let Some(key) = self.call_cursor.and_then(|cursor| self.anchors().get(cursor)).map(|anchor| anchor.id.clone()) else {
            return;
        };
        let Loadable::Ready(rendered) = &self.conversation else { return };
        let conversation = rendered.conversation();
        let Some(fork) = conversation.id_of(&key) else { return };
        let alternates = conversation.alternates(fork);
        if alternates.len() < 2 {
            return;
        }
        let showing = rendered
            .walk(&self.view.branches)
            .iter()
            .find_map(|on| alternates.iter().position(|alternate| alternate == on))
            .unwrap_or(0);
        let next = showing.saturating_add(1).checked_rem(alternates.len()).unwrap_or(0);
        let Some(&child) = alternates.get(next) else { return };
        self.view.branches.insert(fork, child);
        self.view.bump();
        self.rerender();
    }

    fn clamp_call_cursor(&mut self) {
        let calls = self.anchors().len();
        self.call_cursor = self.call_cursor.filter(|_| calls > 0).map(|cursor| cursor.min(calls.saturating_sub(1)));
    }

    fn move_call_cursor(&mut self, forward: bool) {
        let calls = self.anchors().len();
        if calls == 0 {
            return;
        }
        let last = calls.saturating_sub(1);
        self.call_cursor = Some(match (self.call_cursor, forward) {
            (None, true) => 0,
            (None, false) => last,
            (Some(cursor), true) => cursor.saturating_add(1).min(last),
            (Some(cursor), false) => cursor.saturating_sub(1),
        });
        self.reveal_call_cursor();
    }

    fn reveal_call_cursor(&mut self) {
        let Some(line) = self.cursor_line() else { return };
        let height = columns::conversation_height(self.area);
        self.conversation_pane.top = listing::revealed(self.conversation_pane.top, line, height);
    }

    fn toggle_call(&mut self) {
        let Some(cursor) = self.call_cursor else {
            self.move_call_cursor(true);
            return;
        };
        let Some(id) = self.anchors().get(cursor).map(|anchor| anchor.id.clone()) else { return };
        let row = self.cursor_line().map(|line| line.saturating_sub(self.conversation_pane.top));
        if !self.view.expanded.remove(&id) {
            self.view.expanded.insert(id.clone());
            self.queue_tool_output(&id);
        }
        self.view.bump();
        self.anchor_at(&id, row);
    }

    fn toggle_all_calls(&mut self) {
        let ids: Vec<Box<str>> = self.anchors().iter().map(|anchor| anchor.id.clone()).collect();
        if ids.is_empty() {
            return;
        }
        let anchor = self.call_cursor.and_then(|cursor| self.anchors().get(cursor)).map(|anchor| anchor.id.clone());
        let row = self.cursor_line().map(|line| line.saturating_sub(self.conversation_pane.top));
        if self.view.expanded.is_empty() {
            for id in &ids {
                self.view.expanded.insert(id.clone());
                self.queue_tool_output(id);
            }
        } else {
            self.view.expanded.clear();
        }
        self.view.bump();
        match anchor {
            Some(anchor) => self.anchor_at(&anchor, row),
            None => self.rerender(),
        }
    }

    fn rerender(&mut self) {
        let anchored = self.anchored();
        let width = columns::conversation_width(self.area, self.mode);
        if let Loadable::Ready(rendered) = &mut self.conversation {
            rendered.rewrap(width, &self.view);
        }
        self.restore(&anchored);
    }

    fn anchor_at(&mut self, id: &str, row: Option<usize>) {
        let anchored = Anchored { position: None, cursor: Some(Box::from(id)), row };
        let width = columns::conversation_width(self.area, self.mode);
        if let Loadable::Ready(rendered) = &mut self.conversation {
            rendered.rewrap(width, &self.view);
        }
        self.restore(&anchored);
        if row.is_none() {
            self.reveal_call_cursor();
        }
    }

    fn queue_tool_output(&mut self, id: &str) {
        let Loadable::Ready(rendered) = &self.conversation else { return };
        let Some(node) = rendered.conversation().result_of(id) else { return };
        let Some(outcome) = tool::Outcome::of(node, id) else { return };
        let Some(found) = outcome.detail.and_then(tool::overflow) else { return };
        if self.view.outputs.contains_key(id) {
            return;
        }
        let path = tool::overflow_path(rendered.path(), found.name);
        self.view.outputs.insert(Box::from(id), Overflow::Pending);
        self.pending_tool_output.push_back((Box::from(id), path, self.conversation_generation));
    }

    const fn toggle_focus_mode(&mut self) {
        self.mode = match self.mode {
            Mode::Browse => {
                self.pre_focus = self.focused;
                self.focused = Column::Conversation;
                Mode::Focus
            }
            Mode::Focus => {
                self.focused = self.pre_focus;
                Mode::Browse
            }
        };
    }

    fn descend(&mut self) {
        if self.focused != Column::Conversation {
            self.move_focus(true);
            return;
        }
        if self.enter_subagent() {
            return;
        }
        self.toggle_call();
    }

    fn ascend(&mut self) {
        if self.leave_subagent() {
            return;
        }
        if self.mode == Mode::Focus {
            self.toggle_focus_mode();
            return;
        }
        self.move_focus(false);
    }

    fn selected_key(&self) -> Option<&str> {
        let cursor = self.call_cursor?;
        self.anchors().get(cursor)?.agent.as_deref()
    }

    fn enter_inline_subagent(&mut self) -> bool {
        let width = columns::conversation_width(self.area, self.mode);
        let Some(key) = self.selected_key().map(str::to_owned) else { return false };
        let Loadable::Ready(rendered) = &self.conversation else { return false };
        let Some(root) = rendered.conversation().inline_agent(&key) else { return false };
        let label: Box<str> = rendered
            .conversation()
            .node(root)
            .and_then(|node| node.timestamp.map(|_| "sidechain"))
            .map_or_else(|| Box::from("sidechain"), Box::from);
        let child = rendered.rooted(root, width, &View::default());

        self.stack.push(Frame {
            conversation: std::mem::replace(&mut self.conversation, Loadable::Ready(child)),
            pane: self.conversation_pane,
            view: std::mem::take(&mut self.view),
            call_cursor: self.call_cursor.take(),
            label: self.label.replace(label),
        });
        self.conversation_pane = Pane::default();
        true
    }

    fn selected_agent(&self) -> Option<&Agent> {
        let cursor = self.call_cursor?;
        let id = self.anchors().get(cursor)?.agent.as_deref()?;
        let Loadable::Ready(rendered) = &self.conversation else { return None };
        rendered.agents().by_id(id)
    }

    fn enter_subagent(&mut self) -> bool {
        if self.enter_inline_subagent() {
            return true;
        }
        let Some((id, path, label)) = self
            .selected_agent()
            .and_then(|agent| Some((agent.id.clone(), agent.transcript.clone()?, Box::<str>::from(agent.label()))))
        else {
            return false;
        };

        self.conversation_generation = self.conversation_generation.saturating_add(1);
        self.pending_tool_output.clear();
        self.stack.push(Frame {
            conversation: std::mem::replace(&mut self.conversation, Loadable::Loading),
            pane: self.conversation_pane,
            view: std::mem::take(&mut self.view),
            call_cursor: self.call_cursor.take(),
            label: self.label.replace(label),
        });
        self.conversation_pane = Pane::default();
        self.pending_subagent_load = Some((id, path, self.conversation_generation));
        true
    }

    fn leave_subagent(&mut self) -> bool {
        let Some(frame) = self.stack.pop() else { return false };
        self.conversation_generation = self.conversation_generation.saturating_add(1);
        self.pending_subagent_load = None;
        self.pending_tool_output.clear();
        self.conversation = frame.conversation;
        self.conversation_pane = frame.pane;
        self.view = frame.view;
        self.call_cursor = frame.call_cursor;
        self.label = frame.label;
        self.rerender();
        self.conversation_pane.top = frame.pane.top.min(self.last(Column::Conversation));
        true
    }

    fn move_focus(&mut self, forward: bool) {
        if self.mode == Mode::Focus {
            return;
        }
        self.focused = match (self.focused, forward) {
            (Column::Sessions, true) => Column::Conversation,
            (Column::Sessions, false) => Column::Projects,
            (Column::Projects, true) | (Column::Conversation, false) => Column::Sessions,
            (column, _) => column,
        };
    }

    fn move_selection(&mut self, motion: Motion) {
        let column = self.focused;
        if column == Column::Conversation {
            self.scroll_conversation(motion);
            return;
        }
        let last = self.last(column);
        let height = self.pane_height();
        let pane = self.pane_mut(column);

        pane.selected = listing::target(motion, pane.selected, last, height);
        pane.top = listing::revealed(pane.top, pane.selected, height);

        match column {
            Column::Projects => self.request_sessions_for_selection(),
            Column::Sessions => self.request_conversation_for_selection(),
            Column::Conversation => {}
        }
    }

    fn scroll_conversation(&mut self, motion: Motion) {
        let last = self.last(Column::Conversation);
        let height = columns::conversation_height(self.area);
        self.conversation_pane.top = listing::scroll_target(motion, self.conversation_pane.top, last, height);
    }

    fn request_sessions_for_selection(&mut self) {
        self.generation = self.generation.saturating_add(1);
        self.sessions = Loadable::Loading;
        self.sessions_pane = Pane::default();
        if let Some(project) = self.selected_project() {
            let dir = self.claude_dir.join("projects").join(&project.directory);
            self.pending_session_load = Some((dir, self.generation));
        } else {
            self.pending_session_load = None;
            self.sessions = Loadable::Ready(Vec::new());
        }
        self.request_conversation_for_selection();
    }

    fn request_conversation_for_selection(&mut self) {
        self.stack.clear();
        self.label = None;
        self.pending_subagent_load = None;
        self.conversation_generation = self.conversation_generation.saturating_add(1);
        self.conversation = Loadable::Loading;
        self.conversation_pane = Pane::default();
        self.view = View::default();
        self.call_cursor = None;
        self.pending_tool_output.clear();
        self.pending_conversation_path = self.selected_session().map(|session| session.path.clone());
        self.pending_conversation_load =
            self.selected_session().map(|session| (session.path.clone(), self.conversation_generation));
        self.conversation_due =
            self.pending_conversation_load.as_ref().map(|_| Instant::now().checked_add(DEBOUNCE).unwrap_or_else(Instant::now));
    }

    const fn pane_mut(&mut self, column: Column) -> &mut Pane {
        match column {
            Column::Projects => &mut self.projects_pane,
            Column::Sessions => &mut self.sessions_pane,
            Column::Conversation => &mut self.conversation_pane,
        }
    }

    pub fn last(&self, column: Column) -> usize {
        match column {
            Column::Projects => match &self.projects {
                Loadable::Ready(projects) => projects.len().saturating_sub(1),
                _ => 0,
            },
            Column::Sessions => match &self.sessions {
                Loadable::Ready(sessions) => sessions.len().saturating_sub(1),
                _ => 0,
            },
            Column::Conversation => self.lines().len().saturating_sub(1),
        }
    }

    fn pane_height(&self) -> usize {
        usize::from(self.area.height.saturating_sub(CHROME_ROWS)).max(1)
    }
}

fn matches_project(project: &Project, wanted: &str) -> bool {
    project.directory == wanted
        || project.path.display().to_string() == wanted
        || project.path.file_name().is_some_and(|name| name == wanted)
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;
    use crate::domain::diagnostics::Diagnostics;
    use crate::domain::project::Resolution;

    fn ctx() -> Ctx {
        Ctx { now: "2026-01-12T00:00:00Z".parse().expect("a valid instant") }
    }

    fn app(area: Size) -> App {
        App::new(ctx(), &Options { claude_dir: PathBuf::from("/tmp"), ..Options::default() }, area)
    }

    fn project(directory: &str) -> Project {
        Project {
            directory: directory.to_owned(),
            path: PathBuf::from(format!("/Users/fixture/{directory}")),
            resolution: Resolution::Mapped,
            sessions: 1,
            last_activity: SystemTime::UNIX_EPOCH,
            present: true,
        }
    }

    fn session(id: &str) -> Session {
        Session {
            id: id.to_owned(),
            path: PathBuf::from(format!("/tmp/{id}.jsonl")),
            title: id.to_owned(),
            title_source: crate::domain::session::TitleSource::FirstMessage,
            slug: None,
            git_branch: None,
            first_activity: None,
            last_activity: None,
            records: 1,
            messages: 1,
            continued_in: None,
            diagnostics: Diagnostics::new(Path::new("session.jsonl")),
        }
    }

    fn with_calls(area: Size) -> App {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let path = dir.path().join("session.jsonl");
        let human = r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:00Z","origin":{"kind":"human"},"message":{"role":"user","content":"do three things"}}"#;
        let mut lines = vec![human.to_owned()];
        for index in 0..3u32 {
            let parent = if index == 0 { "u1".to_owned() } else { format!("r{}", index.saturating_sub(1)) };
            lines.push(format!(
                r#"{{"type":"assistant","uuid":"a{index}","parentUuid":"{parent}","sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:01Z","requestId":"q{index}","message":{{"model":"opus-5","id":"m{index}","role":"assistant","content":[{{"type":"tool_use","id":"t{index}","name":"Bash","input":{{"command":"step {index}","description":"a step"}}}}]}}}}"#
            ));
            lines.push(format!(
                r#"{{"type":"user","uuid":"r{index}","parentUuid":"a{index}","sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:02Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t{index}","content":"line one\nline two","is_error":false}}]}},"toolUseResult":{{"stdout":"line one\nline two\n"}}}}"#
            ));
        }
        std::fs::write(&path, lines.join("\n") + "\n").expect("a written transcript");
        let conversation = crate::domain::thread::build(&path).expect("a built conversation");
        let mut app = app(area);
        app.pending_conversation_path = Some(path);
        app.set_conversation(app.conversation_generation(), Ok(Box::new(conversation)), Agents::default());
        let _ = dir.keep();
        app
    }

    fn call_lines(app: &App) -> Vec<String> {
        app.lines().iter().map(RenderedLine::text).filter(|text| text.contains('▸') || text.contains('▾')).collect()
    }

    fn with_subagent(area: Size) -> App {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("data").join("claude").join("projects");
        let path = root.join("-Users-fixture-Developer-holodeck").join("11111111-1111-4111-8111-111111111111.jsonl");
        let conversation = crate::domain::thread::build(&path).expect("the baseline session");
        let agents = crate::domain::subagent::discover(&path);
        let mut app = app(area);
        app.pending_conversation_path = Some(path);
        app.set_conversation(app.conversation_generation(), Ok(Box::new(conversation)), agents);
        app
    }

    fn enter_first_subagent(app: &mut App) {
        app.apply(Action::Focus { forward: true });
        app.apply(Action::Focus { forward: true });
        while app.anchors().get(app.call_cursor().unwrap_or(0)).is_none_or(|anchor| anchor.agent.is_none()) {
            let before = app.call_cursor();
            app.apply(Action::NextCall { forward: true });
            assert_ne!(app.call_cursor(), before, "ran out of calls before finding a subagent");
        }
    }

    #[test]
    fn entering_a_subagent_pushes_a_frame_and_leaving_pops_it() {
        let mut app = with_subagent(Size::new(120, 30));
        assert_eq!(app.depth(), 0);
        enter_first_subagent(&mut app);
        let before = app.lines().len();

        app.apply(Action::Descend);
        assert_eq!(app.depth(), 1, "Enter on a subagent call descends rather than expanding it");
        assert_eq!(app.trail(), ["Explore"], "the breadcrumb names the agent");
        let (id, path, generation) = app.take_subagent_load().expect("a subagent load was queued");
        assert_eq!(&*id, "a1b2c3d4e5f607182");
        assert!(path.ends_with("subagents/agent-a1b2c3d4e5f607182.jsonl"));
        assert_eq!(generation, app.conversation_generation());

        app.apply(Action::Ascend);
        assert_eq!(app.depth(), 0, "Esc pops the frame before it exits focus mode");
        assert!(app.trail().is_empty());
        assert_eq!(app.lines().len(), before, "the parent came back exactly as it was");
    }

    #[test]
    fn leaving_a_subagent_restores_the_scroll_position_and_the_call_it_came_from() {
        let mut app = with_subagent(Size::new(120, 12));
        enter_first_subagent(&mut app);
        app.apply(Action::Move(Motion::Line(3)));
        let top = app.pane(Column::Conversation).top;
        let cursor = app.call_cursor();

        app.apply(Action::Descend);
        app.apply(Action::Ascend);
        assert_eq!(app.pane(Column::Conversation).top, top, "the reading position came back");
        assert_eq!(app.call_cursor(), cursor, "and so did the call it was on");
    }

    #[test]
    fn a_subagent_load_that_arrives_after_leaving_is_dropped() {
        let mut app = with_subagent(Size::new(120, 30));
        enter_first_subagent(&mut app);
        app.apply(Action::Descend);
        let (_, path, stale) = app.take_subagent_load().expect("a queued load");
        app.apply(Action::Ascend);

        let conversation = crate::domain::thread::build(&path).expect("the agent");
        app.set_subagent(stale, Ok(Box::new(conversation)), path);
        assert_eq!(app.depth(), 0);
        assert!(app.lines().iter().any(|line| line.text().contains("read the grid scanner")), "the parent is still on screen");
    }

    #[test]
    fn escape_pops_a_frame_before_it_leaves_focus_mode() {
        let mut app = with_subagent(Size::new(120, 30));
        enter_first_subagent(&mut app);
        app.apply(Action::ToggleFocusMode);
        app.apply(Action::Descend);
        assert_eq!(app.depth(), 1);

        app.apply(Action::Ascend);
        assert_eq!(app.depth(), 0);
        assert_eq!(app.mode(), Mode::Focus, "focus mode outlives the drill-in");
        app.apply(Action::Ascend);
        assert_eq!(app.mode(), Mode::Browse);
    }

    #[test]
    fn selecting_another_session_while_nested_clears_the_stack_rather_than_orphaning_it() {
        let mut app = with_subagent(Size::new(120, 30));
        enter_first_subagent(&mut app);
        app.apply(Action::Descend);
        assert_eq!(app.depth(), 1);

        app.set_projects(app.generation(), Ok(vec![project("a")]));
        app.set_sessions(app.generation(), vec![session("s1"), session("s2")]);
        assert_eq!(app.depth(), 0, "the stack cannot outlive the session it was rooted in");
        assert!(app.trail().is_empty());
        assert!(app.take_subagent_load().is_none(), "and the queued load goes with it");
    }

    #[test]
    fn enter_still_expands_a_call_that_spawned_no_subagent() {
        let mut app = with_subagent(Size::new(120, 30));
        app.apply(Action::Focus { forward: true });
        app.apply(Action::Focus { forward: true });
        app.apply(Action::NextCall { forward: true });
        assert!(app.anchors().first().is_some_and(|anchor| anchor.agent.is_none()), "the first call is a Bash");

        app.apply(Action::Descend);
        assert_eq!(app.depth(), 0, "a plain call does not descend");
        assert!(app.lines().iter().any(|line| line.text().contains('▾')), "it expands instead");
    }

    fn with_inline_sidechain(area: Size) -> App {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("data")
            .join("claude")
            .join("projects")
            .join("-Users-fixture-Developer-holodeck")
            .join("44444444-4444-4444-8444-444444444444.jsonl");
        let conversation = crate::domain::thread::build(&path).expect("the drift fixture");
        let agents = crate::domain::subagent::discover(&path);
        let mut app = app(area);
        app.pending_conversation_path = Some(path);
        app.set_conversation(app.conversation_generation(), Ok(Box::new(conversation)), agents);
        app
    }

    #[test]
    fn a_legacy_inline_sidechain_is_entered_without_reading_another_file() {
        let mut app = with_inline_sidechain(Size::new(120, 30));
        app.apply(Action::Focus { forward: true });
        app.apply(Action::Focus { forward: true });
        app.apply(Action::NextCall { forward: true });
        assert!(app.anchors().first().is_some_and(|anchor| anchor.agent.is_some()), "the Task call owns a sidechain");

        app.apply(Action::Descend);
        assert_eq!(app.depth(), 1);
        assert!(app.take_subagent_load().is_none(), "an inline sidechain is already in memory");
        let text: Vec<String> = app.lines().iter().map(RenderedLine::text).collect();
        assert!(text.iter().any(|line| line.contains("prompt")), "its opening turn is the spawning prompt: {text:?}");
        assert!(text.iter().any(|line| line.contains("Four shapes, all still readable")), "{text:?}");
        assert!(!text.iter().any(|line| line.contains("Glob")), "the main thread did not come with it: {text:?}");

        app.apply(Action::Ascend);
        assert_eq!(app.depth(), 0);
        assert!(app.lines().iter().any(|line| line.text().contains("Glob")), "the session came back");
    }

    fn wide_transcript(area: Size) -> App {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let path = dir.path().join("session.jsonl");
        let prose = "the deflector array reads back one plate at a time and then the next ".repeat(6);
        let mut lines = vec![
            r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:00Z","origin":{"kind":"human"},"message":{"role":"user","content":"start"}}"#
                .to_owned(),
        ];
        for index in 0..12u32 {
            let parent = if index == 0 { "u1".to_owned() } else { format!("a{}", index.saturating_sub(1)) };
            lines.push(format!(
                r#"{{"type":"assistant","uuid":"a{index}","parentUuid":"{parent}","sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:01Z","requestId":"q{index}","message":{{"model":"opus-5","id":"m{index}","role":"assistant","content":[{{"type":"text","text":"{prose}"}}]}}}}"#
            ));
        }
        std::fs::write(&path, lines.join("\n") + "\n").expect("a written transcript");
        let conversation = crate::domain::thread::build(&path).expect("a built conversation");
        let mut app = app(area);
        app.pending_conversation_path = Some(path);
        app.set_conversation(app.conversation_generation(), Ok(Box::new(conversation)), Agents::default());
        let _ = dir.keep();
        app
    }

    fn node_at_top(app: &App) -> Option<crate::render::message::Position> {
        let Loadable::Ready(rendered) = app.conversation() else { return None };
        rendered.transcript().position(app.pane(Column::Conversation).top)
    }

    #[test]
    fn a_resize_keeps_the_node_being_read_at_the_top_rather_than_the_line_number() {
        let mut app = wide_transcript(Size::new(120, 20));
        app.apply(Action::Focus { forward: true });
        app.apply(Action::Focus { forward: true });
        app.apply(Action::Move(Motion::HalfPage(1)));
        app.apply(Action::Move(Motion::HalfPage(1)));
        let before = node_at_top(&app).expect("a node at the top");
        let top_before = app.pane(Column::Conversation).top;

        app.apply(Action::Resize(Size::new(48, 20)));
        app.reflow();

        assert_ne!(app.lines().len(), 0);
        assert_ne!(app.pane(Column::Conversation).top, top_before, "a narrower column must move the line index");
        let after = node_at_top(&app).expect("a node at the top");
        assert_eq!(after.node, before.node, "the same node is still at the top of the viewport");
    }

    #[test]
    fn entering_focus_mode_keeps_the_node_being_read_at_the_top() {
        let mut app = wide_transcript(Size::new(120, 20));
        app.apply(Action::Focus { forward: true });
        app.apply(Action::Focus { forward: true });
        app.apply(Action::Move(Motion::HalfPage(1)));
        let before = node_at_top(&app).expect("a node at the top");

        app.apply(Action::ToggleFocusMode);
        app.reflow();
        assert_eq!(node_at_top(&app).map(|position| position.node), Some(before.node));

        app.apply(Action::ToggleFocusMode);
        app.reflow();
        assert_eq!(node_at_top(&app).map(|position| position.node), Some(before.node), "and back again");
    }

    #[test]
    fn the_call_cursor_is_re_found_by_key_rather_than_by_index() {
        let mut app = with_calls(Size::new(120, 30));
        app.apply(Action::NextCall { forward: true });
        app.apply(Action::NextCall { forward: true });
        let Some(key) = app.anchors().get(1).map(|anchor| anchor.id.clone()) else { panic!("a second call") };

        app.apply(Action::Resize(Size::new(60, 30)));
        app.reflow();
        let cursor = app.call_cursor().expect("a cursor");
        assert_eq!(app.anchors().get(cursor).map(|anchor| anchor.id.clone()), Some(key), "the same call, not the same index");
    }

    fn fixture_session(name: &str, area: Size) -> App {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("data")
            .join("claude")
            .join("projects")
            .join("-Users-fixture-Developer-holodeck")
            .join(format!("{name}.jsonl"));
        let conversation = crate::domain::thread::build(&path).expect("a built conversation");
        let agents = crate::domain::subagent::discover(&path);
        let mut app = app(area);
        app.pending_conversation_path = Some(path);
        app.set_conversation(app.conversation_generation(), Ok(Box::new(conversation)), agents);
        app.apply(Action::ToggleFocusMode);
        app.reflow();
        app
    }

    fn text_of(app: &App) -> Vec<String> {
        app.lines().iter().map(RenderedLine::text).collect()
    }

    fn on_marker(app: &mut App, wanted: &str) {
        for _ in 0..app.anchors().len().saturating_add(1) {
            if app.cursor_line().and_then(|line| app.lines().get(line)).is_some_and(|line| line.text().contains(wanted)) {
                return;
            }
            app.apply(Action::NextCall { forward: true });
        }
        panic!("no branch marker matching {wanted:?} to land on");
    }

    #[test]
    fn a_marker_appears_only_where_more_than_one_child_actually_renders() {
        let app = fixture_session("33333333-3333-4333-8333-333333333333", Size::new(120, 40));
        let markers = text_of(&app).iter().filter(|line| line.contains("alternate branches here")).count();
        assert_eq!(markers, 2, "the two genuine forks, and nothing on the parallel tool results");

        let quiet = fixture_session("11111111-1111-4111-8111-111111111111", Size::new(120, 40));
        assert!(!text_of(&quiet).iter().any(|line| line.contains("alternate branches")), "a linear session has no markers");
    }

    #[test]
    fn b_cycles_through_every_alternate_and_back_to_where_it_started() {
        let mut app = fixture_session("33333333-3333-4333-8333-333333333333", Size::new(120, 40));
        on_marker(&mut app, "3 alternate branches");
        let start = text_of(&app);
        assert!(
            text_of(&app).iter().any(|line| line.contains("3 alternate branches")),
            "the three-way fork is the one being cycled: {start:?}"
        );

        app.apply(Action::CycleBranch);
        assert_ne!(text_of(&app), start, "switching a branch must change what is on screen");
        app.apply(Action::CycleBranch);
        assert_ne!(text_of(&app), start);
        app.apply(Action::CycleBranch);
        assert_eq!(text_of(&app), start, "three presses on a three-way fork come back to the start");
    }

    #[test]
    fn b_on_something_that_is_not_a_marker_does_nothing() {
        let mut app = fixture_session("11111111-1111-4111-8111-111111111111", Size::new(120, 40));
        app.apply(Action::NextCall { forward: true });
        let before = text_of(&app);
        app.apply(Action::CycleBranch);
        assert_eq!(text_of(&app), before, "the cursor is on a Bash call, not a fork");
    }

    #[test]
    fn i_reveals_the_injections_and_hides_them_again() {
        let mut app = fixture_session("11111111-1111-4111-8111-111111111111", Size::new(120, 40));
        let hidden = text_of(&app);
        assert!(!hidden.iter().any(|line| line.contains("injections")), "off by default");

        app.apply(Action::ToggleInjections);
        let shown = text_of(&app);
        assert!(shown.iter().any(|line| line.contains("5 injections")), "a run collapses to one line: {shown:?}");
        assert!(
            shown.iter().any(|line| line.contains("date, instructions, total_tokens_reminder +1")),
            "three kinds named and the rest counted: {shown:?}"
        );

        app.apply(Action::ToggleInjections);
        assert_eq!(text_of(&app), hidden, "and back to exactly where it was");
    }

    #[test]
    fn revealing_injections_does_not_move_the_reading_position() {
        let mut app = fixture_session("11111111-1111-4111-8111-111111111111", Size::new(120, 12));
        app.apply(Action::Move(Motion::HalfPage(1)));
        app.apply(Action::Move(Motion::HalfPage(1)));
        let before = node_at_top(&app).expect("a node at the top");

        app.apply(Action::ToggleInjections);
        assert_eq!(node_at_top(&app).map(|position| position.node), Some(before.node), "the same node is still being read");

        app.apply(Action::ToggleInjections);
        assert_eq!(node_at_top(&app).map(|position| position.node), Some(before.node));
    }

    #[test]
    fn revealing_injections_does_not_renumber_the_selected_tool_call() {
        let mut app = fixture_session("11111111-1111-4111-8111-111111111111", Size::new(120, 40));
        app.apply(Action::NextCall { forward: true });
        app.apply(Action::NextCall { forward: true });
        let Some(key) = app.call_cursor().and_then(|cursor| app.anchors().get(cursor)).map(|anchor| anchor.id.clone()) else {
            panic!("a selected call")
        };

        app.apply(Action::ToggleInjections);
        let cursor = app.call_cursor().expect("still a cursor");
        assert_eq!(
            app.anchors().get(cursor).map(|anchor| anchor.id.clone()),
            Some(key),
            "the injection run inserts an anchor above; the cursor must follow the call, not the index"
        );
    }

    #[test]
    fn a_transcript_with_no_calls_has_no_cursor_to_move() {
        let mut app = app(Size::new(120, 30));
        app.apply(Action::NextCall { forward: true });
        assert_eq!(app.call_cursor(), None);
        app.apply(Action::ToggleCall);
        assert_eq!(app.call_cursor(), None, "nothing to expand and nothing to select");
    }

    #[test]
    fn the_call_cursor_hops_between_calls_and_stops_at_both_ends() {
        let mut app = with_calls(Size::new(120, 30));
        assert_eq!(app.anchors().len(), 3);
        assert_eq!(app.call_cursor(), None, "quiet until it is asked for");

        app.apply(Action::NextCall { forward: true });
        assert_eq!(app.call_cursor(), Some(0), "the first call, not the second");
        app.apply(Action::NextCall { forward: true });
        app.apply(Action::NextCall { forward: true });
        app.apply(Action::NextCall { forward: true });
        assert_eq!(app.call_cursor(), Some(2), "no call past the last");
        for _ in 0..5 {
            app.apply(Action::NextCall { forward: false });
        }
        assert_eq!(app.call_cursor(), Some(0), "no call before the first");
    }

    #[test]
    fn the_cursor_line_is_the_line_the_calls_header_is_on() {
        let mut app = with_calls(Size::new(120, 30));
        app.apply(Action::NextCall { forward: true });
        let line = app.cursor_line().expect("a cursor line");
        let text = app.lines().get(line).map(RenderedLine::text).unwrap_or_default();
        assert!(text.contains("▸ Bash  step 0"), "{text:?}");
    }

    #[test]
    fn space_expands_the_call_under_the_cursor_and_collapses_it_again() {
        let mut app = with_calls(Size::new(120, 30));
        let collapsed = app.lines().len();
        app.apply(Action::NextCall { forward: true });

        app.apply(Action::ToggleCall);
        assert!(app.lines().len() > collapsed, "expanding added no lines");
        assert_eq!(call_lines(&app).first().map(|text| text.contains('▾')), Some(true));

        app.apply(Action::ToggleCall);
        assert_eq!(app.lines().len(), collapsed, "collapsing did not undo the expansion");
        assert_eq!(call_lines(&app).first().map(|text| text.contains('▸')), Some(true));
    }

    #[test]
    fn enter_expands_a_call_once_the_conversation_is_the_focused_column() {
        let mut app = with_calls(Size::new(120, 30));
        app.apply(Action::Focus { forward: true });
        app.apply(Action::Descend);
        assert_eq!(app.focused(), Column::Conversation, "the first Enter still descends");
        assert_eq!(app.call_cursor(), None);

        app.apply(Action::Descend);
        assert_eq!(app.call_cursor(), Some(0), "the next Enter reaches the first call");
        app.apply(Action::Descend);
        assert!(call_lines(&app).first().is_some_and(|text| text.contains('▾')), "and the one after expands it");
    }

    #[test]
    fn space_with_no_cursor_yet_selects_the_first_call_rather_than_expanding_nothing() {
        let mut app = with_calls(Size::new(120, 30));
        app.apply(Action::ToggleCall);
        assert_eq!(app.call_cursor(), Some(0));
        assert!(call_lines(&app).iter().all(|text| text.contains('▸')), "the first press only selects");
    }

    #[test]
    fn t_expands_every_call_and_the_next_press_collapses_every_call() {
        let mut app = with_calls(Size::new(120, 30));
        let collapsed = app.lines().len();

        app.apply(Action::ToggleAllCalls);
        assert!(call_lines(&app).iter().all(|text| text.contains('▾')), "{:?}", call_lines(&app));

        app.apply(Action::ToggleAllCalls);
        assert_eq!(app.lines().len(), collapsed);
        assert!(call_lines(&app).iter().all(|text| text.contains('▸')));
    }

    #[test]
    fn t_collapses_everything_when_only_some_calls_are_expanded() {
        let mut app = with_calls(Size::new(120, 30));
        app.apply(Action::NextCall { forward: true });
        app.apply(Action::ToggleCall);
        app.apply(Action::ToggleAllCalls);
        assert!(call_lines(&app).iter().all(|text| text.contains('▸')), "a fold, not three independent toggles");
    }

    #[test]
    fn the_selected_call_stays_on_the_same_screen_row_across_an_expansion() {
        let mut app = with_calls(Size::new(120, 12));
        for _ in 0..3 {
            app.apply(Action::NextCall { forward: true });
        }
        let before = app.cursor_line().expect("a cursor line").saturating_sub(app.pane(Column::Conversation).top);

        app.apply(Action::ToggleCall);
        let after = app.cursor_line().expect("a cursor line").saturating_sub(app.pane(Column::Conversation).top);
        assert_eq!(before, after, "the call under the cursor moved on screen when it expanded");
        assert_eq!(app.call_cursor(), Some(2), "and it is still the same call");
    }

    #[test]
    fn expanding_a_call_above_the_viewport_does_not_shift_the_one_being_read() {
        let mut app = with_calls(Size::new(120, 30));
        app.apply(Action::NextCall { forward: true });
        app.apply(Action::ToggleCall);
        let expanded = app.lines().len();
        app.apply(Action::NextCall { forward: true });
        app.apply(Action::ToggleCall);
        assert!(app.lines().len() > expanded, "the second expansion is independent of the first");
        assert_eq!(app.call_cursor(), Some(1), "the cursor followed the call, not the line index");
    }

    #[test]
    fn selecting_another_session_forgets_which_calls_were_expanded() {
        let mut app = with_calls(Size::new(120, 30));
        let Loadable::Ready(rendered) = app.conversation() else { panic!("a rendered conversation") };
        let path = rendered.path().to_path_buf();
        app.apply(Action::ToggleAllCalls);
        assert!(call_lines(&app).iter().all(|text| text.contains('▾')));

        app.set_projects(app.generation(), Ok(vec![project("a")]));
        app.set_sessions(app.generation(), vec![session("s1"), session("s2")]);
        assert_eq!(app.call_cursor(), None, "the cursor does not survive a session change");

        let conversation = crate::domain::thread::build(&path).expect("a built conversation");
        app.pending_conversation_path = Some(path);
        app.set_conversation(app.conversation_generation(), Ok(Box::new(conversation)), Agents::default());
        assert!(call_lines(&app).iter().all(|text| text.contains('▸')), "expansion is per session, not global");
    }

    #[test]
    fn an_overflowed_result_is_queued_for_a_worker_and_never_read_on_the_ui_thread() {
        let mut app = with_calls(Size::new(120, 30));
        app.apply(Action::ToggleAllCalls);
        assert!(app.take_tool_output().is_none(), "no call here carries a persistedOutputPath");
    }

    #[test]
    fn a_stale_tool_output_is_dropped_the_way_a_stale_conversation_is() {
        let mut app = with_calls(Size::new(120, 30));
        let stale = app.conversation_generation();
        app.apply(Action::NextCall { forward: true });
        app.apply(Action::ToggleCall);
        let before = app.lines().len();
        app.set_tool_output(stale.saturating_add(7), Box::from("t0"), Ok(vec!["nope".to_owned()]));
        app.reflow();
        assert_eq!(app.lines().len(), before, "a result from another session must not land");
    }

    #[test]
    fn quitting_sets_the_flag_and_nothing_else() {
        let mut app = app(Size::new(120, 30));
        app.apply(Action::Quit);
        assert!(app.quit);
    }

    #[test]
    fn moving_within_projects_reveals_the_selection_and_requests_sessions() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("a"), project("b"), project("c")]));
        let generation_after_load = app.generation();

        app.apply(Action::Move(Motion::Line(1)));

        assert_eq!(app.pane(Column::Projects).selected, 1);
        assert!(app.generation() > generation_after_load, "selecting a new project bumps the generation");
        let (dir, generation) = app.take_session_load().expect("a session load was requested");
        assert!(dir.ends_with("projects/b"));
        assert_eq!(generation, app.generation());
    }

    #[test]
    fn a_stale_generation_is_dropped() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("a")]));
        let stale = app.generation();
        app.apply(Action::Move(Motion::Line(1)));
        app.set_sessions(stale, vec![session("s1")]);
        assert!(matches!(app.sessions(), Loadable::Loading), "the stale result must not land");
    }

    #[test]
    fn focus_moves_right_then_left_but_never_past_either_end() {
        let mut app = app(Size::new(120, 30));
        assert_eq!(app.focused(), Column::Projects);
        app.apply(Action::Focus { forward: false });
        assert_eq!(app.focused(), Column::Projects, "no column left of Projects");

        app.apply(Action::Descend);
        assert_eq!(app.focused(), Column::Sessions);
        app.apply(Action::Descend);
        assert_eq!(app.focused(), Column::Conversation);
        app.apply(Action::Descend);
        assert_eq!(app.focused(), Column::Conversation, "no column right of Conversation");

        app.apply(Action::Ascend);
        assert_eq!(app.focused(), Column::Sessions);
    }

    #[test]
    fn focus_mode_goes_full_width_and_restores_the_previous_column_on_exit() {
        let mut app = app(Size::new(120, 30));
        app.apply(Action::Focus { forward: true });
        assert_eq!(app.focused(), Column::Sessions);

        app.apply(Action::ToggleFocusMode);
        assert_eq!(app.mode(), Mode::Focus);
        assert_eq!(app.focused(), Column::Conversation);

        app.apply(Action::ToggleFocusMode);
        assert_eq!(app.mode(), Mode::Browse);
        assert_eq!(app.focused(), Column::Sessions, "the previously focused column comes back");
    }

    #[test]
    fn escape_exits_focus_mode_instead_of_moving_focus() {
        let mut app = app(Size::new(120, 30));
        app.apply(Action::ToggleFocusMode);
        app.apply(Action::Ascend);
        assert_eq!(app.mode(), Mode::Browse);
    }

    #[test]
    fn an_initial_project_flag_preselects_the_matching_project() {
        let mut app = App::new(
            ctx(),
            &Options { claude_dir: PathBuf::from("/tmp"), project: Some("b".to_owned()), ..Options::default() },
            Size::new(120, 30),
        );
        app.set_projects(app.generation(), Ok(vec![project("a"), project("b")]));
        assert_eq!(app.pane(Column::Projects).selected, 1);
    }

    #[test]
    fn an_initial_session_flag_preselects_the_matching_session() {
        let mut app = App::new(
            ctx(),
            &Options { claude_dir: PathBuf::from("/tmp"), session: Some("s2".to_owned()), ..Options::default() },
            Size::new(120, 30),
        );
        app.set_projects(app.generation(), Ok(vec![project("a")]));
        let generation = app.generation();
        app.set_sessions(generation, vec![session("s1"), session("s2")]);
        assert_eq!(app.pane(Column::Sessions).selected, 1);
    }

    #[test]
    fn no_projects_at_all_settles_the_sessions_pane_as_ready_and_empty() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(Vec::new()));
        assert!(matches!(app.sessions(), Loadable::Ready(sessions) if sessions.is_empty()));
    }

    fn elapsed() -> Instant {
        Instant::now().checked_add(DEBOUNCE).unwrap_or_else(Instant::now)
    }

    fn conversation(text: &str) -> Conversation {
        use std::fs;

        use tempfile::TempDir;

        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("session.jsonl");
        let line = format!(
            r#"{{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:00Z","origin":{{"kind":"human"}},"message":{{"role":"user","content":"{text}"}}}}"#
        );
        fs::write(&path, line + "\n").expect("a written transcript");
        crate::domain::thread::build(&path).expect("a built conversation")
    }

    fn loaded(app: &mut App, text: &str) {
        let generation = app.conversation_generation();
        app.set_conversation(generation, Ok(Box::new(conversation(text))), Agents::default());
    }

    #[test]
    fn selecting_a_session_arms_a_conversation_load_on_its_own_generation() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("a")]));
        let (_dir, _generation) = app.take_session_load().expect("a session load was requested");
        let sessions_generation = app.generation();
        app.set_sessions(sessions_generation, vec![session("s1"), session("s2")]);

        let (path, generation) = app.take_conversation_load(elapsed()).expect("a conversation load was requested");
        assert!(path.ends_with("s1.jsonl"));
        assert_eq!(generation, app.conversation_generation());

        app.apply(Action::Focus { forward: true });
        app.apply(Action::Move(Motion::Line(1)));
        let (path, generation) = app.take_conversation_load(elapsed()).expect("moving in Sessions arms another load");
        assert!(path.ends_with("s2.jsonl"));
        assert_eq!(generation, app.conversation_generation());
    }

    #[test]
    fn a_conversation_load_never_invalidates_an_in_flight_session_list() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("a")]));
        let sessions_generation = app.generation();
        app.set_sessions(sessions_generation, vec![session("s1"), session("s2")]);
        app.apply(Action::Focus { forward: true });
        app.apply(Action::Move(Motion::Line(1)));

        assert_eq!(app.generation(), sessions_generation, "the sessions lane must not move with the conversation lane");
        assert!(matches!(app.sessions(), Loadable::Ready(sessions) if sessions.len() == 2));
    }

    #[test]
    fn a_stale_conversation_result_is_dropped() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("a")]));
        app.set_sessions(app.generation(), vec![session("s1"), session("s2")]);
        let stale = app.conversation_generation();
        app.apply(Action::Focus { forward: true });
        app.apply(Action::Move(Motion::Line(1)));

        app.set_conversation(stale, Ok(Box::new(conversation("stale"))), Agents::default());
        assert!(matches!(app.conversation(), Loadable::Loading), "the stale result must not land");
    }

    #[test]
    fn a_conversation_load_waits_out_the_debounce_before_it_fires() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("a")]));
        app.set_sessions(app.generation(), vec![session("s1")]);

        let armed = app.conversation_due().expect("the debounce was armed");
        assert!(app.take_conversation_load(Instant::now()).is_none(), "it fired before the deadline");
        assert!(app.take_conversation_load(armed).is_some(), "it never fired at the deadline");
        assert!(app.conversation_due().is_none(), "the deadline must be cleared, or it re-fires every frame");
        assert!(app.take_conversation_load(elapsed()).is_none(), "one arming is one load");
    }

    #[test]
    fn walking_through_the_session_list_arms_once_per_stop_and_loads_only_the_last() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("a")]));
        app.set_sessions(app.generation(), vec![session("s1"), session("s2"), session("s3")]);
        app.apply(Action::Focus { forward: true });

        app.apply(Action::Move(Motion::Line(1)));
        app.apply(Action::Move(Motion::Line(1)));

        let (path, _) = app.take_conversation_load(elapsed()).expect("the last stop loads");
        assert!(path.ends_with("s3.jsonl"), "{path:?}");
        assert!(app.take_conversation_load(elapsed()).is_none(), "the stops passed through queued nothing");
    }

    #[test]
    fn the_pane_clears_when_the_debounce_is_armed_rather_than_when_it_fires() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("a")]));
        app.set_sessions(app.generation(), vec![session("s1"), session("s2")]);
        loaded(&mut app, "the previous session");
        assert!(!app.lines().is_empty());

        app.apply(Action::Focus { forward: true });
        app.apply(Action::Move(Motion::Line(1)));

        assert!(matches!(app.conversation(), Loadable::Loading), "the old transcript is still on screen");
        assert!(app.lines().is_empty());
        assert_eq!(app.pane(Column::Conversation).top, 0, "the reading position did not reset");
    }

    #[test]
    fn nothing_selected_arms_no_deadline_so_the_loop_blocks_rather_than_spinning() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(Vec::new()));
        assert!(app.conversation_due().is_none());
        assert!(app.take_conversation_load(elapsed()).is_none());
    }

    #[test]
    fn the_conversation_column_scrolls_by_line_rather_than_selecting() {
        let mut app = app(Size::new(120, 10));
        app.set_projects(app.generation(), Ok(vec![project("a")]));
        app.set_sessions(app.generation(), vec![session("s1")]);
        loaded(&mut app, "one two three four five six seven eight nine ten");
        app.apply(Action::Focus { forward: true });
        app.apply(Action::Focus { forward: true });
        assert_eq!(app.focused(), Column::Conversation);

        app.apply(Action::Move(Motion::Line(1)));
        assert_eq!(app.pane(Column::Conversation).selected, 0, "the conversation has no line cursor");
        assert_eq!(app.pane(Column::Conversation).top, 0, "a short transcript does not scroll");

        app.apply(Action::Move(Motion::Bottom));
        assert_eq!(app.pane(Column::Conversation).top, 0);
    }

    #[test]
    fn a_transcript_taller_than_the_pane_scrolls_and_stops_at_the_last_line() {
        let mut app = app(Size::new(60, 10));
        app.set_projects(app.generation(), Ok(vec![project("a")]));
        app.set_sessions(app.generation(), vec![session("s1")]);
        loaded(&mut app, &"prose ".repeat(200));
        app.apply(Action::ToggleFocusMode);

        let last = app.last(Column::Conversation);
        let height = columns::conversation_height(app.area());
        assert!(last >= height, "the fixture must overflow the pane");

        app.apply(Action::Move(Motion::Bottom));
        let bottom = app.pane(Column::Conversation).top;
        assert_eq!(bottom, last.saturating_sub(height.saturating_sub(1)), "the last line lands on the last row");

        app.apply(Action::Move(Motion::Line(1)));
        assert_eq!(app.pane(Column::Conversation).top, bottom, "there is nothing below the last line");

        app.apply(Action::Move(Motion::Top));
        assert_eq!(app.pane(Column::Conversation).top, 0);
    }

    #[test]
    fn a_resize_rewraps_the_transcript_and_a_redundant_reflow_does_nothing() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("a")]));
        app.set_sessions(app.generation(), vec![session("s1")]);
        loaded(&mut app, &"prose ".repeat(60));
        let wide = app.lines().len();

        app.apply(Action::Resize(Size::new(40, 30)));
        app.reflow();
        let narrow = app.lines().len();
        assert!(narrow > wide, "{narrow} lines at 40 columns is not more than {wide} at 120");

        app.reflow();
        assert_eq!(app.lines().len(), narrow, "reflowing at an unchanged width must not re-wrap");
    }

    #[test]
    fn entering_focus_mode_rewraps_for_the_full_width() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("a")]));
        app.set_sessions(app.generation(), vec![session("s1")]);
        loaded(&mut app, &"prose ".repeat(60));
        app.reflow();
        let columnar = app.lines().len();

        app.apply(Action::ToggleFocusMode);
        app.reflow();
        assert!(app.lines().len() < columnar, "focus mode is wider, so it should need fewer lines");
    }

    #[test]
    fn resizing_updates_the_area() {
        let mut app = app(Size::new(120, 30));
        app.apply(Action::Resize(Size::new(80, 24)));
        assert_eq!(app.area(), Size::new(80, 24));
    }
}
