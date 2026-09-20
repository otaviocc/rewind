//! The shell's state, and the reducer that is the only way to change it.

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::layout::Size;

use crate::ctx::Ctx;
use crate::domain::cancel::{Cancel, Gate};
use crate::domain::diagnostics::Diagnostics;
use crate::domain::project::{Project, ProjectError};
use crate::domain::search::{self, corpus::Corpus, engine::Hit, resolve::Opened};
use crate::domain::session::Session;
use crate::domain::subagent::{Agent, Agents};
use crate::domain::thread::{Conversation, NodeId, NodeKind, ThreadError};
use crate::domain::tool;
use crate::render::export;
use crate::render::line::{RenderedLine, split_at_width};
use crate::render::message::{self, Anchor, Position, Transcript};
use crate::render::{Branches, Ctx as RenderCtx, Expanded, Outputs, Overflow};
use crate::theme::Theme;
use crate::ui::input::{Action, CopyTarget, Motion};
use crate::ui::{Options, columns, diagnostics, listing, search as ui_search};

pub const CHROME_ROWS: u16 = 5;
pub const DEBOUNCE: Duration = Duration::from_millis(80);

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
    conversation: Arc<Conversation>,
    path: PathBuf,
    agents: Agents,
    root: Option<NodeId>,
    transcript: Transcript,
    wrapped_at: u16,
    revision: u64,
}

impl Rendered {
    fn new(conversation: Arc<Conversation>, path: PathBuf, agents: Agents, width: u16, view: &View, theme: &Theme) -> Self {
        let transcript = message::transcript(&conversation, &view.ctx(usize::from(width), &agents, None, theme));
        Self { conversation, path, agents, root: None, transcript, wrapped_at: width, revision: view.revision }
    }

    fn rooted(&self, root: NodeId, width: u16, view: &View, theme: &Theme) -> Self {
        let mut rendered = Self {
            conversation: Arc::clone(&self.conversation),
            path: self.path.clone(),
            agents: self.agents.clone(),
            root: Some(root),
            transcript: Transcript::default(),
            wrapped_at: width,
            revision: view.revision,
        };
        rendered.rewrap(width, view, theme);
        rendered
    }

