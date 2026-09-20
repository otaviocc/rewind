//! Painting one frame: header, hairline rules, the columns, and the status bar.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::Style;
use ratatui::widgets::{Block, Clear, Widget};
use ratatui::{Frame, symbols};
use unicode_width::UnicodeWidthStr;

use crate::domain::project::Resolution;
use crate::render::line::{RenderedLine, StyledSpan, truncate};
use crate::theme::Element;
use crate::ui::age;
use crate::ui::app::{App, Column, Loadable};
use crate::ui::columns;
use crate::ui::diagnostics;
use crate::ui::empty;
use crate::ui::export_prompt;
use crate::ui::help;
use crate::ui::search;

const TITLE_PREFIX: &str = "rewind";
const HINTS: &str = "? keys";
const SEPARATOR: &str = " · ";
const ELIDED: &str = "…";
const HINT_GAP: usize = 2;
const EDGE_PAD: u16 = 1;
const CHEVRON: &str = "›";
const GONE: &str = "⊘";
const DOT: &str = "●";
const INFO_WIDTH: u16 = 7;

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
        rule(top_rule, buf, self.app.theme().style(Element::Hint));
        progress(top_rule, buf, self.app);
        content(content_rows, buf, self.app);
        overlay(content_rows, buf, self.app);
        rule(bottom_rule, buf, self.app.theme().style(Element::Hint));
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
    let mut segments: Vec<String> = [Some(TITLE_PREFIX.to_owned()), project, session].into_iter().flatten().collect();
    segments.extend(app.trail().into_iter().map(str::to_owned));
    let hints = HINTS.width();
    let room = usize::from(area.width).saturating_sub(HINT_GAP).saturating_sub(hints);
    let title = breadcrumb(&elided(&segments, room), app);
    painted(area, buf, &title);

    if title.width().saturating_add(HINT_GAP).saturating_add(hints) <= usize::from(area.width) {
        let x = area.right().saturating_sub(u16::try_from(hints).unwrap_or(area.width));
        row(area, buf, x, HINTS, app.theme().style(Element::Hint));
    }
}

fn elided(segments: &[String], room: usize) -> Vec<String> {
    let pieces = |from: usize, lead: bool| {
        let tail = segments.get(from..).unwrap_or_default().iter().cloned();
        if lead { std::iter::once(ELIDED.to_owned()).chain(tail).collect() } else { tail.collect::<Vec<String>>() }
    };
    let width = |pieces: &[String]| {
        pieces
            .iter()
            .map(|piece| piece.width())
            .fold(0, usize::saturating_add)
            .saturating_add(SEPARATOR.width().saturating_mul(pieces.len().saturating_sub(1)))
    };
    let whole = pieces(0, false);
    if width(&whole) <= room {
        return whole;
    }
    for from in 1..segments.len() {
        let candidate = pieces(from, true);
        if width(&candidate) <= room {
            return candidate;
        }
    }
    segments.last().map_or_else(Vec::new, |last| vec![truncate(last, room)])
}

fn breadcrumb(pieces: &[String], app: &App) -> RenderedLine {
    let quiet = app.theme().style(Element::Status);
    let mut line = RenderedLine::blank();
    for (index, piece) in pieces.iter().enumerate() {
        if index > 0 {
            line.push(StyledSpan::new(SEPARATOR, quiet));
        }
        let style = if index == 0 && piece == TITLE_PREFIX { app.theme().style(Element::HeaderTitle) } else { quiet };
        line.push(StyledSpan::new(piece.clone(), style));
    }
    line
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
            cell.set_fg(app.theme().style(Element::ScrollProgress).fg.unwrap_or_default());
        }
    }
}

fn content(area: Rect, buf: &mut Buffer, app: &App) {
    for (index, (which, x, width)) in columns::placement(area.width, app.mode()).into_iter().enumerate() {
        if index > 0 {
            divider(area.x.saturating_add(x).saturating_sub(1), area, buf, app.theme().style(Element::Hint));
        }
        column(Rect { x: area.x.saturating_add(x), width, ..area }, buf, app, which, app.focused() == which);
    }
}

const fn centred(area: Rect, outer: Size) -> Rect {
    Rect {
        x: area.x.saturating_add(area.width.saturating_sub(outer.width).saturating_div(2)),
        y: area.y.saturating_add(area.height.saturating_sub(outer.height).saturating_div(2)),
        width: outer.width,
        height: outer.height,
    }
}

fn overlay(area: Rect, buf: &mut Buffer, app: &App) {
    if app.export_prompt_open() {
        export_overlay(area, buf, app);
        return;
    }
    if app.search_open() {
        search_overlay(area, buf, app);
        return;
    }
    if app.help_open() {
        help_overlay(area, buf, app);
        return;
    }
    if !app.diagnostics_open() {
        return;
    }
    let outer = diagnostics::outer(Size::new(area.width, area.height));
    if outer.width <= 2 || outer.height <= 2 {
        return;
    }
    let box_area = centred(area, outer);
    Clear.render(box_area, buf);
    let frame = Block::bordered()
        .title(diagnostics::TITLE)
        .border_style(app.theme().style(Element::Hint))
        .title_style(app.theme().style(Element::HeaderTitle));
    let inner = frame.inner(box_area);
    frame.render(box_area, buf);

    let lines = app.diagnostics_lines();
    let top = app.diagnostics_top();
    let last = lines.len().min(top.saturating_add(usize::from(inner.height)));
    for (row_index, index) in (top..last).enumerate() {
        let Some(line) = lines.get(index) else { continue };
        let y = inner.y.saturating_add(u16::try_from(row_index).unwrap_or(u16::MAX));
        painted(Rect { y, height: 1, ..inner }, buf, line);
    }
}

