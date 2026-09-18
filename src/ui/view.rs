//! Painting one frame: header, hairline rules, the columns, and the status bar.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;
use ratatui::{Frame, symbols};
use unicode_width::UnicodeWidthStr;

use crate::render::line::RenderedLine;
use crate::ui::age;
use crate::ui::app::{App, Column, Loadable, Mode};
use crate::ui::columns::{self, Columns};

const TITLE_PREFIX: &str = "rewind";
const HINTS: &str = "? help";
const HINT_GAP: usize = 2;
const EDGE_PAD: u16 = 1;
const CHEVRON: &str = "›";
const GONE: &str = "⊘";
const INFO_WIDTH: u16 = 7;

const fn hint_style() -> Style {
    Style::new().fg(Color::DarkGray)
}

const fn title_style() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

const fn cursor_line_style() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

const fn scroll_progress_color() -> Color {
    Color::Cyan
}

pub fn draw(frame: &mut Frame, app: &App) {
    frame.render_widget(Screen { app }, frame.area());
}

struct Screen<'a> {
    app: &'a App,
}

impl Widget for Screen<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let rows =
            [Constraint::Length(1), Constraint::Length(1), Constraint::Min(0), Constraint::Length(1), Constraint::Length(1)];
        let [header_row, top_rule, content_rows, bottom_rule, status_row] = Layout::vertical(rows).areas(area);

        header(header_row, buf, self.app);
        rule(top_rule, buf, hint_style());
        progress(top_rule, buf, self.app);
        content(content_rows, buf, self.app);
        rule(bottom_rule, buf, hint_style());
        statusbar(status_row, buf, self.app);
    }
}

fn row(area: Rect, buf: &mut Buffer, x: u16, text: &str, style: Style) {
    if area.height == 0 || x >= area.right() {
        return;
    }
    buf.set_stringn(x, area.y, text, usize::from(area.right().saturating_sub(x)), style);
}

fn padded(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(EDGE_PAD.min(area.width)),
        width: area.width.saturating_sub(EDGE_PAD.saturating_mul(2)),
        ..area
    }
}

fn header(area: Rect, buf: &mut Buffer, app: &App) {
    let area = padded(area);
    let project = app.selected_project().map(|project| project.path.display().to_string());
    let session = app.selected_session().map(|session| session.title.clone());
    let title = [Some(TITLE_PREFIX.to_owned()), project, session].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    row(area, buf, area.x, &title, title_style());

    let hints = HINTS.width();
    if title.width().saturating_add(HINT_GAP).saturating_add(hints) <= usize::from(area.width) {
        let x = area.right().saturating_sub(u16::try_from(hints).unwrap_or(area.width));
        row(area, buf, x, HINTS, hint_style());
    }
}

fn rule(area: Rect, buf: &mut Buffer, style: Style) {
    row(area, buf, area.x, &symbols::line::HORIZONTAL.repeat(usize::from(area.width)), style);
}

fn progress(area: Rect, buf: &mut Buffer, app: &App) {
    if area.height == 0 {
        return;
    }
    let pane = app.pane(app.focused());
    let last = match app.focused() {
        Column::Projects => matches!(app.projects(), Loadable::Ready(projects) if !projects.is_empty()),
        Column::Sessions => matches!(app.sessions(), Loadable::Ready(sessions) if !sessions.is_empty()),
        Column::Conversation => !app.lines().is_empty(),
    };
    if !last {
        return;
    }
    let denominator = app.last(app.focused()).max(1);
    let position = if app.focused() == Column::Conversation { pane.top } else { pane.selected };
    let filled = u16::try_from(position.saturating_mul(usize::from(area.width)).checked_div(denominator).unwrap_or(0))
        .unwrap_or(u16::MAX)
        .min(area.width);
    for x in area.x..area.x.saturating_add(filled) {
        if let Some(cell) = buf.cell_mut((x, area.y)) {
            cell.set_fg(scroll_progress_color());
        }
    }
}

fn content(area: Rect, buf: &mut Buffer, app: &App) {
    if app.mode() == Mode::Focus {
        column(area, buf, app, Column::Conversation, true);
        return;
    }

    match columns::layout(area.width) {
        Columns::Three { projects, sessions, conversation } => {
            let x1 = area.x.saturating_add(projects);
            let x2 = x1.saturating_add(1).saturating_add(sessions);
            column(Rect { x: area.x, width: projects, ..area }, buf, app, Column::Projects, app.focused() == Column::Projects);
            divider(x1, area, buf);
            column(
                Rect { x: x1.saturating_add(1), width: sessions, ..area },
                buf,
                app,
                Column::Sessions,
                app.focused() == Column::Sessions,
            );
            divider(x2, area, buf);
            column(
                Rect { x: x2.saturating_add(1), width: conversation, ..area },
                buf,
                app,
                Column::Conversation,
                app.focused() == Column::Conversation,
            );
        }
        Columns::Two { sessions, conversation } => {
            let x1 = area.x.saturating_add(sessions);
            column(Rect { x: area.x, width: sessions, ..area }, buf, app, Column::Sessions, app.focused() == Column::Sessions);
            divider(x1, area, buf);
            column(
                Rect { x: x1.saturating_add(1), width: conversation, ..area },
                buf,
                app,
                Column::Conversation,
                app.focused() == Column::Conversation,
            );
        }
    }
}