    fn rewrap(&mut self, width: u16, view: &View, theme: &Theme) {
        self.transcript = message::transcript(&self.conversation, &view.ctx(usize::from(width), &self.agents, self.root, theme));
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

    pub fn conversation(&self) -> &Conversation {
        &self.conversation
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub const fn agents(&self) -> &Agents {
        &self.agents
    }

    pub const fn root(&self) -> Option<NodeId> {
        self.root
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
    const fn ctx<'a>(&'a self, width: usize, agents: &'a Agents, root: Option<NodeId>, theme: &'a Theme) -> RenderCtx<'a> {
        RenderCtx {
            width,
            theme,
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

#[allow(clippy::struct_excessive_bools)]
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
    generation: Gate,
    conversation_generation: Gate,
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
    theme: Theme,
    cache_root: Option<PathBuf>,
    no_cache: bool,
    scan_requested: bool,
    scan_gate: Gate,
    scan_progress: Option<(usize, usize)>,
    search_open: bool,
    search_query: String,
    search_results: Vec<Hit>,
    search_pane: Pane,
    corpus: Option<Corpus>,
    corpus_requested: bool,
    corpus_wanted: bool,
    search_hit_generation: Gate,
    pending_hit_resolve: Option<(Hit, u64)>,
    pending_scroll_uuid: Option<String>,
    pending_hit_agent: Option<String>,
    filter: Option<Filter>,
    pending_copy: Option<String>,
    notice: Option<String>,
    export_prompt: Option<ExportPrompt>,
    pending_export: Option<(PathBuf, ExportPayload, bool)>,
    drag: Option<Drag>,
}

#[derive(Debug, Clone)]
struct Filter {
    column: Column,
    query: String,
    editing: bool,
    visible: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    Jsonl,
}

impl ExportFormat {
    const fn extension(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Jsonl => "jsonl",
        }
    }

    const fn toggled(self) -> Self {
        match self {
            Self::Markdown => Self::Jsonl,
            Self::Jsonl => Self::Markdown,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ExportPayload {
    Text(String),
    CopyFile(PathBuf),
}

#[derive(Debug, Clone)]
struct ExportPrompt {
    path: String,
    format: ExportFormat,
    confirm: bool,
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    anchor: usize,
    cursor: usize,
    moved: bool,
}

impl Drag {
    const fn range(&self) -> std::ops::RangeInclusive<usize> {
        if self.anchor <= self.cursor { self.anchor..=self.cursor } else { self.cursor..=self.anchor }
    }
}

impl App {
    pub fn new(ctx: Ctx, options: &Options, area: Size) -> Self {
        Self {
            ctx,
            quit: false,
            claude_dir: options.claude_dir.clone(),
            theme: options.theme.clone(),
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
            generation: Gate::default(),
            conversation_generation: Gate::default(),
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
            cache_root: options.cache_root.clone(),
            no_cache: options.no_cache,
            scan_requested: false,
            scan_gate: Gate::default(),
            scan_progress: None,
            search_open: false,
            search_query: String::new(),
            search_results: Vec::new(),
            search_pane: Pane::default(),
            corpus: None,
            corpus_requested: false,
            corpus_wanted: false,
            search_hit_generation: Gate::default(),
            pending_hit_resolve: None,
            pending_scroll_uuid: None,
            pending_hit_agent: None,
            filter: None,
            pending_copy: None,
            notice: None,
            export_prompt: None,
            pending_export: None,
            drag: None,
        }
    }

    pub fn claude_dir(&self) -> &Path {
        &self.claude_dir
    }

    pub const fn theme(&self) -> &Theme {
        &self.theme
    }

    pub fn generation(&self) -> u64 {
        self.generation.current()
    }

    pub fn project_cancel(&self, generation: u64) -> Cancel {
        self.generation.token_at(generation)
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

    pub fn conversation_generation(&self) -> u64 {
        self.conversation_generation.current()
    }

    pub fn conversation_cancel(&self, generation: u64) -> Cancel {
        self.conversation_generation.token_at(generation)
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

    pub fn cursor_rows(&self) -> Option<std::ops::Range<usize>> {
        let cursor = self.call_cursor?;
        let anchor = self.anchors().get(cursor)?;
        Some(anchor.line..anchor.line.saturating_add(anchor.head.max(1)))
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
        let index = self.resolve_position(Column::Projects, self.projects_pane.selected)?;
        projects.get(index)
    }

    pub fn selected_session(&self) -> Option<&Session> {
        let Loadable::Ready(sessions) = &self.sessions else { return None };
        let index = self.resolve_position(Column::Sessions, self.sessions_pane.selected)?;
        sessions.get(index)
    }

    pub fn resolve_position(&self, column: Column, position: usize) -> Option<usize> {
        self.filter
            .as_ref()
            .filter(|filter| filter.column == column)
            .map_or(Some(position), |filter| filter.visible.get(position).copied())
    }

    pub fn visible_len(&self, column: Column) -> usize {
        if let Some(filter) = self.filter.as_ref().filter(|filter| filter.column == column) {
            return filter.visible.len();
        }
        match column {
            Column::Projects => match &self.projects {
                Loadable::Ready(projects) => projects.len(),
                Loadable::Loading | Loadable::Failed(_) => 0,
            },
            Column::Sessions => match &self.sessions {
                Loadable::Ready(sessions) => sessions.len(),
                Loadable::Loading | Loadable::Failed(_) => 0,
            },
            Column::Conversation => self.lines().len(),
        }
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

    pub const fn take_copy(&mut self) -> Option<String> {
        self.pending_copy.take()
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn copy_failed(&mut self) {
        self.notice = Some("clipboard unavailable".to_owned());
    }

    pub fn set_subagent(&mut self, generation: u64, result: Result<Arc<Conversation>, ThreadError>, path: PathBuf) {
        if generation != self.conversation_generation.current() {
            return;
        }
        let width = columns::conversation_width(self.area, self.mode);
        let agents = self.inherited_agents();
        self.conversation = match result {
            Ok(conversation) => {
                self.record_drift(&conversation);
                Loadable::Ready(Rendered::new(conversation, path, agents, width, &self.view, &self.theme))
            }
            Err(error) => Loadable::Failed(error.to_string()),
        };
        if let Some(uuid) = self.pending_scroll_uuid.take() {
            self.scroll_to_uuid(&uuid);
        }
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
        if generation != self.conversation_generation.current() {
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

    pub fn set_conversation(&mut self, generation: u64, result: Result<Arc<Conversation>, ThreadError>, agents: Agents) {
        if generation != self.conversation_generation.current() {
            return;
        }
        let width = columns::conversation_width(self.area, self.mode);
        let path = self.pending_conversation_path.take().unwrap_or_default();
        self.conversation = match result {
            Ok(conversation) => {
                self.record_drift(&conversation);
                Loadable::Ready(Rendered::new(conversation, path, agents, width, &self.view, &self.theme))
            }
            Err(error) => Loadable::Failed(error.to_string()),
        };
        if let Some(agent_id) = self.pending_hit_agent.take()
            && self.enter_subagent_by_id(&agent_id)
        {
            return;
        }
        if let Some(uuid) = self.pending_scroll_uuid.take() {
            self.scroll_to_uuid(&uuid);
        }
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
        if generation != self.generation.current() {
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
        if matches!(self.projects, Loadable::Ready(_)) && !self.no_cache && self.cache_root.is_some() {
            self.scan_gate.bump();
            self.scan_requested = true;
        }
    }

    pub fn take_scan(&mut self) -> Option<(PathBuf, PathBuf, Option<String>, Cancel)> {
        if !self.scan_requested {
            return None;
        }
        self.scan_requested = false;
        let cache_root = self.cache_root.clone()?;
        let selected = self.selected_project().map(|project| project.directory.clone());
        Some((self.claude_dir.clone(), cache_root, selected, self.scan_gate.token()))
    }

    pub const fn set_scan_progress(&mut self, done: usize, total: usize) {
        self.scan_progress = Some((done, total));
    }

    pub const fn scan_finished(&mut self) {
        self.scan_progress = None;
    }

    pub const fn scan_status(&self) -> Option<(usize, usize)> {
        self.scan_progress
    }

    pub fn set_sessions(&mut self, generation: u64, sessions: Vec<Session>) {
        if generation != self.generation.current() {
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
        if self.filter.as_ref().is_some_and(|filter| filter.column == Column::Sessions) {
            self.recompute_filter(None);
        }
        self.request_conversation_for_selection();
    }

    pub const fn diagnostics_open(&self) -> bool {
        self.diagnostics_open
    }

    pub const fn diagnostics_top(&self) -> usize {
        self.diagnostics_pane.top
    }

    pub fn diagnostics_lines(&self) -> Vec<RenderedLine> {
        diagnostics::lines(&self.drift, usize::from(diagnostics::inner(self.diagnostics_area()).width), &self.theme)
    }

    const fn diagnostics_area(&self) -> Size {
        Size::new(self.area.width, self.area.height.saturating_sub(CHROME_ROWS))
    }

    fn quit(&mut self) {
        self.quit = true;
        self.scan_gate.bump();
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
        self.notice = None;
        if self.export_prompt.is_some() {
            self.apply_export(action);
            return;
        }
        if self.search_open {
            self.apply_search(action);
            return;
        }
        if self.diagnostics_open {
            match action {
                Action::Quit => self.quit(),
                Action::Resize(size) => self.area = size,
                Action::ToggleDiagnostics | Action::Ascend => self.toggle_diagnostics(),
                Action::Move(motion) => self.scroll_diagnostics(motion),
                Action::Focus { .. }
                | Action::Descend
                | Action::ToggleFocusMode
                | Action::NextCall { .. }
                | Action::NextTurn { .. }
                | Action::ToggleCall
                | Action::ToggleAllCalls
                | Action::CycleBranch
                | Action::ToggleInjections
                | Action::ToggleSearch
                | Action::ToggleFilter
                | Action::Copy(_)
                | Action::ToggleExport
                | Action::CycleExportFormat
                | Action::Type(_)
                | Action::Untype
                | Action::Scroll { .. }
                | Action::Click { .. }
                | Action::Drag { .. }
                | Action::Release => {}
            }
            return;
        }
        if let Some(filter) = &self.filter {
            if filter.editing {
                self.apply_filter_editing(action);
                return;
            }
            if filter.column == self.focused {
                match action {
                    Action::NextCall { forward } => {
                        self.jump_filter(forward);
                        return;
                    }
                    Action::Ascend => {
                        self.clear_filter();
                        return;
                    }
                    _ => {}
                }
            }
        }
        match action {
            Action::Quit => self.quit(),
            Action::Resize(size) => self.area = size,
            Action::ToggleFocusMode => self.toggle_focus_mode(),
            Action::Focus { forward } => self.move_focus(forward),
            Action::Descend => self.descend(),
            Action::Ascend => self.ascend(),
            Action::Move(motion) => self.move_selection(motion),
            Action::NextCall { forward } => self.move_call_cursor(forward),
            Action::NextTurn { forward } => self.jump_turn(forward),
            Action::ToggleCall => self.toggle_call(),
            Action::ToggleAllCalls => self.toggle_all_calls(),
            Action::CycleBranch => self.cycle_branch(),
            Action::ToggleInjections => self.toggle_injections(),
            Action::ToggleDiagnostics => self.toggle_diagnostics(),
            Action::ToggleSearch => self.toggle_search(),
            Action::ToggleFilter => self.toggle_filter(),
            Action::Copy(target) => self.copy(target),
            Action::ToggleExport => self.toggle_export(),
            Action::CycleExportFormat | Action::Type(_) | Action::Untype => {}
            Action::Scroll { column, delta } => self.scroll_column(column, delta),
            Action::Click { column, row } => self.click(column, row),
            Action::Drag { column, row } => self.drag(column, row),
            Action::Release => self.release(),
        }
    }

    fn apply_search(&mut self, action: Action) {
        match action {
            Action::Type(character) => {
                self.search_query.push(character);
                self.run_search();
            }
            Action::Untype => {
                self.search_query.pop();
                self.run_search();
            }
            Action::Move(motion) => {
                let last = self.search_results.len().saturating_sub(1);
                let height = self.pane_height();
                self.search_pane.selected = listing::target(motion, self.search_pane.selected, last, height);
                self.search_pane.top = listing::revealed(self.search_pane.top, self.search_pane.selected, height);
            }
            Action::Descend => self.open_selected_hit(),
            Action::Ascend => self.toggle_search(),
            Action::Resize(size) => self.area = size,
            Action::Quit => self.quit(),
            Action::Focus { .. }
            | Action::ToggleFocusMode
            | Action::NextCall { .. }
            | Action::NextTurn { .. }
            | Action::ToggleCall
            | Action::ToggleAllCalls
            | Action::CycleBranch
            | Action::ToggleInjections
            | Action::ToggleDiagnostics
            | Action::ToggleSearch
            | Action::ToggleFilter
            | Action::Copy(_)
            | Action::ToggleExport
            | Action::CycleExportFormat
            | Action::Scroll { .. }
            | Action::Click { .. }
            | Action::Drag { .. }
            | Action::Release => {}
        }
    }

    fn apply_filter_editing(&mut self, action: Action) {
        match action {
            Action::Type(character) => self.edit_filter_query(|query| query.push(character)),
            Action::Untype => self.edit_filter_query(|query| {
                query.pop();
            }),
            Action::Descend => {
                if let Some(filter) = &mut self.filter {
                    filter.editing = false;
                }
            }
            Action::Ascend => self.clear_filter(),
            Action::Move(motion) => self.move_selection(motion),
            Action::Resize(size) => self.area = size,
            Action::Quit => self.quit(),
            Action::Focus { .. }
            | Action::ToggleFocusMode
            | Action::NextCall { .. }
            | Action::NextTurn { .. }
            | Action::ToggleCall
            | Action::ToggleAllCalls
            | Action::CycleBranch
            | Action::ToggleInjections
            | Action::ToggleDiagnostics
            | Action::ToggleSearch
            | Action::ToggleFilter
            | Action::Copy(_)
            | Action::ToggleExport
            | Action::CycleExportFormat
            | Action::Scroll { .. }
            | Action::Click { .. }
            | Action::Drag { .. }
            | Action::Release => {}
        }
    }

    fn copy(&mut self, target: CopyTarget) {
        let text = match target {
            CopyTarget::Message => self.copy_message(),
            CopyTarget::Session => self.copy_session(),
            CopyTarget::Resume => self.copy_resume(),
        };
        self.notice = Some(text.as_deref().map_or_else(|| "nothing to copy".to_owned(), |text| notice_for(target, text)));
        self.pending_copy = text;
    }

    fn render_ctx<'a>(&'a self, rendered: &'a Rendered) -> RenderCtx<'a> {
        let width = columns::conversation_width(self.area, self.mode);
        self.view.ctx(usize::from(width), rendered.agents(), rendered.root(), &self.theme)
    }

    fn copy_message(&self) -> Option<String> {
        let Loadable::Ready(rendered) = &self.conversation else { return None };
        let position = rendered.transcript().position(self.conversation_pane.top)?;
        export::message(rendered.conversation(), position.node, &self.render_ctx(rendered))
    }

    fn copy_session(&self) -> Option<String> {
        let Loadable::Ready(rendered) = &self.conversation else { return None };
        let text = export::session(rendered.conversation(), &self.render_ctx(rendered));
        (!text.is_empty()).then_some(text)
    }

    fn copy_resume(&self) -> Option<String> {
        let session = self.selected_session()?;
        let mut command = String::new();
        if let Some(path) = self.resume_cwd() {
            let _ = write!(command, "cd {} && ", shell_quote(path));
        }
        let _ = write!(command, "claude --resume {}", session.id);
        Some(command)
    }

    fn resume_cwd(&self) -> Option<&Path> {
        if let Some(project) = self.selected_project()
            && project.present
        {
            return Some(&project.path);
        }
        let Loadable::Ready(rendered) = &self.conversation else { return None };
        let cwd = rendered.conversation().cwd()?;
        let Loadable::Ready(projects) = &self.projects else { return None };
        projects.iter().find(|project| project.present && project.path == cwd).map(|project| project.path.as_path())
    }

    fn toggle_export(&mut self) {
        let (Loadable::Ready(_), Some(session)) = (&self.conversation, self.selected_session()) else {
            self.notice = Some("nothing to export".to_owned());
            return;
        };
        let format = ExportFormat::Markdown;
        self.export_prompt = Some(ExportPrompt { path: default_export_path(session, format), format, confirm: false });
    }

    fn apply_export(&mut self, action: Action) {
        match action {
            Action::Type(character) => self.export_type(character),
            Action::Untype => self.export_untype(),
            Action::CycleExportFormat => self.cycle_export_format(),
            Action::Descend => self.export_confirm_or_write(),
            Action::Ascend => self.export_back_or_close(),
            Action::Resize(size) => self.area = size,
            Action::Quit => self.quit(),
            Action::Focus { .. }
            | Action::ToggleFocusMode
            | Action::NextCall { .. }
            | Action::NextTurn { .. }
            | Action::ToggleCall
            | Action::ToggleAllCalls
            | Action::CycleBranch
            | Action::ToggleInjections
            | Action::ToggleDiagnostics
            | Action::ToggleSearch
            | Action::ToggleFilter
            | Action::ToggleExport
            | Action::Copy(_)
            | Action::Move(_)
            | Action::Scroll { .. }
            | Action::Click { .. }
            | Action::Drag { .. }
            | Action::Release => {}
        }
    }

    fn export_type(&mut self, character: char) {
        let confirm = self.export_prompt.as_ref().is_some_and(|prompt| prompt.confirm);
        if confirm {
            match character {
                'y' | 'Y' => self.export_write(true),
                'n' | 'N' => {
                    if let Some(prompt) = &mut self.export_prompt {
                        prompt.confirm = false;
                    }
                    self.notice = None;
                }
                _ => {}
            }
            return;
        }
        if let Some(prompt) = &mut self.export_prompt {
            prompt.path.push(character);
        }
    }

    fn export_untype(&mut self) {
        let Some(prompt) = &mut self.export_prompt else { return };
        if prompt.confirm {
            return;
        }
        prompt.path.pop();
    }

    fn cycle_export_format(&mut self) {
        let Some(prompt) = &mut self.export_prompt else { return };
        if prompt.confirm {
            return;
        }
        prompt.format = prompt.format.toggled();
        prompt.path = swap_extension(&prompt.path, prompt.format);
    }

    fn export_confirm_or_write(&mut self) {
        let confirm = self.export_prompt.as_ref().is_some_and(|prompt| prompt.confirm);
        self.export_write(confirm);
    }

    fn export_back_or_close(&mut self) {
        let Some(prompt) = &mut self.export_prompt else { return };
        if prompt.confirm {
            prompt.confirm = false;
            self.notice = None;
            return;
        }
        self.export_prompt = None;
    }

    fn export_write(&mut self, force: bool) {
        let Some(prompt) = &self.export_prompt else { return };
        let trimmed = prompt.path.trim();
        if trimmed.is_empty() {
            self.notice = Some("nothing to export".to_owned());
            return;
        }
        let Loadable::Ready(rendered) = &self.conversation else {
            self.export_prompt = None;
            self.notice = Some("nothing to export".to_owned());
            return;
        };
        let dest = PathBuf::from(trimmed);
        let payload = match prompt.format {
            ExportFormat::Markdown => ExportPayload::Text(export::session(rendered.conversation(), &self.render_ctx(rendered))),
            ExportFormat::Jsonl => ExportPayload::CopyFile(rendered.path().to_path_buf()),
        };
        self.pending_export = Some((dest, payload, force));
    }

    pub const fn take_export(&mut self) -> Option<(PathBuf, ExportPayload, bool)> {
        self.pending_export.take()
    }

    pub fn export_written(&mut self, path: &Path) {
        self.export_prompt = None;
        self.notice = Some(format!("exported to {}", path.display()));
    }

    pub fn export_needs_confirmation(&mut self) {
        if let Some(prompt) = &mut self.export_prompt {
            prompt.confirm = true;
        }
        self.notice = Some("overwrite? y/n".to_owned());
    }

    pub fn export_failed(&mut self, error: &str) {
        self.notice = Some(format!("export failed: {error}"));
    }

    pub const fn export_prompt_open(&self) -> bool {
        self.export_prompt.is_some()
    }

    pub fn export_prompt_path(&self) -> Option<&str> {
        self.export_prompt.as_ref().map(|prompt| prompt.path.as_str())
    }

    pub fn export_prompt_format(&self) -> Option<ExportFormat> {
        self.export_prompt.as_ref().map(|prompt| prompt.format)
    }

    pub fn export_prompt_confirm(&self) -> bool {
        self.export_prompt.as_ref().is_some_and(|prompt| prompt.confirm)
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

    fn jump_turn(&mut self, forward: bool) {
        let top = self.conversation_pane.top;
        let line = {
            let Loadable::Ready(rendered) = &self.conversation else { return };
            let transcript = rendered.transcript();
            let found = if forward { transcript.turn_after(top) } else { transcript.turn_before(top) };
            let Some(line) = found else { return };
            line
        };
        self.conversation_pane.top = line.min(self.last(Column::Conversation));
    }

    fn reveal_call_cursor(&mut self) {
        let Some(rows) = self.cursor_rows() else { return };
        let height = columns::conversation_height(self.area);
        let last = rows.end.saturating_sub(1);
        self.conversation_pane.top = listing::revealed(self.conversation_pane.top, last, height);
        self.conversation_pane.top = listing::revealed(self.conversation_pane.top, rows.start, height);
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
        self.drag = None;
        let anchored = self.anchored();
        let width = columns::conversation_width(self.area, self.mode);
        if let Loadable::Ready(rendered) = &mut self.conversation {
            rendered.rewrap(width, &self.view, &self.theme);
        }
        self.restore(&anchored);
    }

    fn anchor_at(&mut self, id: &str, row: Option<usize>) {
        let anchored = Anchored { position: None, cursor: Some(Box::from(id)), row };
        let width = columns::conversation_width(self.area, self.mode);
        if let Loadable::Ready(rendered) = &mut self.conversation {
            rendered.rewrap(width, &self.view, &self.theme);
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
        self.pending_tool_output.push_back((Box::from(id), path, self.conversation_generation.current()));
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
        let child = rendered.rooted(root, width, &View::default(), &self.theme);

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
        let Some(id) = self.selected_agent().map(|agent| agent.id.clone()) else { return false };
        self.enter_subagent_by_id(&id)
    }

    fn enter_subagent_by_id(&mut self, agent_id: &str) -> bool {
        let Loadable::Ready(rendered) = &self.conversation else { return false };
        let Some((id, path, label)) = rendered
            .agents()
            .by_id(agent_id)
            .and_then(|agent| Some((agent.id.clone(), agent.transcript.clone()?, Box::<str>::from(agent.label()))))
        else {
            return false;
        };

        self.conversation_generation.bump();
        self.pending_tool_output.clear();
        self.stack.push(Frame {
            conversation: std::mem::replace(&mut self.conversation, Loadable::Loading),
            pane: self.conversation_pane,
            view: std::mem::take(&mut self.view),
            call_cursor: self.call_cursor.take(),
            label: self.label.replace(label),
        });
        self.conversation_pane = Pane::default();
        self.pending_subagent_load = Some((id, path, self.conversation_generation.current()));
        true
    }

    fn leave_subagent(&mut self) -> bool {
        let Some(frame) = self.stack.pop() else { return false };
        self.conversation_generation.bump();
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

    fn scroll_column(&mut self, column: Column, delta: isize) {
        if column == Column::Conversation {
            self.scroll_conversation(Motion::Line(delta));
            return;
        }
        let last = self.last(column);
        let height = self.pane_height();
        let pane = self.pane_mut(column);
        pane.top = listing::scrolled(pane.top, delta, last, height);
        let selected = listing::snapped(pane.selected, pane.top, last, height);
        if selected == pane.selected {
            return;
        }
        pane.selected = selected;
        match column {
            Column::Projects => self.request_sessions_for_selection(),
            Column::Sessions => self.request_conversation_for_selection(),
            Column::Conversation => {}
        }
    }

    fn click(&mut self, column: Column, row: u16) {
        self.focused = column;
        self.drag = (column == Column::Conversation)
            .then(|| self.conversation_pane.top.saturating_add(usize::from(row)))
            .map(|line| Drag { anchor: line, cursor: line, moved: false });
        match column {
            Column::Projects | Column::Sessions => self.click_list(column, row),
            Column::Conversation => self.click_conversation(row),
        }
    }

    fn drag(&mut self, column: Column, row: u16) {
        if column != Column::Conversation || self.drag.is_none() {
            return;
        }
        let line = self.conversation_pane.top.saturating_add(usize::from(row)).min(self.last(Column::Conversation));
        let Some(drag) = &mut self.drag else { return };
        if line != drag.cursor {
            drag.moved = true;
        }
        drag.cursor = line;
    }

    fn release(&mut self) {
        let Some(drag) = self.drag.take() else { return };
        if !drag.moved {
            return;
        }
        let Some(text) = self.selected_text(&drag) else { return };
        let lines = text.lines().count();
        let plural = if lines == 1 { "line" } else { "lines" };
        self.notice = Some(format!("copied selection · {lines} {plural}"));
        self.pending_copy = Some(text);
    }

    fn selected_text(&self, drag: &Drag) -> Option<String> {
        let lines = self.lines();
        let selected = lines.get(drag.range())?;
        let text = selected.iter().map(stripped_text).collect::<Vec<_>>().join("\n");
        (!text.trim().is_empty()).then_some(text)
    }

    pub fn selected_rows(&self) -> Option<std::ops::RangeInclusive<usize>> {
        self.drag.as_ref().filter(|drag| drag.moved).map(Drag::range)
    }

    fn click_list(&mut self, column: Column, row: u16) {
        let last = self.last(column);
        let pane = self.pane(column);
        let Some(index) = listing::picked(pane.top, usize::from(row), last) else { return };
        if index == pane.selected {
            return;
        }
        self.pane_mut(column).selected = index;
        match column {
            Column::Projects => self.request_sessions_for_selection(),
            Column::Sessions => self.request_conversation_for_selection(),
            Column::Conversation => {}
        }
    }

    fn click_conversation(&mut self, row: u16) {
        let line = self.conversation_pane.top.saturating_add(usize::from(row));
        let Some(cursor) = self
            .anchors()
            .iter()
            .position(|anchor| (anchor.line..anchor.line.saturating_add(anchor.head.max(1))).contains(&line))
        else {
            return;
        };
        self.call_cursor = Some(cursor);
        self.descend();
    }

    fn request_sessions_for_selection(&mut self) {
        self.generation.bump();
        self.sessions = Loadable::Loading;
        self.sessions_pane = Pane::default();
        if let Some(project) = self.selected_project() {
            let dir = self.claude_dir.join("projects").join(&project.directory);
            self.pending_session_load = Some((dir, self.generation.current()));
        } else {
            self.pending_session_load = None;
            self.sessions = Loadable::Ready(Vec::new());
        }
        self.request_conversation_for_selection();
    }

    fn request_conversation_for_selection(&mut self) {
        self.drag = None;
        self.stack.clear();
        self.label = None;
        self.pending_subagent_load = None;
        self.conversation_generation.bump();
        self.conversation = Loadable::Loading;
        self.conversation_pane = Pane::default();
        self.view = View::default();
        self.call_cursor = None;
        self.pending_tool_output.clear();
        self.pending_conversation_path = self.selected_session().map(|session| session.path.clone());
        self.pending_conversation_load =
            self.selected_session().map(|session| (session.path.clone(), self.conversation_generation.current()));
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
        self.visible_len(column).saturating_sub(1)
    }

    fn pane_height(&self) -> usize {
        usize::from(self.area.height.saturating_sub(CHROME_ROWS)).max(1)
    }

    pub const fn search_open(&self) -> bool {
        self.search_open
    }

    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    pub fn search_results(&self) -> &[Hit] {
        &self.search_results
    }

    pub const fn search_pane(&self) -> Pane {
        self.search_pane
    }

    pub const fn corpus_status(&self) -> ui_search::CorpusStatus {
        if self.corpus.is_none() {
            ui_search::CorpusStatus::Indexing
        } else if self.scan_progress.is_some() {
            ui_search::CorpusStatus::Partial
        } else {
            ui_search::CorpusStatus::Complete
        }
    }

    pub fn text_entry(&self) -> bool {
        self.search_open || self.filter.as_ref().is_some_and(|filter| filter.editing) || self.export_prompt.is_some()
    }

    pub fn filter_status(&self) -> Option<String> {
        let filter = self.filter.as_ref()?;
        if filter.column != self.focused {
            return None;
        }
        let hint = if filter.editing { "type to filter, Enter to lock" } else { "n/N to step, Esc to clear" };
        Some(format!("/{} · {hint}", filter.query))
    }

    fn toggle_search(&mut self) {
        self.search_open = !self.search_open;
        self.search_pane = Pane::default();
        if !self.search_open {
            self.search_query.clear();
            self.search_results.clear();
            return;
        }
        if !self.no_cache && self.cache_root.is_some() {
            self.corpus_wanted = true;
            if self.corpus.is_none() && !self.corpus_requested {
                self.corpus_requested = true;
            }
        }
    }

    fn toggle_filter(&mut self) {
        if let Some(filter) = &mut self.filter {
            filter.editing = true;
            return;
        }
        if matches!(self.focused, Column::Projects | Column::Sessions) {
            let column = self.focused;
            let previous = self.pane(column).selected;
            self.filter = Some(Filter { column, query: String::new(), editing: true, visible: Vec::new() });
            self.recompute_filter(Some(previous));
        }
    }

    fn run_search(&mut self) {
        self.search_pane = Pane::default();
        let Some(corpus) = &self.corpus else {
            self.search_results = Vec::new();
            return;
        };
        let query = search::query::parse(&self.search_query);
        self.search_results = search::engine::search(corpus, &query, now_ms());
    }

    fn open_selected_hit(&mut self) {
        let Some(hit) = self.search_results.get(self.search_pane.selected).cloned() else { return };
        self.search_hit_generation.bump();
        self.pending_hit_resolve = Some((hit, self.search_hit_generation.current()));
    }

    pub const fn take_hit_resolve(&mut self) -> Option<(Hit, u64)> {
        self.pending_hit_resolve.take()
    }

    pub fn set_corpus(&mut self, corpus: Arc<Corpus>) {
        let loaded = Arc::try_unwrap(corpus).unwrap_or_default();
        match self.corpus.as_mut() {
            Some(existing) => {
                for entry in loaded.shards {
                    if !existing.shards.iter().any(|shard| shard.directory == entry.directory) {
                        existing.shards.push(entry);
                    }
                }
            }
            None => self.corpus = Some(loaded),
        }
        if self.search_open {
            self.run_search();
        }
    }

    pub fn set_shard_ready(&mut self, generation: u64, entry: search::corpus::ShardEntry) {
        if !self.corpus_wanted || generation != self.scan_gate.current() {
            return;
        }
        self.corpus.get_or_insert_with(Corpus::default).upsert(entry);
        if self.search_open {
            self.run_search();
        }
    }

    pub fn take_corpus_load(&mut self) -> Option<PathBuf> {
        if !self.corpus_requested {
            return None;
        }
        self.corpus_requested = false;
        self.cache_root.clone()
    }

    pub fn set_hit_resolved(&mut self, generation: u64, target: Option<Opened>) {
        if generation != self.search_hit_generation.current() {
            return;
        }
        let Some(target) = target else { return };
        self.open_search_hit(target);
    }

    fn open_search_hit(&mut self, target: Opened) {
        self.search_open = false;
        self.filter = None;
        let Loadable::Ready(projects) = &self.projects else { return };
        let Some(index) = projects.iter().position(|project| project.directory == target.project_directory) else { return };
        self.projects_pane.selected = index;
        self.pending_session = Some(target.session_id);
        self.pending_scroll_uuid = target.uuid;
        self.pending_hit_agent = target.agent_id;
        self.focused = Column::Conversation;
        self.request_sessions_for_selection();
    }

    fn scroll_to_uuid(&mut self, uuid: &str) {
        let Loadable::Ready(rendered) = &self.conversation else { return };
        let Some(node) = rendered.conversation().id_of(uuid) else { return };
        let Some(line) = rendered.transcript().line_of(Position { node, offset: 0 }) else { return };
        self.conversation_pane.top = line.min(self.last(Column::Conversation));
    }

    fn jump_filter(&mut self, forward: bool) {
        let Some(filter) = &self.filter else { return };
        let column = filter.column;
        let len = filter.visible.len();
        if len == 0 {
            return;
        }
        let current = self.pane(column).selected;
        let next = if forward {
            current.saturating_add(1).checked_rem(len).unwrap_or(0)
        } else {
            current.checked_sub(1).unwrap_or_else(|| len.saturating_sub(1))
        };
        self.select_in_column(column, next);
    }

    fn select_in_column(&mut self, column: Column, position: usize) {
        self.pane_mut(column).selected = position;
        let height = self.pane_height();
        let pane = self.pane_mut(column);
        pane.top = listing::revealed(pane.top, pane.selected, height);
        match column {
            Column::Projects => self.request_sessions_for_selection(),
            Column::Sessions => self.request_conversation_for_selection(),
            Column::Conversation => {}
        }
    }

    fn edit_filter_query(&mut self, edit: impl FnOnce(&mut String)) {
        let Some(column) = self.filter.as_ref().map(|filter| filter.column) else { return };
        let previous = self.resolve_position(column, self.pane(column).selected);
        if let Some(filter) = &mut self.filter {
            edit(&mut filter.query);
        }
        self.recompute_filter(previous);
    }

    fn recompute_filter(&mut self, previous_underlying: Option<usize>) {
        let Some((column, query)) = self.filter.as_ref().map(|filter| (filter.column, filter.query.clone())) else { return };
        let labels = self.filter_labels(column);
        let scored: Vec<(usize, Option<i32>)> = labels
            .iter()
            .enumerate()
            .map(|(index, label)| {
                let score = if query.is_empty() { Some(0) } else { search::fuzzy::score(&query, label) };
                (index, score)
            })
            .collect();
        let visible: Vec<usize> = scored.iter().filter(|(_, score)| score.is_some()).map(|(index, _)| *index).collect();

        let best_scored = scored
            .iter()
            .filter(|(_, score)| score.is_some())
            .max_by_key(|(_, score)| score.unwrap_or(i32::MIN))
            .map(|(index, _)| *index);

        let new_position = previous_underlying
            .and_then(|underlying| visible.iter().position(|&index| index == underlying))
            .or_else(|| best_scored.and_then(|underlying| visible.iter().position(|&index| index == underlying)))
            .unwrap_or(0)
            .min(visible.len().saturating_sub(1));
        let new_underlying = visible.get(new_position).copied();
        let selection_changed = new_underlying != previous_underlying;

        if let Some(filter) = &mut self.filter {
            filter.visible = visible;
        }
        let height = self.pane_height();
        let pane = self.pane_mut(column);
        pane.selected = new_position;
        pane.top = listing::revealed(pane.top, pane.selected, height);
        if selection_changed {
            match column {
                Column::Projects => self.request_sessions_for_selection(),
                Column::Sessions => self.request_conversation_for_selection(),
                Column::Conversation => {}
            }
        }
    }

    fn clear_filter(&mut self) {
        let Some(filter) = self.filter.take() else { return };
        let column = filter.column;
        let previous_position = self.pane(column).selected;
        let Some(&underlying) = filter.visible.get(previous_position) else { return };
        let height = self.pane_height();
        let pane = self.pane_mut(column);
        pane.selected = underlying;
        pane.top = listing::revealed(0, underlying, height);
    }

    fn filter_labels(&self, column: Column) -> Vec<String> {
        match column {
            Column::Projects => match &self.projects {
                Loadable::Ready(projects) => projects.iter().map(|project| project.path.display().to_string()).collect(),
                _ => Vec::new(),
            },
            Column::Sessions => match &self.sessions {
                Loadable::Ready(sessions) => sessions.iter().map(|session| session.title.clone()).collect(),
                _ => Vec::new(),
            },
            Column::Conversation => Vec::new(),
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map_or(0, |duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
}

fn matches_project(project: &Project, wanted: &str) -> bool {
    project.directory == wanted
        || project.path.display().to_string() == wanted
        || project.path.file_name().is_some_and(|name| name == wanted)
}

fn notice_for(target: CopyTarget, text: &str) -> String {
    match target {
        CopyTarget::Message => "copied the message".to_owned(),
        CopyTarget::Session => {
            let lines = text.lines().count();
            let plural = if lines == 1 { "line" } else { "lines" };
            format!("copied the session · {lines} {plural}")
        }
        CopyTarget::Resume => "copied the resume command".to_owned(),
    }
}

fn shell_quote(path: &Path) -> String {
    let text = path.to_string_lossy();
    if text.chars().all(|character| character.is_ascii_alphanumeric() || matches!(character, '/' | '-' | '_' | '.')) {
        text.into_owned()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

fn default_export_path(session: &Session, format: ExportFormat) -> String {
    let id8: String = session.id.chars().take(8).collect();
    let slug = session.slug.as_deref().filter(|slug| !slug.is_empty()).unwrap_or("session");
    format!("./{slug}-{id8}.{}", format.extension())
}

fn swap_extension(path: &str, format: ExportFormat) -> String {
    let wanted = format.extension();
    let other = format.toggled().extension();
    path.strip_suffix(&format!(".{other}")).map_or_else(|| format!("{path}.{wanted}"), |base| format!("{base}.{wanted}"))
}

fn stripped_text(line: &RenderedLine) -> String {
    let text = line.text();
    let (_, rest) = split_at_width(&text, line.inset);
    rest.to_owned()
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
        app.set_conversation(app.conversation_generation(), Ok(Arc::new(conversation)), Agents::default());
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
        app.set_conversation(app.conversation_generation(), Ok(Arc::new(conversation)), agents);
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
        app.set_subagent(stale, Ok(Arc::new(conversation)), path);
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
        app.set_conversation(app.conversation_generation(), Ok(Arc::new(conversation)), agents);
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
        app.set_conversation(app.conversation_generation(), Ok(Arc::new(conversation)), Agents::default());
        let _ = dir.keep();
        app
    }

    fn exchanges(area: Size) -> App {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let path = dir.path().join("session.jsonl");
        let prose = "the deflector array reads back one plate at a time and then the next ".repeat(3);
        let mut lines = Vec::new();
        for index in 0..4u32 {
            let parent = if index == 0 { "null".to_owned() } else { format!(r#""a{}""#, index.saturating_sub(1)) };
            lines.push(format!(
                r#"{{"type":"user","uuid":"u{index}","parentUuid":{parent},"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:00Z","origin":{{"kind":"human"}},"message":{{"role":"user","content":"ask {index}"}}}}"#
            ));
            lines.push(format!(
                r#"{{"type":"assistant","uuid":"a{index}","parentUuid":"u{index}","sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:01Z","requestId":"q{index}","message":{{"model":"opus-5","id":"m{index}","role":"assistant","content":[{{"type":"text","text":"{prose}"}}]}}}}"#
            ));
        }
        std::fs::write(&path, lines.join("\n") + "\n").expect("a written transcript");
        let conversation = crate::domain::thread::build(&path).expect("a built conversation");
        let mut app = app(area);
        app.pending_conversation_path = Some(path);
        app.set_conversation(app.conversation_generation(), Ok(Arc::new(conversation)), Agents::default());
        let _ = dir.keep();
        app
    }

    fn turns_of(app: &App) -> Vec<usize> {
        let Loadable::Ready(rendered) = app.conversation() else { return Vec::new() };
        rendered.transcript().turns.clone()
    }

    #[test]
    fn a_step_forward_puts_the_next_header_at_the_top_of_the_conversation() {
        let mut app = exchanges(Size::new(120, 20));
        let turns = turns_of(&app);
        assert!(turns.len() >= 4, "the fixture has to have headers to walk: {turns:?}");
        assert_eq!(app.pane(Column::Conversation).top, 0);

        app.apply(Action::NextTurn { forward: true });
        assert_eq!(Some(app.pane(Column::Conversation).top), turns.get(1).copied());
        app.apply(Action::NextTurn { forward: true });
        assert_eq!(Some(app.pane(Column::Conversation).top), turns.get(2).copied());
    }

    #[test]
    fn a_step_back_returns_to_the_header_it_came_from() {
        let mut app = exchanges(Size::new(120, 20));
        app.apply(Action::NextTurn { forward: true });
        app.apply(Action::NextTurn { forward: true });
        let top = app.pane(Column::Conversation).top;
        app.apply(Action::NextTurn { forward: false });
        assert!(app.pane(Column::Conversation).top < top);
        app.apply(Action::NextTurn { forward: false });
        assert_eq!(app.pane(Column::Conversation).top, 0, "back to where the reading started");
    }

    #[test]
    fn a_step_from_inside_a_block_leaves_that_block_rather_than_landing_on_its_own_header() {
        let mut app = exchanges(Size::new(120, 20));
        app.apply(Action::Focus { forward: true });
        app.apply(Action::Focus { forward: true });
        app.apply(Action::Move(Motion::Line(1)));
        let turns = turns_of(&app);
        app.apply(Action::NextTurn { forward: true });
        assert_eq!(Some(app.pane(Column::Conversation).top), turns.get(1).copied());
    }

    #[test]
    fn a_step_past_either_end_of_the_conversation_moves_nothing() {
        let mut app = exchanges(Size::new(120, 20));
        app.apply(Action::NextTurn { forward: false });
        assert_eq!(app.pane(Column::Conversation).top, 0, "nothing above the first header");

        for _ in 0..turns_of(&app).len().saturating_add(2) {
            app.apply(Action::NextTurn { forward: true });
        }
        let bottom = app.pane(Column::Conversation).top;
        app.apply(Action::NextTurn { forward: true });
        assert_eq!(app.pane(Column::Conversation).top, bottom, "and nothing below the last");
        assert!(bottom <= app.lines().len().saturating_sub(1), "the pane never scrolls past its own last line");
    }

    #[test]
    fn a_run_of_replies_under_one_header_is_a_single_stop() {
        let mut app = wide_transcript(Size::new(120, 20));
        assert_eq!(turns_of(&app).len(), 2, "one you, one claude, though twelve records rendered");
        app.apply(Action::NextTurn { forward: true });
        let claude = app.pane(Column::Conversation).top;
        assert!(claude > 0);
        app.apply(Action::NextTurn { forward: true });
        assert_eq!(app.pane(Column::Conversation).top, claude, "the run is one block, not twelve");
    }

    #[test]
    fn a_conversation_that_never_loaded_has_nowhere_to_step() {
        let mut app = app(Size::new(120, 20));
        app.apply(Action::NextTurn { forward: true });
        app.apply(Action::NextTurn { forward: false });
        assert_eq!(app.pane(Column::Conversation).top, 0);
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
        app.set_conversation(app.conversation_generation(), Ok(Arc::new(conversation)), agents);
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
        assert!(text.contains("▸ Bash"), "{text:?}");
        let digest = app.lines().get(line.saturating_add(1)).map(RenderedLine::text).unwrap_or_default();
        assert!(digest.contains("step 0"), "{digest:?}");
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
        app.set_conversation(app.conversation_generation(), Ok(Arc::new(conversation)), Agents::default());
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
    fn scrolling_a_column_does_not_take_focus_away_from_another() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok((0..30).map(|index| project(&format!("p{index}"))).collect()));
        assert_eq!(app.focused(), Column::Projects);
        app.apply(Action::Focus { forward: true });
        assert_eq!(app.focused(), Column::Sessions);

        app.apply(Action::Scroll { column: Column::Projects, delta: 5 });
        assert_eq!(app.focused(), Column::Sessions, "the wheel does not steal focus");
        assert!(app.pane(Column::Projects).top > 0, "the column under the pointer still scrolled");
    }

    #[test]
    fn scrolling_past_the_selection_snaps_it_back_onto_the_visible_rows_and_requests_sessions() {
        let mut app = app(Size::new(120, 10));
        app.set_projects(app.generation(), Ok((0..30).map(|index| project(&format!("p{index}"))).collect()));
        let generation_after_load = app.generation();

        app.apply(Action::Scroll { column: Column::Projects, delta: 20 });

        assert!(app.pane(Column::Projects).selected > 0, "the selection followed the scroll onto the visible rows");
        assert!(app.generation() > generation_after_load, "the new selection requested sessions");
    }

    #[test]
    fn scrolling_without_crossing_the_selection_requests_nothing() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok((0..30).map(|index| project(&format!("p{index}"))).collect()));
        app.apply(Action::Click { column: Column::Projects, row: 15 });
        let generation_after_click = app.generation();

        app.apply(Action::Scroll { column: Column::Projects, delta: 1 });

        assert_eq!(app.generation(), generation_after_click, "the selection stayed on screen, so nothing reloaded");
    }

    #[test]
    fn a_click_selects_a_row_focuses_the_column_and_requests_sessions() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("a"), project("b"), project("c")]));
        assert_eq!(app.focused(), Column::Projects);
        let generation_after_load = app.generation();

        app.apply(Action::Click { column: Column::Projects, row: 1 });

        assert_eq!(app.focused(), Column::Projects);
        assert_eq!(app.pane(Column::Projects).selected, 1);
        assert!(app.generation() > generation_after_load, "clicking a new row requests sessions");
        let (dir, _) = app.take_session_load().expect("a session load was requested");
        assert!(dir.ends_with("projects/b"));
    }

    #[test]
    fn clicking_the_column_already_focused_and_selected_moves_the_focus_but_reloads_nothing() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("a"), project("b")]));
        app.apply(Action::Move(Motion::Line(1)));
        let generation_after_move = app.generation();

        app.apply(Action::Click { column: Column::Projects, row: 1 });

        assert_eq!(app.pane(Column::Projects).selected, 1);
        assert_eq!(app.generation(), generation_after_move, "re-clicking the same row did not reload");
    }

    #[test]
    fn a_click_past_the_end_of_a_short_list_selects_nothing() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("a")]));
        let generation_after_load = app.generation();

        app.apply(Action::Click { column: Column::Projects, row: 10 });

        assert_eq!(app.pane(Column::Projects).selected, 0);
        assert_eq!(app.generation(), generation_after_load);
    }

    #[test]
    fn clicking_a_calls_header_row_expands_it_just_like_space() {
        let mut app = with_calls(Size::new(120, 30));
        app.apply(Action::Focus { forward: true });
        app.apply(Action::Focus { forward: true });
        let row =
            u16::try_from(app.anchors().first().map(|anchor| anchor.line).expect("at least one call")).expect("a small row");

        app.apply(Action::Click { column: Column::Conversation, row });

        assert_eq!(app.call_cursor(), Some(0));
        assert!(call_lines(&app).first().is_some_and(|text| text.contains('▾')), "the click expanded the call");
    }

    #[test]
    fn clicking_a_subagent_row_enters_it_just_like_enter() {
        let mut app = with_subagent(Size::new(120, 30));
        enter_first_subagent(&mut app);
        let cursor = app.call_cursor().expect("a subagent call is selected");
        let line = app.anchors().get(cursor).map(|anchor| anchor.line).expect("the anchor exists");
        app.call_cursor = None;
        let top = app.pane(Column::Conversation).top;
        let row = u16::try_from(line.saturating_sub(top)).expect("a small row");

        app.apply(Action::Click { column: Column::Conversation, row });

        assert_eq!(app.depth(), 1, "the click descended into the subagent");
    }

    #[test]
    fn a_click_on_a_body_row_of_an_expanded_call_only_focuses_the_column() {
        let mut app = with_calls(Size::new(120, 30));
        app.apply(Action::Focus { forward: true });
        app.apply(Action::Focus { forward: true });
        app.apply(Action::ToggleCall);
        let cursor = app.call_cursor();

        app.apply(Action::Click { column: Column::Conversation, row: 1 });

        assert_eq!(app.call_cursor(), cursor, "a body row is not a header row, so the cursor did not move");
    }

    #[test]
    fn the_mouse_does_nothing_while_the_diagnostics_overlay_is_open() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("a"), project("b")]));
        app.apply(Action::ToggleDiagnostics);
        assert!(app.diagnostics_open());

        app.apply(Action::Click { column: Column::Projects, row: 1 });
        assert_eq!(app.pane(Column::Projects).selected, 0, "the click did not reach the list underneath");

        app.apply(Action::Scroll { column: Column::Projects, delta: 1 });
        assert_eq!(app.pane(Column::Projects).top, 0, "the wheel did not reach the list underneath");
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
        app.set_conversation(generation, Ok(Arc::new(conversation(text))), Agents::default());
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

        app.set_conversation(stale, Ok(Arc::new(conversation("stale"))), Agents::default());
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
    fn holding_a_motion_key_through_thirty_stops_still_arms_only_the_last_load() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("a")]));
        app.set_sessions(app.generation(), (0..31).map(|index| session(&format!("s{index}"))).collect());
        app.apply(Action::Focus { forward: true });

        for _ in 0..30 {
            app.apply(Action::Move(Motion::Line(1)));
        }

        let (path, _) = app.take_conversation_load(elapsed()).expect("thirty stops still arm exactly one load");
        assert!(path.ends_with("s30.jsonl"), "{path:?}");
        assert!(app.take_conversation_load(elapsed()).is_none(), "one arming is one load, however many stops preceded it");
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

    #[test]
    fn no_scan_is_requested_without_a_cache_root() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("a")]));
        assert!(app.take_scan().is_none(), "there is nowhere to build the corpus without a cache root");
    }

    #[test]
    fn no_scan_is_requested_with_no_cache() {
        let mut app = App::new(
            ctx(),
            &Options {
                claude_dir: PathBuf::from("/tmp"),
                cache_root: Some(PathBuf::from("/tmp/cache")),
                no_cache: true,
                ..Options::default()
            },
            Size::new(120, 30),
        );
        app.set_projects(app.generation(), Ok(vec![project("a")]));
        assert!(app.take_scan().is_none(), "--no-cache must skip the pool entirely");
    }

    #[test]
    fn a_scan_is_requested_once_the_projects_load_and_only_once() {
        let mut app = App::new(
            ctx(),
            &Options { claude_dir: PathBuf::from("/tmp"), cache_root: Some(PathBuf::from("/tmp/cache")), ..Options::default() },
            Size::new(120, 30),
        );
        app.set_projects(app.generation(), Ok(vec![project("a"), project("b")]));

        let (claude_dir, cache_root, selected, _cancel) = app.take_scan().expect("a scan is requested");
        assert_eq!(claude_dir, PathBuf::from("/tmp"));
        assert_eq!(cache_root, PathBuf::from("/tmp/cache"));
        assert_eq!(selected.as_deref(), Some("a"), "the selected project leads the scan queue");
        assert!(app.take_scan().is_none(), "one projects load arms exactly one scan");
    }

    #[test]
    fn scan_progress_reports_until_finish_clears_it() {
        let mut app = app(Size::new(120, 30));
        assert!(app.scan_status().is_none());
        app.set_scan_progress(3, 10);
        assert_eq!(app.scan_status(), Some((3, 10)));
        app.scan_finished();
        assert!(app.scan_status().is_none());
    }

    #[test]
    fn no_cache_never_requests_the_search_corpus_either() {
        let mut app = App::new(
            ctx(),
            &Options {
                claude_dir: PathBuf::from("/tmp"),
                cache_root: Some(PathBuf::from("/tmp/cache")),
                no_cache: true,
                ..Options::default()
            },
            Size::new(120, 30),
        );
        app.apply(Action::ToggleSearch);
        assert!(app.search_open());
        assert!(app.take_corpus_load().is_none(), "--no-cache must never load a possibly-stale corpus off disk");
    }

    #[test]
    fn toggling_search_opens_the_overlay_and_requests_the_corpus_once() {
        let mut app = App::new(
            ctx(),
            &Options { claude_dir: PathBuf::from("/tmp"), cache_root: Some(PathBuf::from("/tmp/cache")), ..Options::default() },
            Size::new(120, 30),
        );
        assert!(!app.search_open());
        assert!(!app.text_entry());

        app.apply(Action::ToggleSearch);
        assert!(app.search_open());
        assert!(app.text_entry());
        assert_eq!(app.take_corpus_load(), Some(PathBuf::from("/tmp/cache")));
        assert!(app.take_corpus_load().is_none(), "the corpus load is requested once, not on every poll");
    }

    fn cached_app() -> App {
        App::new(
            ctx(),
            &Options { claude_dir: PathBuf::from("/tmp"), cache_root: Some(PathBuf::from("/tmp/cache")), ..Options::default() },
            Size::new(120, 30),
        )
    }

    #[test]
    fn a_shard_ready_from_a_superseded_scan_is_dropped() {
        let mut app = cached_app();
        app.apply(Action::ToggleSearch);
        let stale = app.scan_gate.current();
        app.scan_gate.bump();

        app.set_shard_ready(
            stale,
            search::corpus::ShardEntry {
                directory: Some("-a".to_owned()),
                weight: search::corpus::NORMAL_WEIGHT,
                bytes: vec![1],
            },
        );

        assert!(app.corpus.is_none(), "a shard from a superseded scan must not be merged into the corpus");
    }

    #[test]
    fn a_shard_ready_before_search_has_ever_been_opened_is_dropped_to_stay_lazy() {
        let mut app = cached_app();
        let generation = app.scan_gate.current();

        app.set_shard_ready(
            generation,
            search::corpus::ShardEntry {
                directory: Some("-a".to_owned()),
                weight: search::corpus::NORMAL_WEIGHT,
                bytes: vec![1],
            },
        );

        assert!(app.corpus.is_none(), "nothing has asked for the corpus yet, so streamed bytes must not accumulate");
    }

    #[test]
    fn a_second_shard_ready_for_the_same_directory_replaces_rather_than_duplicates() {
        let mut app = cached_app();
        app.apply(Action::ToggleSearch);
        let generation = app.scan_gate.current();

        app.set_shard_ready(
            generation,
            search::corpus::ShardEntry {
                directory: Some("-a".to_owned()),
                weight: search::corpus::NORMAL_WEIGHT,
                bytes: vec![1],
            },
        );
        app.set_shard_ready(
            generation,
            search::corpus::ShardEntry {
                directory: Some("-a".to_owned()),
                weight: search::corpus::NORMAL_WEIGHT,
                bytes: vec![2],
            },
        );

        assert_eq!(app.corpus.as_ref().map(|corpus| corpus.shards.len()), Some(1));
    }

    #[test]
    fn the_corpus_status_is_indexing_then_partial_while_scanning_then_complete_once_finished() {
        let mut app = cached_app();
        app.apply(Action::ToggleSearch);
        assert_eq!(app.corpus_status(), ui_search::CorpusStatus::Indexing);

        let generation = app.scan_gate.current();
        app.set_scan_progress(1, 3);
        app.set_shard_ready(
            generation,
            search::corpus::ShardEntry {
                directory: Some("-a".to_owned()),
                weight: search::corpus::NORMAL_WEIGHT,
                bytes: vec![1],
            },
        );
        assert_eq!(app.corpus_status(), ui_search::CorpusStatus::Partial);

        app.scan_finished();
        assert_eq!(app.corpus_status(), ui_search::CorpusStatus::Complete);
    }

    #[test]
    fn a_disk_load_arriving_after_a_streamed_shard_does_not_clobber_it() {
        let mut app = cached_app();
        app.apply(Action::ToggleSearch);
        let generation = app.scan_gate.current();
        app.set_shard_ready(
            generation,
            search::corpus::ShardEntry {
                directory: Some("-a".to_owned()),
                weight: search::corpus::NORMAL_WEIGHT,
                bytes: vec![9],
            },
        );

        let stale_disk_snapshot = search::corpus::Corpus {
            shards: vec![
                search::corpus::ShardEntry {
                    directory: Some("-a".to_owned()),
                    weight: search::corpus::NORMAL_WEIGHT,
                    bytes: vec![0],
                },
                search::corpus::ShardEntry {
                    directory: Some("-b".to_owned()),
                    weight: search::corpus::NORMAL_WEIGHT,
                    bytes: vec![7],
                },
            ],
        };
        app.set_corpus(Arc::new(stale_disk_snapshot));

        let corpus = app.corpus.as_ref().expect("a corpus after both a streamed shard and a disk load");
        assert_eq!(corpus.shards.len(), 2, "the disk load fills in -b but must not duplicate -a");
        let a = corpus.shards.iter().find(|shard| shard.directory.as_deref() == Some("-a")).expect("shard -a");
        assert_eq!(a.bytes, vec![9], "the streamed shard for -a must win over the stale disk snapshot");
    }

    #[test]
    fn typing_while_search_is_open_builds_the_query_instead_of_moving_selection() {
        let mut app = app(Size::new(120, 30));
        app.apply(Action::ToggleSearch);
        app.apply(Action::Type('g'));
        app.apply(Action::Type('r'));
        app.apply(Action::Type('i'));
        app.apply(Action::Type('d'));
        assert_eq!(app.search_query(), "grid");
        app.apply(Action::Untype);
        assert_eq!(app.search_query(), "gri");
    }

    #[test]
    fn escape_closes_the_search_overlay_and_clears_the_query() {
        let mut app = app(Size::new(120, 30));
        app.apply(Action::ToggleSearch);
        app.apply(Action::Type('x'));
        app.apply(Action::Ascend);
        assert!(!app.search_open());
        assert_eq!(app.search_query(), "");
    }

    #[test]
    fn a_bound_letter_like_d_types_into_the_query_rather_than_opening_diagnostics() {
        let mut app = app(Size::new(120, 30));
        app.apply(Action::ToggleSearch);
        app.apply(Action::Type('D'));
        assert_eq!(app.search_query(), "D");
        assert!(!app.diagnostics_open());
    }

    #[test]
    fn the_filter_only_opens_on_a_list_column() {
        let mut projects_focused = app(Size::new(120, 30));
        projects_focused.apply(Action::ToggleFilter);
        assert!(projects_focused.text_entry(), "Projects is focused by default, so / opens the filter");

        let mut on_conversation = app(Size::new(120, 30));
        on_conversation.apply(Action::Focus { forward: true });
        on_conversation.apply(Action::Focus { forward: true });
        on_conversation.apply(Action::ToggleFilter);
        assert!(!on_conversation.text_entry(), "the conversation column has no filterable list");
    }

    #[test]
    fn typing_a_filter_selects_the_first_fuzzy_match_and_enter_locks_it_in() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("aaa"), project("bbb"), project("grid-scanner")]));
        app.apply(Action::ToggleFilter);
        for character in "grd".chars() {
            app.apply(Action::Type(character));
        }
        assert_eq!(app.selected_project().map(|project| project.directory.as_str()), Some("grid-scanner"));
        assert!(app.text_entry());

        app.apply(Action::Descend);
        assert!(!app.text_entry(), "Enter locks the filter and leaves text entry");
        assert!(app.filter_status().is_some_and(|status| status.starts_with("/grd")));
    }

    #[test]
    fn escape_on_a_locked_filter_clears_it() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("aaa"), project("grid-scanner")]));
        app.apply(Action::ToggleFilter);
        app.apply(Action::Type('g'));
        app.apply(Action::Descend);
        assert!(app.filter_status().is_some());

        app.apply(Action::Ascend);
        assert!(app.filter_status().is_none());
    }

    #[test]
    fn escape_on_a_locked_sessions_filter_clears_it_too() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("aaa")]));
        app.set_sessions(app.generation(), vec![session("safety-alpha"), session("safety-beta"), session("other")]);
        app.apply(Action::Focus { forward: true });
        assert_eq!(app.focused(), Column::Sessions);
        app.apply(Action::ToggleFilter);
        for character in "safety".chars() {
            app.apply(Action::Type(character));
        }
        app.apply(Action::Descend);
        assert!(app.filter_status().is_some());

        app.apply(Action::Ascend);
        assert!(app.filter_status().is_none(), "escape must clear a locked Sessions filter too");
        assert_eq!(app.last(Column::Sessions), 2, "the full session list is visible again");
    }

    #[test]
    fn a_locked_filter_reduces_last_to_the_match_count_not_the_full_list() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("aaa"), project("bbb"), project("grid-scanner")]));
        app.apply(Action::ToggleFilter);
        for character in "grd".chars() {
            app.apply(Action::Type(character));
        }
        app.apply(Action::Descend);

        assert_eq!(app.last(Column::Projects), 0, "only one project matches \"grd\"");
    }

    #[test]
    fn narrowing_a_filter_clamps_the_selection_onto_a_surviving_row() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("aaa"), project("grid-one"), project("bbb"), project("grid-two")]));
        app.apply(Action::ToggleFilter);
        app.apply(Action::Type('g'));
        app.apply(Action::Move(Motion::Line(1)));
        assert_eq!(app.selected_project().map(|project| project.directory.as_str()), Some("grid-two"));

        for character in "rid-tw".chars() {
            app.apply(Action::Type(character));
        }
        assert_eq!(
            app.selected_project().map(|project| project.directory.as_str()),
            Some("grid-two"),
            "the same underlying project stays selected as the query narrows onto it"
        );
    }

    #[test]
    fn a_selection_that_no_longer_matches_falls_back_to_the_best_remaining_match() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("grid-one"), project("grid-two"), project("aaa")]));
        app.apply(Action::ToggleFilter);
        app.apply(Action::Type('g'));
        app.apply(Action::Move(Motion::Line(1)));
        assert_eq!(app.selected_project().map(|project| project.directory.as_str()), Some("grid-two"));

        app.apply(Action::Untype);
        for character in "aaa".chars() {
            app.apply(Action::Type(character));
        }
        assert_eq!(app.selected_project().map(|project| project.directory.as_str()), Some("aaa"));
    }

    #[test]
    fn a_projects_filter_that_moves_the_selection_reloads_sessions_as_it_types_not_only_on_a_step_key() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("grid-one"), project("grid-two"), project("aaa")]));
        let generation_before = app.generation();
        app.apply(Action::ToggleFilter);
        for character in "aaa".chars() {
            app.apply(Action::Type(character));
        }
        assert_eq!(app.selected_project().map(|project| project.directory.as_str()), Some("aaa"));
        assert!(app.generation() > generation_before, "the selection moved to aaa, so sessions must reload for it");
    }

    #[test]
    fn typing_a_filter_query_that_does_not_move_the_selection_reloads_nothing() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("grid-scanner"), project("aaa")]));
        app.apply(Action::ToggleFilter);
        app.apply(Action::Type('g'));
        let generation_after_first_char = app.generation();

        app.apply(Action::Type('r'));
        assert_eq!(
            app.generation(),
            generation_after_first_char,
            "grid-scanner was already selected and still matches, nothing to reload"
        );
    }

    #[test]
    fn clearing_a_filter_restores_the_underlying_index_so_browsing_resumes_from_the_same_row() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("aaa"), project("bbb"), project("grid-scanner")]));
        app.apply(Action::ToggleFilter);
        for character in "grd".chars() {
            app.apply(Action::Type(character));
        }
        app.apply(Action::Descend);
        assert_eq!(app.selected_project().map(|project| project.directory.as_str()), Some("grid-scanner"));

        app.apply(Action::Ascend);
        assert_eq!(app.last(Column::Projects), 2, "the full list is visible again");
        assert_eq!(app.selected_project().map(|project| project.directory.as_str()), Some("grid-scanner"));
    }

    #[test]
    fn stepping_a_locked_filter_wraps_through_the_visible_rows_only() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("grid-one"), project("aaa"), project("grid-two")]));
        app.apply(Action::ToggleFilter);
        app.apply(Action::Type('g'));
        app.apply(Action::Descend);
        assert_eq!(app.selected_project().map(|project| project.directory.as_str()), Some("grid-one"));

        app.apply(Action::NextCall { forward: true });
        assert_eq!(app.selected_project().map(|project| project.directory.as_str()), Some("grid-two"));

        app.apply(Action::NextCall { forward: true });
        assert_eq!(
            app.selected_project().map(|project| project.directory.as_str()),
            Some("grid-one"),
            "stepping past the last match wraps to the first"
        );

        app.apply(Action::NextCall { forward: false });
        assert_eq!(
            app.selected_project().map(|project| project.directory.as_str()),
            Some("grid-two"),
            "stepping backward past the first match wraps to the last"
        );
    }

    #[test]
    fn an_empty_query_shows_every_row_in_its_natural_order() {
        let mut app = app(Size::new(120, 30));
        app.set_projects(app.generation(), Ok(vec![project("zzz"), project("aaa")]));
        app.apply(Action::ToggleFilter);

        assert_eq!(app.last(Column::Projects), 1);
        assert_eq!(app.selected_project().map(|project| project.directory.as_str()), Some("zzz"));
    }

    #[test]
    fn copying_a_message_takes_the_one_under_the_viewport_top() {
        let mut app = with_calls(Size::new(120, 30));
        app.apply(Action::Copy(CopyTarget::Message));
        let text = app.take_copy().expect("a message under the top line");
        assert!(text.contains("do three things"), "{text}");
    }

    #[test]
    fn copying_the_resume_command_prefixes_the_directory_only_while_it_exists() {
        let mut present = app(Size::new(120, 30));
        present.projects = Loadable::Ready(vec![project("present-project")]);
        present.sessions = Loadable::Ready(vec![session("abc123")]);
        present.apply(Action::Copy(CopyTarget::Resume));
        let text = present.take_copy().expect("a resume command");
        assert_eq!(text, "cd /Users/fixture/present-project && claude --resume abc123");

        let mut gone_project = project("gone-project");
        gone_project.present = false;
        let mut gone = app(Size::new(120, 30));
        gone.projects = Loadable::Ready(vec![gone_project]);
        gone.sessions = Loadable::Ready(vec![session("abc123")]);
        gone.apply(Action::Copy(CopyTarget::Resume));
        let text = gone.take_copy().expect("a resume command with no directory");
        assert_eq!(text, "claude --resume abc123", "a gone project is not worth a cd into nowhere");
    }

    #[test]
    fn a_copy_is_handed_over_once_rather_than_on_every_poll() {
        let mut app = with_calls(Size::new(120, 30));
        app.apply(Action::Copy(CopyTarget::Message));
        assert!(app.take_copy().is_some());
        assert!(app.take_copy().is_none(), "one copy is handed over once");
    }

    #[test]
    fn every_copy_leaves_a_confirmation_that_the_next_action_clears() {
        let mut app = with_calls(Size::new(120, 30));
        app.apply(Action::Copy(CopyTarget::Message));
        assert!(app.notice().is_some());
        app.apply(Action::Move(Motion::Line(1)));
        assert!(app.notice().is_none(), "the next action clears the confirmation");
    }

    #[test]
    fn copying_with_nothing_loaded_confirms_nothing_rather_than_panicking() {
        let mut app = app(Size::new(120, 30));
        app.apply(Action::Copy(CopyTarget::Message));
        assert!(app.take_copy().is_none());
        assert_eq!(app.notice(), Some("nothing to copy"));

        app.apply(Action::Copy(CopyTarget::Session));
        assert!(app.take_copy().is_none());
        assert_eq!(app.notice(), Some("nothing to copy"));

        app.apply(Action::Copy(CopyTarget::Resume));
        assert!(app.take_copy().is_none());
        assert_eq!(app.notice(), Some("nothing to copy"));
    }

    fn with_calls_and_session(area: Size) -> App {
        let mut app = with_calls(area);
        app.sessions = Loadable::Ready(vec![session("s1")]);
        app
    }

    fn has_extension(path: &str, extension: &str) -> bool {
        Path::new(path).extension().is_some_and(|found| found == extension)
    }

    #[test]
    fn e_opens_a_prompt_with_a_default_markdown_path() {
        let mut app = with_calls_and_session(Size::new(120, 30));
        app.apply(Action::ToggleExport);
        assert!(app.export_prompt_open());
        assert_eq!(app.export_prompt_format(), Some(ExportFormat::Markdown));
        assert!(
            app.export_prompt_path().is_some_and(|path| path.starts_with("./") && has_extension(path, "md")),
            "{:?}",
            app.export_prompt_path()
        );
    }

    #[test]
    fn e_with_nothing_loaded_confirms_nothing_rather_than_opening_a_prompt() {
        let mut app = app(Size::new(120, 30));
        app.apply(Action::ToggleExport);
        assert!(!app.export_prompt_open());
        assert_eq!(app.notice(), Some("nothing to export"));
    }

    #[test]
    fn tab_cycles_the_format_and_swaps_the_extension() {
        let mut app = with_calls_and_session(Size::new(120, 30));
        app.apply(Action::ToggleExport);
        let markdown_path = app.export_prompt_path().expect("a default path").to_owned();
        assert!(has_extension(&markdown_path, "md"));

        app.apply(Action::CycleExportFormat);
        assert_eq!(app.export_prompt_format(), Some(ExportFormat::Jsonl));
        assert!(app.export_prompt_path().is_some_and(|path| has_extension(path, "jsonl")), "{:?}", app.export_prompt_path());

        app.apply(Action::CycleExportFormat);
        assert_eq!(app.export_prompt_format(), Some(ExportFormat::Markdown));
        assert_eq!(app.export_prompt_path(), Some(markdown_path.as_str()), "cycling back restores the original extension");
    }

    #[test]
    fn enter_requests_a_markdown_write_and_confirms_once_the_run_loop_reports_success() {
        let dir = tempfile::TempDir::new().expect("a temp dir");
        let dest = dir.path().join("out.md");
        let mut app = with_calls_and_session(Size::new(120, 30));
        app.apply(Action::ToggleExport);
        app.export_prompt =
            Some(ExportPrompt { path: dest.to_string_lossy().into_owned(), format: ExportFormat::Markdown, confirm: false });

        app.apply(Action::Descend);
        let (written, payload, force) = app.take_export().expect("an export request");
        assert_eq!(written, dest);
        assert!(!force);
        assert!(
            matches!(payload, ExportPayload::Text(text) if text.contains("do three things")),
            "the markdown export carries the session"
        );

        app.export_written(&dest);
        assert!(!app.export_prompt_open(), "a successful write closes the prompt");
        assert_eq!(app.notice(), Some(format!("exported to {}", dest.display())).as_deref());
    }

    #[test]
    fn an_existing_destination_asks_before_a_second_write_is_requested() {
        let dir = tempfile::TempDir::new().expect("a temp dir");
        let dest = dir.path().join("out.md");
        std::fs::write(&dest, "already here").expect("a pre-existing file");
        let mut app = with_calls_and_session(Size::new(120, 30));
        app.apply(Action::ToggleExport);
        app.export_prompt =
            Some(ExportPrompt { path: dest.to_string_lossy().into_owned(), format: ExportFormat::Markdown, confirm: false });

        app.apply(Action::Descend);
        let (_, _, force) = app.take_export().expect("the first export request");
        assert!(!force);

        app.export_needs_confirmation();
        assert!(app.export_prompt_confirm());
        assert_eq!(app.notice(), Some("overwrite? y/n"));

        app.apply(Action::Type('n'));
        assert!(!app.export_prompt_confirm(), "n backs out of the confirmation");
        assert!(app.export_prompt_open(), "and stays on the prompt rather than closing it");

        app.export_needs_confirmation();
        app.apply(Action::Type('y'));
        let (_, _, force) = app.take_export().expect("the confirmed export request");
        assert!(force, "y carries the force flag through on the next attempt");
    }

    #[test]
    fn escape_backs_out_of_a_confirmation_before_it_closes_the_prompt() {
        let mut app = with_calls_and_session(Size::new(120, 30));
        app.apply(Action::ToggleExport);
        app.export_needs_confirmation();
        assert!(app.export_prompt_confirm());

        app.apply(Action::Ascend);
        assert!(app.export_prompt_open(), "the first escape only cancels the confirmation");
        assert!(!app.export_prompt_confirm());

        app.apply(Action::Ascend);
        assert!(!app.export_prompt_open(), "the second escape closes the prompt");
    }

    #[test]
    fn the_jsonl_format_copies_the_raw_transcript_rather_than_rendering_it() {
        let dir = tempfile::TempDir::new().expect("a temp dir");
        let dest = dir.path().join("out.jsonl");
        let mut app = with_calls_and_session(Size::new(120, 30));
        app.apply(Action::ToggleExport);
        app.export_prompt =
            Some(ExportPrompt { path: dest.to_string_lossy().into_owned(), format: ExportFormat::Jsonl, confirm: false });

        app.apply(Action::Descend);
        let (_, payload, _) = app.take_export().expect("an export request");
        assert!(matches!(payload, ExportPayload::CopyFile(_)), "raw JSONL copies the transcript file verbatim");
    }

    #[test]
    fn an_export_is_requested_once_rather_than_on_every_poll() {
        let dir = tempfile::TempDir::new().expect("a temp dir");
        let dest = dir.path().join("out.md");
        let mut app = with_calls_and_session(Size::new(120, 30));
        app.apply(Action::ToggleExport);
        app.export_prompt =
            Some(ExportPrompt { path: dest.to_string_lossy().into_owned(), format: ExportFormat::Markdown, confirm: false });
        app.apply(Action::Descend);
        assert!(app.take_export().is_some());
        assert!(app.take_export().is_none(), "one write request is handed over once");
    }

    #[test]
    fn typing_edits_the_path_and_backspace_removes_from_the_end() {
        let mut app = with_calls_and_session(Size::new(120, 30));
        app.apply(Action::ToggleExport);
        app.export_prompt = Some(ExportPrompt { path: "./out.md".to_owned(), format: ExportFormat::Markdown, confirm: false });
        app.apply(Action::Type('x'));
        assert_eq!(app.export_prompt_path(), Some("./out.mdx"));
        app.apply(Action::Untype);
        assert_eq!(app.export_prompt_path(), Some("./out.md"));
    }

    #[test]
    fn dragging_in_the_conversation_highlights_the_range_and_copies_it_on_release() {
        let mut app = with_calls(Size::new(120, 30));
        app.apply(Action::Click { column: Column::Conversation, row: 0 });
        assert!(app.selected_rows().is_none(), "a click with no movement is not yet a selection");

        app.apply(Action::Drag { column: Column::Conversation, row: 2 });
        assert_eq!(app.selected_rows(), Some(0..=2));

        app.apply(Action::Release);
        assert!(app.selected_rows().is_none(), "the highlight clears once the copy is handed off");
        let text = app.take_copy().expect("a copied selection");
        assert!(text.contains("do three things"), "{text}");
        assert!(app.notice().is_some_and(|notice| notice.starts_with("copied selection")), "{:?}", app.notice());
    }

    #[test]
    fn a_click_without_movement_copies_nothing_on_release() {
        let mut app = with_calls(Size::new(120, 30));
        app.apply(Action::Click { column: Column::Conversation, row: 1 });
        app.apply(Action::Release);
        assert!(app.take_copy().is_none(), "a plain click is not a drag");
    }

    #[test]
    fn dragging_outside_the_conversation_column_does_nothing() {
        let mut app = with_calls(Size::new(120, 30));
        app.apply(Action::Click { column: Column::Conversation, row: 0 });
        app.apply(Action::Drag { column: Column::Sessions, row: 2 });
        assert!(app.selected_rows().is_none());
    }

    #[test]
    fn a_fresh_click_clears_a_previous_selection() {
        let mut app = with_calls(Size::new(120, 30));
        app.apply(Action::Click { column: Column::Conversation, row: 0 });
        app.apply(Action::Drag { column: Column::Conversation, row: 2 });
        assert!(app.selected_rows().is_some());

        app.apply(Action::Click { column: Column::Conversation, row: 0 });
        assert!(app.selected_rows().is_none(), "a new click starts over rather than keeping the old range");
    }

    #[test]
    fn a_resize_clears_a_stale_selection() {
        let mut app = with_calls(Size::new(120, 30));
        app.apply(Action::Click { column: Column::Conversation, row: 0 });
        app.apply(Action::Drag { column: Column::Conversation, row: 2 });
        assert!(app.selected_rows().is_some());

        app.apply(Action::Resize(Size::new(80, 30)));
        app.reflow();
        assert!(app.selected_rows().is_none(), "a rewrap invalidates the line numbers a selection was built from");
    }
}