fn help_overlay(area: Rect, buf: &mut Buffer, app: &App) {
    let outer = help::outer(Size::new(area.width, area.height));
    if outer.width <= 2 || outer.height <= 2 {
        return;
    }
    let box_area = centred(area, outer);
    Clear.render(box_area, buf);
    buf.set_style(box_area, app.theme().style(Element::HelpWindow));
    let frame = Block::bordered()
        .title(help::TITLE)
        .border_style(app.theme().style(Element::Hint))
        .title_style(app.theme().style(Element::HeaderTitle));
    let inner = frame.inner(box_area);
    frame.render(box_area, buf);

    let lines = app.help_lines();
    let top = app.help_top();
    let last = lines.len().min(top.saturating_add(usize::from(inner.height)));
    for (row_index, index) in (top..last).enumerate() {
        let Some(line) = lines.get(index) else { continue };
        let y = inner.y.saturating_add(u16::try_from(row_index).unwrap_or(u16::MAX));
        painted(Rect { y, height: 1, ..inner }, buf, line);
    }
}

fn search_overlay(area: Rect, buf: &mut Buffer, app: &App) {
    let outer = search::outer(Size::new(area.width, area.height));
    if outer.width <= 2 || outer.height <= search::HEADER_ROWS {
        return;
    }
    let box_area = centred(area, outer);
    Clear.render(box_area, buf);
    let frame = Block::bordered()
        .title(search::TITLE)
        .border_style(app.theme().style(Element::Hint))
        .title_style(app.theme().style(Element::HeaderTitle));
    let inner = frame.inner(box_area);
    frame.render(box_area, buf);

    let width = usize::from(inner.width);
    let prompt = search::prompt_line(app.search_query(), width, app.theme());
    painted(Rect { height: 1, ..inner }, buf, &prompt);

    let status_row = Rect { y: inner.y.saturating_add(1), height: 1, ..inner };
    let status = search::status_line(app.search_query(), app.corpus_status(), app.search_results().len(), width, app.theme());
    painted(status_row, buf, &status);

    let list_area = Rect {
        y: inner.y.saturating_add(search::HEADER_ROWS),
        height: inner.height.saturating_sub(search::HEADER_ROWS),
        ..inner
    };
    let rows = search::rows(app.search_results(), width, app.theme());
    let pane = app.search_pane();
    let last = rows.len().min(pane.top.saturating_add(usize::from(list_area.height)));
    for (row_index, index) in (pane.top..last).enumerate() {
        let Some(line) = rows.get(index) else { continue };
        let y = list_area.y.saturating_add(u16::try_from(row_index).unwrap_or(u16::MAX));
        let row_rect = Rect { y, height: 1, ..list_area };
        if index == pane.selected {
            for x in row_rect.x..row_rect.right() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_style(app.theme().style(Element::Selection));
                }
            }
        }
        painted(row_rect, buf, line);
    }
}

fn export_overlay(area: Rect, buf: &mut Buffer, app: &App) {
    let outer = export_prompt::outer(Size::new(area.width, area.height));
    if outer.width <= 2 || outer.height <= 2 {
        return;
    }
    let box_area = centred(area, outer);
    Clear.render(box_area, buf);
    let frame = Block::bordered()
        .title(export_prompt::TITLE)
        .border_style(app.theme().style(Element::Hint))
        .title_style(app.theme().style(Element::HeaderTitle));
    let inner = frame.inner(box_area);
    frame.render(box_area, buf);

    let (Some(format), Some(path)) = (app.export_prompt_format(), app.export_prompt_path()) else { return };
    let confirm = app.export_prompt_confirm();
    let width = usize::from(inner.width);

    let format_row = Rect { height: 1, ..inner };
    painted(format_row, buf, &export_prompt::format_line(format, width, app.theme()));

    let path_row = Rect { y: inner.y.saturating_add(1), height: 1, ..inner };
    painted(path_row, buf, &export_prompt::path_line(path, width, app.theme()));

    let hint_row = Rect { y: inner.y.saturating_add(2), height: 1, ..inner };
    painted(hint_row, buf, &export_prompt::hint_line(confirm, width, app.theme()));
}

fn divider(x: u16, area: Rect, buf: &mut Buffer, style: Style) {
    for y in area.y..area.bottom() {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_symbol(symbols::line::VERTICAL);
            cell.set_style(style);
        }
    }
}

