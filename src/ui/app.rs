//! The shell's state, and the reducer that is the only way to change it.

use std::path::{Path, PathBuf};

use ratatui::layout::Size;

use crate::ctx::Ctx;
use crate::domain::project::{Project, ProjectError};
use crate::domain::session::Session;
use crate::ui::input::{Action, Motion};
use crate::ui::{Options, listing};

pub(crate) const CHROME_ROWS: u16 = 5;

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
pub enum Loadable<T> {
    Loading,
    Ready(T),
    Failed(String),
}

pub struct App {
    pub ctx: Ctx,
    pub quit: bool,
    claude_dir: PathBuf,
    projects: Loadable<Vec<Project>>,
    sessions: Loadable<Vec<Session>>,
    projects_pane: Pane,
    sessions_pane: Pane,
    conversation_pane: Pane,
    focused: Column,
    pre_focus: Column,
    mode: Mode,
    area: Size,
    generation: u64,
    pending_project: Option<String>,
    pending_session: Option<String>,
    pending_session_load: Option<(PathBuf, u64)>,
}

impl App {
    pub fn new(ctx: Ctx, options: &Options, area: Size) -> Self {
        Self {
            ctx,
            quit: false,
            claude_dir: options.claude_dir.clone(),
            projects: Loadable::Loading,
            sessions: Loadable::Loading,
            projects_pane: Pane::default(),
            sessions_pane: Pane::default(),
            conversation_pane: Pane::default(),
            focused: Column::Projects,
            pre_focus: Column::Projects,
            mode: Mode::Browse,
            area,
            generation: 0,
            pending_project: options.project.clone(),
            pending_session: options.session.clone(),
            pending_session_load: None,
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
        self.sessions = Loadable::Ready(sessions);
    }

    pub fn apply(&mut self, action: Action) {
        match action {
            Action::Quit => self.quit = true,
            Action::Resize(size) => self.area = size,
            Action::ToggleFocusMode => self.toggle_focus_mode(),
            Action::Focus { forward } => self.move_focus(forward),
            Action::Descend => self.move_focus(true),
            Action::Ascend => self.ascend(),
            Action::Move(motion) => self.move_selection(motion),
        }
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

    fn ascend(&mut self) {
        if self.mode == Mode::Focus {
            self.toggle_focus_mode();
            return;
        }
        self.move_focus(false);
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
        let last = self.last(column);
        let height = self.pane_height();
        let pane = self.pane_mut(column);

        pane.selected = listing::target(motion, pane.selected, last, height);
        pane.top = listing::revealed(pane.top, pane.selected, height);

        if column == Column::Projects {
            self.request_sessions_for_selection();
        }
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
    }

    const fn pane_mut(&mut self, column: Column) -> &mut Pane {
        match column {
            Column::Projects => &mut self.projects_pane,
            Column::Sessions => &mut self.sessions_pane,
            Column::Conversation => &mut self.conversation_pane,
        }
    }

    const fn last(&self, column: Column) -> usize {
        match column {
            Column::Projects => match &self.projects {
                Loadable::Ready(projects) => projects.len().saturating_sub(1),
                _ => 0,
            },
            Column::Sessions => match &self.sessions {
                Loadable::Ready(sessions) => sessions.len().saturating_sub(1),
                _ => 0,
            },
            Column::Conversation => 0,
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
        }
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

    #[test]
    fn resizing_updates_the_area() {
        let mut app = app(Size::new(120, 30));
        app.apply(Action::Resize(Size::new(80, 24)));
        assert_eq!(app.area(), Size::new(80, 24));
    }
}
