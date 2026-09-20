//! The interactive shell: the terminal's lifetime, and the event loop.

pub mod age;
pub mod app;
pub mod clipboard;
pub mod columns;
pub mod diagnostics;
pub mod export_prompt;
pub mod input;
pub mod listing;
mod lru;
pub mod save;
pub mod search;
pub mod view;
mod worker;

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;

use crate::ctx::Ctx;
use crate::theme::Theme;
use crate::ui::app::App;
use crate::ui::worker::{Job, Wake};

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub claude_dir: PathBuf,
    pub project: Option<String>,
    pub session: Option<String>,
    pub mouse: bool,
    pub theme: Theme,
    pub cache_root: Option<PathBuf>,
    pub no_cache: bool,
}

pub fn run(ctx: Ctx, options: &Options) -> Result<()> {
    if options.mouse {
        release_mouse_on_panic();
    }

    let (tx, rx) = mpsc::channel();
    let (job_tx, job_rx) = mpsc::channel();

    let mut terminal = ratatui::try_init().context("cannot open the terminal")?;
    worker::spawn_input(tx.clone());
    worker::spawn_loader(job_rx, tx.clone());

    let outcome = capture_mouse(options.mouse)
        .and_then(|()| terminal.size().context("cannot measure the terminal"))
        .map(|area| App::new(ctx, options, area))
        .and_then(|mut app| {
            worker::spawn_projects_load(&tx, app.claude_dir().to_path_buf(), app.generation());
            event_loop(&mut terminal, &mut app, &tx, &job_tx, &rx)
        });

    if options.mouse {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
    ratatui::restore();
    outcome
}

fn capture_mouse(wanted: bool) -> Result<()> {
    if wanted {
        execute!(io::stdout(), EnableMouseCapture).context("cannot capture the mouse")?;
    }
    Ok(())
}

fn release_mouse_on_panic() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        hook(info);
    }));
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    tx: &Sender<Wake>,
    job_tx: &Sender<Job>,
    rx: &Receiver<Wake>,
) -> Result<()> {
    loop {
        app.reflow();
        terminal.draw(|frame| view::draw(frame, app)).context("cannot draw")?;

        match next(app, rx) {
            Ok(wake) => handle(app, wake)?,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        while let Ok(wake) = rx.try_recv() {
            handle(app, wake)?;
        }

        if app.quit {
            return Ok(());
        }
        if let Some((dir, generation)) = app.take_session_load() {
            worker::spawn_sessions_load(tx, dir, generation);
        }
        if let Some((path, generation)) = app.take_conversation_load(Instant::now()) {
            let cancel = app.conversation_cancel(generation);
            let _ = job_tx.send(Job::Conversation { path, generation, cancel });
        }
        if let Some((id, path, generation)) = app.take_tool_output() {
            let _ = job_tx.send(Job::ToolOutput { id, path, generation });
        }
        if let Some((_, path, generation)) = app.take_subagent_load() {
            let cancel = app.conversation_cancel(generation);
            let _ = job_tx.send(Job::Subagent { path, generation, cancel });
        }
        if let Some((claude_dir, cache_root, selected, cancel)) = app.take_scan() {
            worker::spawn_scan(tx, claude_dir, cache_root, selected, cancel);
        }
        if let Some(cache_root) = app.take_corpus_load() {
            worker::spawn_corpus_load(tx, cache_root);
        }
        if let Some((hit, generation)) = app.take_hit_resolve() {
            worker::spawn_resolve_hit(tx, app.claude_dir().to_path_buf(), hit, generation);
        }
        if let Some(text) = app.take_copy()
            && clipboard::copy(&text).is_err()
        {
            app.copy_failed();
        }
        if let Some((dest, payload, force)) = app.take_export() {
            let attempt = match payload {
                app::ExportPayload::Text(text) => save::attempt_text(&dest, &text, force),
                app::ExportPayload::CopyFile(source) => save::attempt_copy(&source, &dest, force),
            };
            match attempt {
                save::Attempt::Written(path) => app.export_written(&path),
                save::Attempt::NeedsConfirmation => app.export_needs_confirmation(),
                save::Attempt::Failed(error) => app.export_failed(&error),
            }
        }
    }
}

fn next(app: &App, rx: &Receiver<Wake>) -> Result<Wake, RecvTimeoutError> {
    let Some(due) = app.conversation_due() else {
        return rx.recv().map_err(|_| RecvTimeoutError::Disconnected);
    };
    rx.recv_timeout(due.saturating_duration_since(Instant::now()))
}

fn handle(app: &mut App, wake: Wake) -> Result<()> {
    match wake {
        Wake::Input(event) => {
            let viewport = input::Viewport { area: app.area(), mode: app.mode(), text_entry: app.text_entry() };
            if let Some(action) = input::action(&event, viewport) {
                app.apply(action);
            }
        }
        Wake::ProjectsLoaded { generation, result } => app.set_projects(generation, result),
        Wake::SessionsLoaded { generation, sessions } => app.set_sessions(generation, sessions),
        Wake::ConversationLoaded { generation, result, agents } => app.set_conversation(generation, result, *agents),
        Wake::ToolOutputLoaded { generation, id, result } => app.set_tool_output(generation, id, result),
        Wake::SubagentLoaded { generation, path, result } => app.set_subagent(generation, result, path),
        Wake::ScanProgress { done, total } => app.set_scan_progress(done, total),
        Wake::ScanFinished => app.scan_finished(),
        Wake::ShardReady { generation, entry } => app.set_shard_ready(generation, entry),
        Wake::CorpusLoaded(corpus) => app.set_corpus(corpus),
        Wake::HitResolved { generation, target } => app.set_hit_resolved(generation, target),
        Wake::InputLost(error) => bail!("cannot read keyboard input: {error}"),
    }
    Ok(())
}
