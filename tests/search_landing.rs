//! A hit whose match is inside tool output opens with that call unfolded, so the term
//! the reader searched for is on a line they can see — the whole point of asking for `is:tool`.

#![allow(clippy::expect_used)]

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::fixture_tree;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Size;
use rewind::ctx::Ctx;
use rewind::domain::cache::shard::{Field, Kind};
use rewind::domain::cache::store;
use rewind::domain::project;
use rewind::domain::search::{corpus, engine, query, resolve};
use rewind::domain::session;
use rewind::domain::subagent::Agents;
use rewind::domain::thread;
use rewind::ui::app::{App, Column, DEBOUNCE};
use rewind::ui::{Options, columns, view};

const TERM: &str = "Replicated";
const SESSION: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";

fn frame_text(app: &App, size: Size) -> String {
    let mut terminal = Terminal::new(TestBackend::new(size.width, size.height)).expect("a test terminal");
    terminal.draw(|frame| view::draw(frame, app)).expect("a drawn frame");
    let buffer = terminal.backend().buffer().clone();
    let mut text = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    text
}

#[test]
fn a_tool_output_hit_opens_with_the_call_unfolded_and_the_term_on_screen() {
    let tree = fixture_tree();
    let cache = tempfile::TempDir::new().expect("a temporary cache directory");
    let claude_dir = tree.claude_dir();

    let report = store::rebuild(&claude_dir, cache.path());
    assert!(report.failures.is_empty(), "{:?}", report.failures);

    let corpus = corpus::load(cache.path());
    let parsed = query::parse(&format!("is:tool {}", TERM.to_lowercase()));
    let hits = engine::search(&corpus, &parsed, 0);
    let hit = hits
        .iter()
        .find(|hit| hit.kind == Kind::Transcript && hit.field == Field::ToolResult && hit.file_name.contains(SESSION))
        .expect("the fixture's replicator result is a tool-output hit");

    let opened = resolve::resolve_transcript(&claude_dir, hit).expect("a resolvable transcript hit");
    assert_eq!(opened.session_id, SESSION);

    let size = Size::new(120, 40);
    let options = Options { claude_dir: claude_dir.clone(), ..Options::default() };
    let mut app = App::new(Ctx { now: jiff::Timestamp::now() }, &options, size);

    let projects = project::discover(&claude_dir).expect("the fixture tree has a projects directory");
    app.set_projects(app.generation(), Ok(projects));
    let _ = app.take_session_load();

    app.set_hit_resolved(app.search_hit_generation(), Some(opened));

    let (directory, _) = app.take_session_load().expect("the hit asked for its project's sessions");
    let sessions = session::discover(&directory);
    app.set_sessions(app.generation(), sessions);

    let (path, generation) = app
        .take_conversation_load(Instant::now().checked_add(DEBOUNCE).unwrap_or_else(Instant::now))
        .expect("the hit asked for its session");
    let conversation = thread::build(&path).expect("a built conversation");
    app.set_conversation(generation, Ok(Arc::new(conversation)), Agents::default());

    let text = frame_text(&app, size);
    assert!(text.contains(TERM), "the frame does not hold the term at all:\n{text}");

    let top = app.pane(Column::Conversation).top;
    let height = columns::conversation_height(size);
    let showing: String = app.lines().iter().skip(top).take(height).map(rewind::render::line::RenderedLine::text).collect();
    assert!(
        showing.contains(TERM),
        "the term is in no conversation line the reader can see — the call it is inside is still folded"
    );
}