fn column(area: Rect, buf: &mut Buffer, app: &App, which: Column, focused: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let title = if focused { Element::ColumnTitleActive } else { Element::ColumnTitle };
    row(area, buf, area.x, label(which), app.theme().style(title));
    let inner = Rect { y: area.y.saturating_add(1), height: area.height.saturating_sub(1), ..area };
    match which {
        Column::Projects => projects_rows(inner, buf, app, focused),
        Column::Sessions => sessions_rows(inner, buf, app, focused),
        Column::Conversation => conversation_rows(inner, buf, app, focused),
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
    if let Some((primary, detail)) = app.projects_empty() {
        empty_state(area, buf, app, &primary, detail.as_deref());
        return;
    }
    let Loadable::Ready(projects) = app.projects() else { return };
    let pane = app.pane(Column::Projects);
    let last = app.visible_len(Column::Projects).min(pane.top.saturating_add(usize::from(area.height)));

    for (row_index, position) in (pane.top..last).enumerate() {
        let Some(index) = app.resolve_position(Column::Projects, position) else { continue };
        let Some(project) = projects.get(index) else { continue };
        let y = area.y.saturating_add(u16::try_from(row_index).unwrap_or(u16::MAX));
        let picked = position == pane.selected;
        let gone = !project.present && project.resolution != Resolution::Unresolved;
        let marker = if gone {
            GONE
        } else if picked && focused {
            CHEVRON
        } else {
            " "
        };
        let marker_style = if gone { app.theme().style(Element::ProjectMissing) } else { app.theme().style(Element::Body) };
        row(Rect { y, height: 1, ..area }, buf, area.x, marker, marker_style);

        let name = project
            .path
            .file_name()
            .map_or_else(|| project.path.display().to_string(), |name| name.to_string_lossy().into_owned());
        let info = format!("{} {}", age::relative(app.ctx.now, timestamp_of(project.last_activity)), project.sessions);
        let name_element = if gone { Element::ProjectMissing } else { Element::Body };
        text_and_info(Rect { y, height: 1, ..area }, buf, &name, &info, app, name_element);
        band(Rect { y, height: 1, ..area }, buf, app, picked, focused);
    }
}

fn sessions_rows(area: Rect, buf: &mut Buffer, app: &App, focused: bool) {
    if let Some((primary, detail)) = app.sessions_empty() {
        empty_state(area, buf, app, &primary, detail.as_deref());
        return;
    }
    let Loadable::Ready(sessions) = app.sessions() else { return };
    let pane = app.pane(Column::Sessions);
    let last = app.visible_len(Column::Sessions).min(pane.top.saturating_add(usize::from(area.height)));

    for (row_index, position) in (pane.top..last).enumerate() {
        let Some(index) = app.resolve_position(Column::Sessions, position) else { continue };
        let Some(session) = sessions.get(index) else { continue };
        let y = area.y.saturating_add(u16::try_from(row_index).unwrap_or(u16::MAX));
        let picked = position == pane.selected;
        let live = app.live_for(&session.id);
        let marker = if picked && focused {
            CHEVRON
        } else if live.is_some() {
            DOT
        } else {
            " "
        };
        let marker_style =
            if live.is_some() { app.theme().style(Element::SessionLive) } else { app.theme().style(Element::Body) };
        row(Rect { y, height: 1, ..area }, buf, area.x, marker, marker_style);

        let age = session.last_activity.map_or_else(|| "-".to_owned(), |at| age::relative(app.ctx.now, at));
        let info = format!("{age} {}", session.messages);
        let title = live.map_or_else(|| session.title.clone(), |live| format!("{}{SEPARATOR}{}", session.title, live.describe()));
        text_and_info(Rect { y, height: 1, ..area }, buf, &title, &info, app, Element::Body);
        band(Rect { y, height: 1, ..area }, buf, app, picked, focused);
    }
}

fn conversation_rows(area: Rect, buf: &mut Buffer, app: &App, focused: bool) {
    if let Some((primary, detail)) = app.conversation_empty() {
        empty_state(area, buf, app, &primary, detail.as_deref());
        return;
    }
    let lines = app.lines();
    let top = app.pane(Column::Conversation).top;
    let last = lines.len().min(top.saturating_add(usize::from(area.height)));
    let cursor = app.cursor_rows();
    let selected = app.selected_rows();

    for (row_index, index) in (top..last).enumerate() {
        let Some(line) = lines.get(index) else { continue };
        let y = area.y.saturating_add(u16::try_from(row_index).unwrap_or(u16::MAX));
        let row = Rect { y, height: 1, ..area };
        painted(row, buf, line);
        let picked = cursor.as_ref().is_some_and(|rows| rows.contains(&index))
            || selected.as_ref().is_some_and(|rows| rows.contains(&index));
        band(row, buf, app, picked, focused);
    }
}

fn band(area: Rect, buf: &mut Buffer, app: &App, picked: bool, focused: bool) {
    if !picked {
        return;
    }
    let element = if focused { Element::Selection } else { Element::CursorLine };
    buf.set_style(area, app.theme().style(element));
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

fn text_and_info(area: Rect, buf: &mut Buffer, text: &str, info: &str, app: &App, text_element: Element) {
    let x = area.x.saturating_add(2);
    let info_width = INFO_WIDTH.min(area.width);
    let text_width = area.width.saturating_sub(2).saturating_sub(info_width);
    row(Rect { width: text_width, ..area }, buf, x, text, app.theme().style(text_element));

    let info_x = area.right().saturating_sub(u16::try_from(info.width()).unwrap_or(info_width).min(info_width));
    row(area, buf, info_x, info, app.theme().style(Element::Hint));
}

fn empty_state(area: Rect, buf: &mut Buffer, app: &App, primary: &str, detail: Option<&str>) {
    let lines = empty::lines(primary, detail, usize::from(area.width), app.theme());
    for (row_index, line) in lines.iter().enumerate() {
        let Ok(row_index) = u16::try_from(row_index) else { continue };
        let y = area.y.saturating_add(row_index);
        if y >= area.bottom() {
            break;
        }
        painted(Rect { y, height: 1, ..area }, buf, line);
    }
}

fn timestamp_of(at: std::time::SystemTime) -> jiff::Timestamp {
    jiff::Timestamp::try_from(at).unwrap_or(jiff::Timestamp::UNIX_EPOCH)
}

fn statusbar(area: Rect, buf: &mut Buffer, app: &App) {
    let area = padded(area);
    if let Some(notice) = app.notice() {
        let error = notice.starts_with("clipboard unavailable");
        let element = if error { Element::StatusError } else { Element::StatusNotice };
        row(area, buf, area.x, notice, app.theme().style(element));
        return;
    }
    if let Some(status) = app.filter_status() {
        row(area, buf, area.x, &status, app.theme().style(Element::Status));
        return;
    }
    if let Some(status) = app.subagent_status() {
        row(area, buf, area.x, &status, app.theme().style(Element::Status));
        let used = status.width().saturating_add(unreadable(area, buf, app, status.width()));
        indexing(area, buf, app, used);
        return;
    }
    if app.focused() == Column::Projects
        && let Some(project) = app.selected_project()
        && !project.present
        && project.resolution != Resolution::Unresolved
    {
        let text = format!("{GONE} {}", project.path.display());
        row(area, buf, area.x, &text, app.theme().style(Element::ProjectMissing));
        return;
    }
    let Some(session) = app.selected_session() else { return };
    let branch = session.git_branch.as_deref().unwrap_or("-");
    let plural = if session.messages == 1 { "msg" } else { "msgs" };
    let age = session.last_activity.map_or_else(|| "-".to_owned(), |at| age::relative(app.ctx.now, at));
    let text = format!("{} · {} {plural} · {branch} · {age}", session.id, session.messages);
    row(area, buf, area.x, &text, app.theme().style(Element::Status));
    let used = text.width().saturating_add(unreadable(area, buf, app, text.width()));
    indexing(area, buf, app, used);
}

fn unreadable(area: Rect, buf: &mut Buffer, app: &App, used: usize) -> usize {
    let count = app.unreadable();
    if count == 0 {
        return 0;
    }
    let text = format!("{SEPARATOR}{count} unreadable");
    let x = area.x.saturating_add(u16::try_from(used).unwrap_or(area.width));
    row(area, buf, x, &text, app.theme().style(Element::StatusNotice));
    text.width()
}

fn indexing(area: Rect, buf: &mut Buffer, app: &App, used: usize) {
    let Some((done, total)) = app.scan_status() else { return };
    let text = format!("{SEPARATOR}indexing {done}/{total}");
    let x = area.x.saturating_add(u16::try_from(used).unwrap_or(area.width));
    row(area, buf, x, &text, app.theme().style(Element::Hint));
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::SystemTime;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Size;

    use super::*;
    use crate::ctx::Ctx;
    use crate::domain::diagnostics::{Defect, Diagnostics};
    use crate::domain::live::{Live, Status};
    use crate::domain::project::{Project, ProjectError, Resolution};
    use crate::domain::session::{Session, TitleSource};
    use crate::ui::Options;
    use crate::ui::app::{Mode, Pane};
    use crate::ui::input::{Action, CopyTarget, Motion};

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

    fn unresolved_project(directory: &str) -> Project {
        Project {
            directory: directory.to_owned(),
            path: PathBuf::from(directory),
            resolution: Resolution::Unresolved,
            sessions: 0,
            last_activity: SystemTime::UNIX_EPOCH,
            present: false,
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
            diagnostics: Diagnostics::new(Path::new("session.jsonl")),
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
    fn an_unresolved_project_is_not_claimed_gone() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![unresolved_project("jeffries-tube")]));
        let buffer = frame(&app, Size::new(120, 24));
        let row = text_row(&buffer, 3);
        assert!(
            !row.starts_with(GONE),
            "an unresolved project's path is the raw directory name, not proof it went missing: {row}"
        );
    }

    #[test]
    fn the_status_bar_shows_the_full_path_of_a_gone_project_when_projects_is_focused() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("nomad", false)]));
        let buffer = frame(&app, Size::new(120, 24));
        let status = text_row(&buffer, 23);
        assert!(status.contains(GONE), "{status}");
        assert!(status.contains("/Users/fixture/nomad"), "{status}");
    }

    #[test]
    fn no_projects_at_all_explains_rather_than_shows_a_blank_pane() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(Vec::new()));
        let buffer = frame(&app, Size::new(120, 24));
        let row = text_row(&buffer, 3);
        assert!(row.contains("No projects here yet"), "{row}");
    }

    #[test]
    fn a_still_loading_project_list_says_so_rather_than_showing_nothing() {
        let app = app(Size::new(120, 24));
        let buffer = frame(&app, Size::new(120, 24));
        let row = text_row(&buffer, 3);
        assert!(row.contains("Reading"), "{row}");
        assert!(row.contains("/tmp"), "{row}");
    }

    #[test]
    fn a_failed_project_scan_shows_its_error_instead_of_a_blank_pane() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Err(ProjectError::Unreadable(PathBuf::from("/tmp/projects"))));
        let buffer = frame(&app, Size::new(120, 24));
        let row = text_row(&buffer, 3);
        assert!(row.contains("cannot read"), "{row}");
        assert!(row.contains("/tmp/projects"), "{row}");
    }

    #[test]
    fn a_project_with_no_transcripts_names_itself_in_the_sessions_pane() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("shuttlebay", true)]));
        let generation = app.generation();
        app.set_sessions(generation, Vec::new());
        let buffer = frame(&app, Size::new(120, 24));
        let row = text_row(&buffer, 3);
        assert!(row.contains("No transcripts"), "{row}");
        let detail = text_row(&buffer, 4);
        assert!(detail.contains("/Users/fixture/shuttlebay"), "{detail}");
    }

    #[test]
    fn no_session_selected_asks_you_to_pick_one() {
        let app = app(Size::new(120, 24));
        let buffer = frame(&app, Size::new(120, 24));
        let row = text_row(&buffer, 3);
        assert!(row.contains("Pick a session"), "{row}");
    }

    #[test]
    fn a_session_that_parsed_to_nothing_says_so_and_points_at_diagnostics() {
        use std::fs;

        use tempfile::TempDir;

        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true)]));
        app.set_sessions(app.generation(), vec![session("s1", "an empty session")]);

        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("session.jsonl");
        fs::write(&path, b"").expect("a written empty transcript");
        let conversation = crate::domain::thread::build(&path).expect("an empty conversation still builds");
        let generation = app.conversation_generation();
        app.set_conversation(generation, Ok(Arc::new(conversation)), crate::domain::subagent::Agents::default());
        app.reflow();

        let buffer = frame(&app, Size::new(120, 24));
        let row = text_row(&buffer, 3);
        assert!(row.contains("Nothing readable"), "{row}");
        let detail = text_row(&buffer, 4);
        assert!(detail.contains("D lists"), "{detail}");
    }

    #[test]
    fn a_locked_filter_matching_nothing_explains_rather_than_shows_a_blank_pane() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("aaa", true), project("bbb", true)]));
        app.apply(Action::ToggleFilter);
        for character in "zzz".chars() {
            app.apply(Action::Type(character));
        }
        app.apply(Action::Descend);

        let buffer = frame(&app, Size::new(120, 24));
        let row = text_row(&buffer, 3);
        assert!(row.contains("No rows match \"zzz\""), "{row}");
    }

    #[test]
    fn the_focused_column_selection_carries_the_chevron() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true), project("b", true)]));
        let buffer = frame(&app, Size::new(120, 24));
        let row = text_row(&buffer, 3);
        assert!(row.starts_with(CHEVRON), "{row}");
    }

    fn live(session_id: &str, status: Status, waiting_for: Option<&str>) -> Live {
        Live { pid: 4101, session_id: session_id.to_owned(), status, waiting_for: waiting_for.map(str::to_owned), name: None }
    }

    #[test]
    fn a_live_session_carries_the_dot_and_its_status() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true)]));
        app.set_sessions(app.generation(), vec![session("s1", "a session")]);
        app.set_live(vec![live("s1", Status::Busy, None)]);

        let buffer = frame(&app, Size::new(120, 24));
        let sessions_x = columns::placement(120, Mode::Browse)
            .into_iter()
            .find_map(|(column, x, _)| (column == Column::Sessions).then_some(x))
            .expect("a sessions column");
        let row = text_row(&buffer, 3);
        assert!(row.contains(DOT), "{row}");
        assert!(row.contains("busy"), "{row}");
        let dot_style = app.theme().style(Element::SessionLive).fg.unwrap_or_default();
        assert_eq!(buffer[(sessions_x, 3)].fg, dot_style, "the dot did not take the live style");
    }

    #[test]
    fn a_session_with_no_live_entry_shows_no_dot() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true)]));
        app.set_sessions(app.generation(), vec![session("s1", "a session")]);

        let buffer = frame(&app, Size::new(120, 24));
        let row = text_row(&buffer, 3);
        assert!(!row.contains(DOT), "{row}");
    }

    #[test]
    fn waiting_for_input_reads_as_such_rather_than_a_generic_status() {
        let mut app = app(Size::new(220, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true)]));
        app.set_sessions(app.generation(), vec![session("s1", "a session")]);
        app.set_live(vec![live("s1", Status::Idle, Some("input"))]);

        let buffer = frame(&app, Size::new(220, 24));
        let row = text_row(&buffer, 3);
        assert!(row.contains("waiting for input"), "{row}");
    }

    #[test]
    fn a_dead_pids_session_never_carries_a_badge() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true)]));
        app.set_sessions(app.generation(), vec![session("s1", "a session")]);
        app.set_live(vec![live("some-other-session", Status::Busy, None)]);

        let buffer = frame(&app, Size::new(120, 24));
        let row = text_row(&buffer, 3);
        assert!(!row.contains(DOT), "{row}");
    }

    #[test]
    fn a_locked_filter_hides_non_matching_rows_rather_than_only_jumping_to_them() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("aaa", true), project("bbb", true), project("grid-scanner", true)]));
        app.apply(Action::ToggleFilter);
        for character in "grd".chars() {
            app.apply(Action::Type(character));
        }
        app.apply(Action::Descend);

        let buffer = frame(&app, Size::new(120, 24));
        assert!(text_row(&buffer, 3).contains("grid-scanner"), "{}", text_row(&buffer, 3));
        let second_row = text_row(&buffer, 4);
        assert!(!second_row.contains("aaa") && !second_row.contains("bbb"), "no other row should render: {second_row}");
    }

    #[test]
    fn clicking_a_filtered_row_selects_the_underlying_item_not_its_screen_position() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(
            app.generation(),
            Ok(vec![project("aaa", true), project("grid-one", true), project("bbb", true), project("grid-two", true)]),
        );
        app.apply(Action::ToggleFilter);
        for character in "grid".chars() {
            app.apply(Action::Type(character));
        }
        app.apply(Action::Descend);

        app.apply(Action::Click { column: Column::Projects, row: 1 });
        assert_eq!(app.selected_project().map(|project| project.directory.as_str()), Some("grid-two"));
    }

    #[test]
    fn escape_on_a_locked_filter_restores_every_row() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("aaa", true), project("bbb", true), project("grid-scanner", true)]));
        app.apply(Action::ToggleFilter);
        app.apply(Action::Type('g'));
        app.apply(Action::Descend);
        app.apply(Action::Ascend);

        let buffer = frame(&app, Size::new(120, 24));
        assert!(text_row(&buffer, 3).contains("aaa"), "{}", text_row(&buffer, 3));
        assert!(text_row(&buffer, 4).contains("bbb"), "{}", text_row(&buffer, 4));
        assert!(text_row(&buffer, 5).contains("grid-scanner"), "{}", text_row(&buffer, 5));
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
    fn a_copy_notice_takes_the_status_row_and_the_next_action_restores_the_session_line() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true)]));
        let generation = app.generation();
        app.set_sessions(generation, vec![session("s1", "hello there")]);

        app.apply(Action::Copy(CopyTarget::Message));
        let buffer = frame(&app, Size::new(120, 24));
        let status = text_row(&buffer, 23);
        assert!(status.contains("nothing to copy"), "{status}");

        app.apply(Action::ToggleInjections);
        let buffer = frame(&app, Size::new(120, 24));
        let status = text_row(&buffer, 23);
        assert!(status.contains("s1"), "{status}");
    }

    #[test]
    fn e_opens_an_overlay_carrying_the_format_the_path_and_a_hint() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true)]));
        app.set_sessions(app.generation(), vec![session("s1", "a session")]);
        with_conversation(&mut app, "read the grid scanner back to me");

        app.apply(Action::ToggleExport);
        let buffer = frame(&app, Size::new(120, 24));
        let rows: Vec<String> = (0..24).map(|y| text_row(&buffer, y)).collect();
        assert!(rows.iter().any(|row| row.contains("Export")), "{rows:?}");
        assert!(rows.iter().any(|row| row.contains("Markdown")), "{rows:?}");
        assert!(rows.iter().any(|row| row.contains("Enter to write")), "{rows:?}");
    }

    fn drifting_session(id: &str) -> Session {
        let mut session = session(id, "a session that will not read cleanly");
        let mut diagnostics = Diagnostics::new(&session.path);
        diagnostics.push(Defect::UnknownBlock { line: 12, kind: "server_tool_use".to_owned() });
        diagnostics.push(Defect::UnknownRecord { line: 7, kind: "telemetry-latch".to_owned() });
        diagnostics.push(Defect::Truncated { line: 31 });
        session.diagnostics = diagnostics;
        session
    }

    #[test]
    fn the_statusbar_ends_with_what_could_not_be_read() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true)]));
        let generation = app.generation();
        app.set_sessions(generation, vec![drifting_session("s1")]);
        let buffer = frame(&app, Size::new(120, 24));
        let status = text_row(&buffer, 23);
        assert!(status.ends_with("· 3 unreadable"), "{status}");
        let notice = app.theme().style(Element::StatusNotice).fg.unwrap_or_default();
        let tinted = (0..120).filter(|&x| buffer[(x, 23)].fg == notice).count();
        assert!(tinted > 0, "the count is styled apart from the rest of the line");
    }

    #[test]
    fn a_session_that_reads_cleanly_says_nothing_about_being_unreadable() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true)]));
        let generation = app.generation();
        app.set_sessions(generation, vec![session("s1", "a session")]);
        let buffer = frame(&app, Size::new(120, 24));
        let status = text_row(&buffer, 23);
        assert!(!status.contains("unreadable"), "{status}");
    }

    #[test]
    fn a_scan_in_progress_shows_an_indexing_segment_at_the_end_of_the_status_line() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true)]));
        let generation = app.generation();
        app.set_sessions(generation, vec![session("s1", "a session")]);
        app.set_scan_progress(3, 10);
        let buffer = frame(&app, Size::new(120, 24));
        let status = text_row(&buffer, 23);
        assert!(status.ends_with("· indexing 3/10"), "{status}");
    }

    #[test]
    fn a_finished_scan_says_nothing_about_indexing() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true)]));
        let generation = app.generation();
        app.set_sessions(generation, vec![session("s1", "a session")]);
        app.set_scan_progress(10, 10);
        app.scan_finished();
        let buffer = frame(&app, Size::new(120, 24));
        let status = text_row(&buffer, 23);
        assert!(!status.contains("indexing"), "{status}");
    }

    fn screen(buffer: &Buffer) -> String {
        (0..buffer.area.height).map(|y| text_row(buffer, y)).collect::<Vec<String>>().join("\n")
    }

    #[test]
    fn d_opens_the_diagnostics_over_the_columns_and_escape_closes_it() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true)]));
        let generation = app.generation();
        app.set_sessions(generation, vec![drifting_session("s1")]);

        app.apply(Action::ToggleDiagnostics);
        let opened = screen(&frame(&app, Size::new(120, 24)));
        assert!(opened.contains("Diagnostics"), "{opened}");
        assert!(opened.contains("server_tool_use"), "{opened}");
        assert!(opened.contains("telemetry-latch"), "{opened}");

        app.apply(Action::Ascend);
        let closed = screen(&frame(&app, Size::new(120, 24)));
        assert!(!closed.contains("server_tool_use"), "{closed}");
    }

    #[test]
    fn the_diagnostics_open_over_a_column_that_has_loaded_nothing_at_all() {
        let mut app = app(Size::new(120, 24));
        app.apply(Action::ToggleDiagnostics);
        let opened = screen(&frame(&app, Size::new(120, 24)));
        assert!(opened.contains("Nothing unreadable"), "{opened}");
    }

    #[test]
    fn a_key_the_diagnostics_do_not_use_does_not_reach_the_view_underneath() {
        let mut app = app(Size::new(120, 24));
        app.apply(Action::ToggleDiagnostics);
        app.apply(Action::ToggleFocusMode);
        assert_eq!(app.mode(), Mode::Browse, "focus mode must not toggle behind the overlay");
    }

    #[test]
    fn question_mark_opens_the_help_overlay_and_escape_closes_it() {
        let mut app = app(Size::new(120, 24));
        app.apply(Action::ToggleHelp);
        let opened = screen(&frame(&app, Size::new(120, 24)));
        assert!(opened.contains("Keys"), "{opened}");
        assert!(opened.contains("move between columns"), "{opened}");

        app.apply(Action::Ascend);
        let closed = screen(&frame(&app, Size::new(120, 24)));
        assert!(!closed.contains("move between columns"), "{closed}");
    }

    #[test]
    fn a_key_the_help_overlay_does_not_use_does_not_reach_the_view_underneath() {
        let mut app = app(Size::new(120, 24));
        app.apply(Action::ToggleHelp);
        app.apply(Action::ToggleFocusMode);
        assert_eq!(app.mode(), Mode::Browse, "focus mode must not toggle behind the overlay");
    }

    #[test]
    fn the_column_dividers_are_the_same_hairline_colour_as_the_rules_around_them() {
        let app = app(Size::new(120, 24));
        let buffer = frame(&app, Size::new(120, 24));
        let hairline = app.theme().style(Element::Hint).fg.expect("the hint style names a foreground");

        let dividers: Vec<u16> = (0..120).filter(|&x| buffer[(x, 3)].symbol() == symbols::line::VERTICAL).collect();
        assert_eq!(dividers.len(), 2, "three columns should be split by two dividers: {dividers:?}");
        for x in dividers {
            assert_eq!(buffer[(x, 3)].fg, hairline, "the divider at {x} is not the hairline colour");
            assert_ne!(buffer[(x, 3)].fg, ratatui::style::Color::Reset, "the divider at {x} took the terminal's foreground");
        }
        assert_eq!(buffer[(0, 1)].fg, hairline, "the top rule is no longer the hairline colour");
    }

    #[test]
    fn the_top_rule_is_untinted_with_nothing_loaded() {
        let app = app(Size::new(120, 24));
        let buffer = frame(&app, Size::new(120, 24));
        let progress = app.theme().style(Element::ScrollProgress).fg.unwrap_or_default();
        let tinted = (0..120).filter(|&x| buffer[(x, 1)].fg == progress).count();
        assert_eq!(tinted, 0);
    }

    #[test]
    fn scrolling_tints_the_top_rule() {
        let mut app = app(Size::new(120, 10));
        app.set_projects(app.generation(), Ok((0..50).map(|index| project(&format!("p{index}"), true)).collect()));
        app.apply(Action::Move(Motion::Bottom));
        let buffer = frame(&app, Size::new(120, 10));
        let progress = app.theme().style(Element::ScrollProgress).fg.unwrap_or_default();
        let tinted = (0..120).filter(|&x| buffer[(x, 1)].fg == progress).count();
        assert!(tinted > 0);
    }

    fn with_tool_call(app: &mut App) {
        use std::fs;

        use tempfile::TempDir;

        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("session.jsonl");
        let user = r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:00Z","origin":{"kind":"human"},"message":{"role":"user","content":"scan the grid"}}"#;
        let assistant = r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"s","isSidechain":false,"timestamp":"2026-01-01T00:00:01Z","requestId":"r1","message":{"id":"m1","role":"assistant","model":"opus-5","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/grid.rs"}}]}}"#;
        fs::write(&path, format!("{user}\n{assistant}\n")).expect("a written transcript");
        let conversation = crate::domain::thread::build(&path).expect("a built conversation");
        let generation = app.conversation_generation();
        app.set_conversation(generation, Ok(Arc::new(conversation)), crate::domain::subagent::Agents::default());
        app.reflow();
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
        app.set_conversation(generation, Ok(Arc::new(conversation)), crate::domain::subagent::Agents::default());
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
    fn scrolled_to_the_bottom_the_conversation_fills_every_row_it_is_given() {
        let mut app = app(Size::new(60, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true)]));
        app.set_sessions(app.generation(), vec![session("s1", "a session")]);
        app.apply(Action::ToggleFocusMode);
        with_conversation(&mut app, &"prose ".repeat(200));

        app.apply(Action::Move(Motion::Bottom));
        let buffer = frame(&app, Size::new(60, 24));
        let last_row = buffer.area.height.saturating_sub(3);
        assert!(!text_row(&buffer, last_row).is_empty(), "the last content row went unused after scrolling to the end");
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
    fn the_focused_column_header_takes_the_active_title_colour() {
        let app = app(Size::new(120, 24));
        let buffer = frame(&app, Size::new(120, 24));
        let active = app.theme().style(Element::ColumnTitleActive).fg.unwrap_or_default();
        let resting = app.theme().style(Element::ColumnTitle).fg.unwrap_or_default();
        assert_ne!(active, resting, "the two title colours must differ or the cue says nothing");

        for (column, x, _) in columns::placement(120, Mode::Browse) {
            let want = if column == Column::Projects { active } else { resting };
            assert_eq!(buffer[(x, 2)].fg, want, "the {column:?} header at {x}");
        }
    }

    #[test]
    fn an_unfocused_column_header_recedes_when_the_focus_moves_on() {
        let mut app = app(Size::new(120, 24));
        app.apply(Action::Focus { forward: true });
        let buffer = frame(&app, Size::new(120, 24));
        let active = app.theme().style(Element::ColumnTitleActive).fg.unwrap_or_default();
        let resting = app.theme().style(Element::ColumnTitle).fg.unwrap_or_default();

        for (column, x, _) in columns::placement(120, Mode::Browse) {
            let want = if column == Column::Sessions { active } else { resting };
            assert_eq!(buffer[(x, 2)].fg, want, "the {column:?} header at {x}");
        }
    }

    #[test]
    fn the_focused_selection_band_is_stronger_than_an_unfocused_one() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true), project("b", true)]));
        app.set_sessions(app.generation(), vec![session("s1", "a session"), session("s2", "another")]);

        let selection = app.theme().style(Element::Selection).bg.unwrap_or_default();
        let cursor = app.theme().style(Element::CursorLine).bg.unwrap_or_default();
        assert_ne!(selection, cursor, "the two bands must differ or the cue says nothing");

        let sessions_x = columns::placement(120, Mode::Browse)
            .into_iter()
            .find_map(|(column, x, _)| (column == Column::Sessions).then_some(x))
            .expect("a sessions column");

        let buffer = frame(&app, Size::new(120, 24));
        assert_eq!(buffer[(0, 3)].bg, selection, "the focused projects row");
        assert_eq!(buffer[(sessions_x, 3)].bg, cursor, "the unfocused sessions row");

        app.apply(Action::Focus { forward: true });
        let buffer = frame(&app, Size::new(120, 24));
        assert_eq!(buffer[(0, 3)].bg, cursor, "the projects row once the focus left it");
        assert_eq!(buffer[(sessions_x, 3)].bg, selection, "the sessions row once the focus arrived");
    }

    #[test]
    fn the_focused_band_repaints_the_whole_row_so_nothing_reads_as_muted_against_it() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true), project("b", true)]));
        let buffer = frame(&app, Size::new(120, 24));
        let selection = app.theme().style(Element::Selection);

        let width = columns::placement(120, Mode::Browse)
            .into_iter()
            .find_map(|(column, _, width)| (column == Column::Projects).then_some(width))
            .expect("a projects column");
        for x in 0..width {
            assert_eq!(buffer[(x, 3)].bg, selection.bg.unwrap_or_default(), "the band stops short at {x}");
            assert_eq!(buffer[(x, 3)].fg, selection.fg.unwrap_or_default(), "the age and count keep the hint colour at {x}");
        }
    }

    #[test]
    fn the_conversation_cursor_line_takes_the_focused_band() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true)]));
        app.set_sessions(app.generation(), vec![session("s1", "a session")]);
        with_tool_call(&mut app);
        app.apply(Action::Focus { forward: true });
        app.apply(Action::Focus { forward: true });
        app.apply(Action::NextCall { forward: true });
        assert!(app.cursor_line().is_some(), "the call cursor never landed on a tool call");

        let x = columns::placement(120, Mode::Browse)
            .into_iter()
            .find_map(|(column, x, _)| (column == Column::Conversation).then_some(x))
            .expect("a conversation column");
        let y = 3 + u16::try_from(app.cursor_line().unwrap_or(0)).unwrap_or(0);

        let buffer = frame(&app, Size::new(120, 24));
        let selection = app.theme().style(Element::Selection).bg.unwrap_or_default();
        assert_eq!(buffer[(x, y)].bg, selection, "the focused conversation cursor line");
        assert_eq!(buffer[(x, y + 1)].bg, selection, "the band covers the digest row too");

        app.apply(Action::Focus { forward: false });
        let buffer = frame(&app, Size::new(120, 24));
        let cursor = app.theme().style(Element::CursorLine).bg.unwrap_or_default();
        assert_eq!(buffer[(x, y)].bg, cursor, "the conversation cursor line once the focus left it");
    }

    #[test]
    fn a_drag_in_the_conversation_bands_every_row_it_spans() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true)]));
        app.set_sessions(app.generation(), vec![session("s1", "a session")]);
        with_conversation(&mut app, "read the grid scanner back to me");

        let x = columns::placement(120, Mode::Browse)
            .into_iter()
            .find_map(|(column, x, _)| (column == Column::Conversation).then_some(x))
            .expect("a conversation column");

        app.apply(Action::Click { column: Column::Conversation, row: 0 });
        app.apply(Action::Drag { column: Column::Conversation, row: 1 });

        let buffer = frame(&app, Size::new(120, 24));
        let selection = app.theme().style(Element::Selection).bg.unwrap_or_default();
        assert_eq!(buffer[(x, 3)].bg, selection, "the first row of the drag");
        assert_eq!(buffer[(x, 4)].bg, selection, "the second row of the drag");

        app.apply(Action::Release);
        let buffer = frame(&app, Size::new(120, 24));
        assert_ne!(buffer[(x, 3)].bg, selection, "the band clears once the selection is copied");
    }

    #[test]
    fn a_selected_row_outside_focus_carries_the_band_but_not_the_chevron() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true), project("b", true)]));
        app.apply(Action::Focus { forward: true });
        let buffer = frame(&app, Size::new(120, 24));
        let row = text_row(&buffer, 3);
        assert!(!row.starts_with(CHEVRON), "{row}");
        let cursor = app.theme().style(Element::CursorLine).bg.unwrap_or_default();
        assert_eq!(buffer[(0, 3)].bg, cursor);
    }

    fn pane_of(app: &App, column: Column) -> Pane {
        app.pane(column)
    }

    #[test]
    fn the_pane_accessor_matches_the_focused_column() {
        let app = app(Size::new(120, 24));
        assert_eq!(pane_of(&app, Column::Projects), Pane::default());
    }

    fn joined(segments: &[String], room: usize) -> String {
        elided(segments, room).join(SEPARATOR)
    }

    #[test]
    fn a_breadcrumb_that_fits_keeps_every_segment() {
        let segments = ["rewind".to_owned(), "holodeck".to_owned(), "The tool surface".to_owned(), "Explore".to_owned()];
        assert_eq!(joined(&segments, 60), "rewind · holodeck · The tool surface · Explore");
    }

    #[test]
    fn a_breadcrumb_too_wide_elides_from_the_left_so_the_deepest_segment_survives() {
        let segments = ["rewind".to_owned(), "holodeck".to_owned(), "The tool surface".to_owned(), "code-review".to_owned()];
        let elided = joined(&segments, 34);
        assert!(elided.starts_with("…"), "{elided:?}");
        assert!(elided.ends_with("code-review"), "the segment being read is the one that must not go: {elided:?}");
        assert!(elided.width() <= 34);
    }

    #[test]
    fn a_breadcrumb_with_no_room_at_all_keeps_a_truncated_last_segment() {
        let segments = ["rewind".to_owned(), "holodeck".to_owned(), "code-review".to_owned()];
        let elided = joined(&segments, 6);
        assert!(elided.width() <= 6, "{elided:?}");
        assert!(!elided.is_empty());
    }

    #[test]
    fn only_the_name_at_the_head_of_the_breadcrumb_carries_the_accent() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("holodeck", true)]));
        let buffer = frame(&app, Size::new(120, 24));

        let name = app.theme().style(Element::HeaderTitle).fg.expect("the header title names a foreground");
        let quiet = app.theme().style(Element::Status).fg.expect("the status style names a foreground");
        let hairline = app.theme().style(Element::Hint).fg.expect("the hint style names a foreground");
        assert_ne!(name, quiet, "the two must differ or the accent says nothing");
        assert_ne!(quiet, hairline, "the trail must read above the hairlines, not with them");

        let row = text_row(&buffer, 0);
        let crumb = " rewind · /Users/fixture/holodeck";
        assert!(row.starts_with(crumb), "{row}");
        for x in 1..7 {
            assert_eq!(buffer[(x, 0)].fg, name, "the name at {x} is not the accent");
        }
        for x in 7..u16::try_from(crumb.width()).unwrap_or(0) {
            assert_eq!(buffer[(x, 0)].fg, quiet, "the trail at {x} did not recede");
        }
    }

    #[test]
    fn the_status_line_sits_a_step_below_the_rows_it_describes() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("holodeck", true)]));
        let generation = app.generation();
        app.set_sessions(generation, vec![session("s1", "a session")]);
        let buffer = frame(&app, Size::new(120, 24));

        let status = app.theme().style(Element::Status).fg.expect("the status style names a foreground");
        let body = app.theme().style(Element::Body).fg.unwrap_or_default();
        assert_ne!(status, body, "the status line reads at the same weight as a project name");
        assert_eq!(buffer[(1, 23)].fg, status, "the status line is not the status colour");
    }

    #[test]
    fn the_breadcrumb_trail_and_the_status_line_read_at_one_weight() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("holodeck", true)]));
        let generation = app.generation();
        app.set_sessions(generation, vec![session("s1", "a session")]);
        let buffer = frame(&app, Size::new(120, 24));

        let trail = buffer[(10, 0)].fg;
        assert_eq!(buffer[(10, 0)].symbol(), "/", "the cell sampled is not the start of the project path");
        assert_eq!(trail, buffer[(1, 23)].fg, "the top bar and the status bar drifted apart");
    }

    #[test]
    fn a_project_name_keeps_the_body_colour_while_its_age_stays_a_hint() {
        let mut app = app(Size::new(120, 24));
        app.set_projects(app.generation(), Ok(vec![project("a", true), project("holodeck", true)]));
        let buffer = frame(&app, Size::new(120, 24));

        let body = app.theme().style(Element::Body).fg.unwrap_or_default();
        let quiet = app.theme().style(Element::Hint).fg.unwrap_or_default();
        assert_eq!(buffer[(2, 4)].fg, body, "the unselected project name left the body colour");
        assert_eq!(buffer[(2, 4)].bg, ratatui::style::Color::Reset, "row 4 should be the unselected project");
        assert_ne!(body, quiet);
    }
}
