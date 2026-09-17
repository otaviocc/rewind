//! The interactive shell: the terminal's lifetime, and the event loop.

pub mod age;
pub mod app;
pub mod columns;
pub mod input;
pub mod listing;
pub mod view;

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use anyhow::{Context, Result, bail};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;

use crate::ctx::Ctx;
use crate::domain::project::{self, Project, ProjectError};
use crate::domain::session;
use crate::ui::app::App;

enum Wake {
    Input(event::Event),
    ProjectsLoaded { generation: u64, result: Result<Vec<Project>, ProjectError> },
    SessionsLoaded { generation: u64, sessions: Vec<session::Session> },
    InputLost(String),
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub claude_dir: PathBuf,
    pub project: Option<String>,
    pub session: Option<String>,
    pub mouse: bool,
}

pub fn run(ctx: Ctx, options: &Options) -> Result<()> {
    if options.mouse {
        release_mouse_on_panic();
    }

    let (tx, rx) = mpsc::channel();

    let mut terminal = ratatui::try_init().context("cannot open the terminal")?;
    spawn_input(tx.clone());

    let outcome = capture_mouse(options.mouse)
        .and_then(|()| terminal.size().context("cannot measure the terminal"))
        .map(|area| App::new(ctx, options, area))
        .and_then(|mut app| {
            spawn_projects_load(&tx, app.claude_dir().to_path_buf(), app.generation());
            event_loop(&mut terminal, &mut app, &tx, &rx)
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

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App, tx: &Sender<Wake>, rx: &Receiver<Wake>) -> Result<()> {
    loop {
        terminal.draw(|frame| view::draw(frame, app)).context("cannot draw")?;

        let Ok(wake) = rx.recv() else { return Ok(()) };
        handle(app, wake)?;
        while let Ok(wake) = rx.try_recv() {
            handle(app, wake)?;
        }

        if app.quit {
            return Ok(());
        }
        if let Some((dir, generation)) = app.take_session_load() {
            spawn_sessions_load(tx, dir, generation);
        }
    }
}

fn handle(app: &mut App, wake: Wake) -> Result<()> {
    match wake {
        Wake::Input(event) => {
            if let Some(action) = input::action(&event) {
                app.apply(action);
            }
        }
        Wake::ProjectsLoaded { generation, result } => app.set_projects(generation, result),
        Wake::SessionsLoaded { generation, sessions } => app.set_sessions(generation, sessions),
        Wake::InputLost(error) => bail!("cannot read keyboard input: {error}"),
    }
    Ok(())
}

fn spawn_input(tx: Sender<Wake>) {
    std::thread::spawn(move || {
        loop {
            let event = match event::read() {
                Ok(event) => event,
                Err(error) => {
                    let _ = tx.send(Wake::InputLost(error.to_string()));
                    return;
                }
            };
            if tx.send(Wake::Input(event)).is_err() {
                return;
            }
        }
    });
}

fn spawn_projects_load(tx: &Sender<Wake>, claude_dir: PathBuf, generation: u64) {
    let tx = tx.clone();
    std::thread::spawn(move || {
        let result = project::discover(&claude_dir);
        let _ = tx.send(Wake::ProjectsLoaded { generation, result });
    });
}

fn spawn_sessions_load(tx: &Sender<Wake>, project_dir: PathBuf, generation: u64) {
    let tx = tx.clone();
    std::thread::spawn(move || {
        let sessions = session::discover(&project_dir);
        let _ = tx.send(Wake::SessionsLoaded { generation, sessions });
    });
}