fn divider(x: u16, area: Rect, buf: &mut Buffer) {
    for y in area.y..area.bottom() {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_symbol(symbols::line::VERTICAL);
        }
    }
}

fn column(area: Rect, buf: &mut Buffer, app: &App, which: Column, focused: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    row(area, buf, area.x, label(which), title_style());
    let inner = Rect { y: area.y.saturating_add(1), height: area.height.saturating_sub(1), ..area };
    match which {
        Column::Projects => projects_rows(inner, buf, app, focused),
        Column::Sessions => sessions_rows(inner, buf, app, focused),
        Column::Conversation => conversation_rows(inner, buf, app),
    }
}

const fn label(column: Column) -> &'static str {
    match column {
        Column::Projects => "Projects",
        Column::Sessions => "Sessions",
        Column::Conversation => "Conversation",
    }
}

fn projects_rows(area: Rect, buf: &mut Buffer, app: &App, focused: bool) {
    let Loadable::Ready(projects) = app.projects() else { return };
    let pane = app.pane(Column::Projects);
    let last = projects.len().min(pane.top.saturating_add(usize::from(area.height)));

    for (row_index, index) in (pane.top..last).enumerate() {
        let Some(project) = projects.get(index) else { continue };
        let y = area.y.saturating_add(u16::try_from(row_index).unwrap_or(u16::MAX));
        let picked = index == pane.selected;
        if picked {
            buf.set_style(Rect { y, height: 1, ..area }, cursor_line_style());
        }
        let marker = if !project.present {
            GONE
        } else if picked && focused {
            CHEVRON
        } else {
            " "
        };
        row(Rect { y, height: 1, ..area }, buf, area.x, marker, Style::new());

        let name = project
            .path
            .file_name()
            .map_or_else(|| project.path.display().to_string(), |name| name.to_string_lossy().into_owned());
        let info = format!("{} {}", age::relative(app.ctx.now, timestamp_of(project.last_activity)), project.sessions);
        text_and_info(Rect { y, height: 1, ..area }, buf, &name, &info);
    }
}

fn sessions_rows(area: Rect, buf: &mut Buffer, app: &App, focused: bool) {
    let Loadable::Ready(sessions) = app.sessions() else { return };
    let pane = app.pane(Column::Sessions);
    let last = sessions.len().min(pane.top.saturating_add(usize::from(area.height)));

    for (row_index, index) in (pane.top..last).enumerate() {
        let Some(session) = sessions.get(index) else { continue };
        let y = area.y.saturating_add(u16::try_from(row_index).unwrap_or(u16::MAX));
        let picked = index == pane.selected;
        if picked {
            buf.set_style(Rect { y, height: 1, ..area }, cursor_line_style());
        }
        let marker = if picked && focused { CHEVRON } else { " " };
        row(Rect { y, height: 1, ..area }, buf, area.x, marker, Style::new());

        let age = session.last_activity.map_or_else(|| "-".to_owned(), |at| age::relative(app.ctx.now, at));
        let info = format!("{age} {}", session.messages);
        text_and_info(Rect { y, height: 1, ..area }, buf, &session.title, &info);
    }
}

fn conversation_rows(area: Rect, buf: &mut Buffer, app: &App) {
    let lines = app.lines();
    let top = app.pane(Column::Conversation).top;
    let last = lines.len().min(top.saturating_add(usize::from(area.height)));
    let cursor = app.cursor_line();

    for (row_index, index) in (top..last).enumerate() {
        let Some(line) = lines.get(index) else { continue };
        let y = area.y.saturating_add(u16::try_from(row_index).unwrap_or(u16::MAX));
        let row = Rect { y, height: 1, ..area };
        painted(row, buf, line);
        if cursor == Some(index) {
            buf.set_style(row, cursor_line_style());
        }
    }
}

fn painted(area: Rect, buf: &mut Buffer, line: &RenderedLine) {
    let mut x = area.x;
    for span in &line.spans {
        if x >= area.right() {
            return;
        }
        row(area, buf, x, &span.text, span.style);
        x = x.saturating_add(u16::try_from(span.width()).unwrap_or(area.width));
    }
}

fn text_and_info(area: Rect, buf: &mut Buffer, text: &str, info: &str) {
    let x = area.x.saturating_add(2);
    let info_width = INFO_WIDTH.min(area.width);
    let text_width = area.width.saturating_sub(2).saturating_sub(info_width);
    row(Rect { width: text_width, ..area }, buf, x, text, Style::new());

    let info_x = area.right().saturating_sub(u16::try_from(info.width()).unwrap_or(info_width).min(info_width));
    row(area, buf, info_x, info, hint_style());
}

fn timestamp_of(at: std::time::SystemTime) -> jiff::Timestamp {
    jiff::Timestamp::try_from(at).unwrap_or(jiff::Timestamp::UNIX_EPOCH)
}

fn statusbar(area: Rect, buf: &mut Buffer, app: &App) {
    let area = padded(area);
    let Some(session) = app.selected_session() else { return };
    let branch = session.git_branch.as_deref().unwrap_or("-");
    let plural = if session.messages == 1 { "msg" } else { "msgs" };
    let age = session.last_activity.map_or_else(|| "-".to_owned(), |at| age::relative(app.ctx.now, at));
    let text = format!("{} · {} {plural} · {branch} · {age}", session.id, session.messages);
    row(area, buf, area.x, &text, Style::new());
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::SystemTime;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Size;

    use super::*;
    use crate::ctx::Ctx;
    use crate::domain::project::{Project, Resolution};
    use crate::domain::session::{Session, TitleSource};
    use crate::ui::Options;
    use crate::ui::app::Pane;
    use crate::ui::input::{Action, Motion};

    fn ctx() -> Ctx {
        Ctx { now: "2026-01-12T00:00:00Z".parse().expect("a valid instant") }
    }

    fn app(size: Size) -> App {
        App::new(ctx(), &Options { claude_dir: PathBuf::from("/tmp"), ..Options::default() }, size)
    }

    fn project(directory: &str, present: bool) -> Project {
        Project {
            directory: directory.to_owned(),
            path: PathBuf::from(format!("/Users/fixture/{directory}")),
            resolution: Resolution::Mapped,
            sessions: 4,
            last_activity: SystemTime::UNIX_EPOCH,
            present,
        }
    }

    fn session(id: &str, title: &str) -> Session {
        Session {
            id: id.to_owned(),
            path: PathBuf::from(format!("/tmp/{id}.jsonl")),
            title: title.to_owned(),
            title_source: TitleSource::FirstMessage,
            slug: None,
            git_branch: Some("main".to_owned()),
            first_activity: None,
            last_activity: Some(ctx().now),
            records: 10,
            messages: 6,
            continued_in: None,
        }
    }

    fn frame(app: &App, size: Size) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(size.width, size.height)).expect("a test terminal");
        terminal.draw(|frame| draw(frame, app)).expect("a drawn frame");
        terminal.backend().buffer().clone()
    }

    fn text_row(buffer: &Buffer, y: u16) -> String {
        let mut text = String::new();
        for x in 0..buffer.area.width {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.trim_end().to_owned()
    }

    #[test]
    fn a_wide_terminal_shows_all_three_column_headers() {
        let app = app(Size::new(120, 24));
        let buffer = frame(&app, Size::new(120, 24));
        let header = text_row(&buffer, 2);
        assert!(header.contains("Projects"), "{header}");
        assert!(header.contains("Sessions"), "{header}");
        assert!(header.contains("Conversation"), "{header}");
    }

    #[test]
    fn a_narrow_terminal_drops_the_projects_column() {
        let app = app(Size::new(40, 24));
        let buffer = frame(&app, Size::new(40, 24));
        let header = text_row(&buffer, 2);
        assert!(!header.contains("Projects"), "{header}");
        assert!(header.contains("Sessions"), "{header}");
    }

    #[test]
    fn a_tmux_pane_of_ninety_three_columns_still_paints_all_three() {
        let app = app(Size::new(93, 24));
        let buffer = frame(&app, Size::new(93, 24));
        let header = text_row(&buffer, 2);
        assert!(header.contains("Projects"), "{header}");
        assert!(header.contains("Sessions"), "{header}");
        assert!(header.contains("Conversation"), "{header}");
    }

    #[test]
    fn projects_are_listed_with_their_age_and_session_count() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("holodeck", true)]));
        let buffer = frame(&app, Size::new(120, 24));
        let row = text_row(&buffer, 3);
        assert!(row.contains("holodeck"), "{row}");
        assert!(row.contains('4'), "{row}");
    }

    #[test]
    fn a_gone_project_carries_the_gone_marker() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("nomad", false)]));
        let buffer = frame(&app, Size::new(120, 24));
        let row = text_row(&buffer, 3);
        assert!(row.starts_with(GONE), "{row}");
    }

    #[test]
    fn the_focused_column_selection_carries_the_chevron() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true), project("b", true)]));
        let buffer = frame(&app, Size::new(120, 24));
        let row = text_row(&buffer, 3);
        assert!(row.starts_with(CHEVRON), "{row}");
    }

    #[test]
    fn focus_mode_renders_only_the_conversation_column_at_full_width() {
        let mut app = app(Size::new(120, 24));
        app.apply(Action::ToggleFocusMode);
        let buffer = frame(&app, Size::new(120, 24));
        let header = text_row(&buffer, 2);
        assert_eq!(header.trim(), "Conversation");
    }

    #[test]
    fn the_statusbar_reads_the_selected_session() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true)]));
        let generation = app.generation();
        app.set_sessions(generation, vec![session("s1", "hello there")]);
        let buffer = frame(&app, Size::new(120, 24));
        let status = text_row(&buffer, 23);
        assert!(status.contains("s1"), "{status}");
        assert!(status.contains("6 msgs"), "{status}");
        assert!(status.contains("main"), "{status}");
    }

    #[test]
    fn the_top_rule_is_untinted_with_nothing_loaded() {
        let app = app(Size::new(120, 24));
        let buffer = frame(&app, Size::new(120, 24));
        let tinted = (0..120).filter(|&x| buffer[(x, 1)].fg == scroll_progress_color()).count();
        assert_eq!(tinted, 0);
    }

    #[test]
    fn scrolling_tints_the_top_rule() {
        let mut app = app(Size::new(120, 10));
        app.set_projects(app.generation(), Ok((0..50).map(|index| project(&format!("p{index}"), true)).collect()));
        app.apply(Action::Move(Motion::Bottom));
        let buffer = frame(&app, Size::new(120, 10));
        let tinted = (0..120).filter(|&x| buffer[(x, 1)].fg == scroll_progress_color()).count();
        assert!(tinted > 0);
    }

    fn with_conversation(app: &mut App, text: &str) {
        use std::fs;

        use tempfile::TempDir;

        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("session.jsonl");
        let line = format!(
            r#"{{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:00Z","origin":{{"kind":"human"}},"message":{{"role":"user","content":"{text}"}}}}"#
        );
        fs::write(&path, line + "\n").expect("a written transcript");
        let conversation = crate::domain::thread::build(&path).expect("a built conversation");
        let generation = app.conversation_generation();
        app.set_conversation(generation, Ok(Box::new(conversation)));
        app.reflow();
    }

    #[test]
    fn the_conversation_column_paints_the_loaded_transcript() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true)]));
        app.set_sessions(app.generation(), vec![session("s1", "a session")]);
        with_conversation(&mut app, "read the grid scanner back to me");

        let buffer = frame(&app, Size::new(120, 24));
        assert!(text_row(&buffer, 3).contains("▎ you"), "{}", text_row(&buffer, 3));
        assert!(text_row(&buffer, 4).contains("▎ read the grid scanner"), "{}", text_row(&buffer, 4));
    }

    #[test]
    fn scrolling_the_conversation_moves_the_first_painted_line() {
        let mut app = app(Size::new(60, 12));
        app.set_projects(app.generation(), Ok(vec![project("a", true)]));
        app.set_sessions(app.generation(), vec![session("s1", "a session")]);
        app.apply(Action::ToggleFocusMode);
        with_conversation(&mut app, &"prose ".repeat(200));

        let before = text_row(&frame(&app, Size::new(60, 12)), 3);
        app.apply(Action::Move(Motion::HalfPage(1)));
        let after = text_row(&frame(&app, Size::new(60, 12)), 3);
        assert_ne!(before, after, "the pane did not scroll");
    }

    #[test]
    fn a_terminal_too_short_for_the_chrome_draws_what_it_can_and_does_not_panic() {
        let app = app(Size::new(40, 3));
        let _ = frame(&app, Size::new(40, 3));
    }

    #[test]
    fn a_zero_area_terminal_does_not_panic() {
        let app = app(Size::new(0, 0));
        let _ = frame(&app, Size::new(0, 0));
    }

    #[test]
    fn a_selected_row_outside_focus_carries_the_band_but_not_the_chevron() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true), project("b", true)]));
        app.apply(Action::Focus { forward: true });
        let buffer = frame(&app, Size::new(120, 24));
        let row = text_row(&buffer, 3);
        assert!(!row.starts_with(CHEVRON), "{row}");
        assert_eq!(buffer[(0, 3)].style().add_modifier, Modifier::REVERSED);
    }

    fn pane_of(app: &App, column: Column) -> Pane {
        app.pane(column)
    }

    #[test]
    fn the_pane_accessor_matches_the_focused_column() {
        let app = app(Size::new(120, 24));
        assert_eq!(pane_of(&app, Column::Projects), Pane::default());
    }
}
