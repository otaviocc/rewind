//! Wiring: parse the command line, resolve the directories, hand over to the browser.

use std::io::IsTerminal;
use std::time::SystemTime;

use anyhow::Result;
use clap::Parser;
use jiff::Timestamp;
use rewind::ctx::Ctx;
use rewind::domain::project::{self, Project, Resolution};
use rewind::theme::Theme;
use rewind::ui;
use rewind::{cli::Cli, paths};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let claude = paths::claude_dir(cli.claude_dir)?;

    if std::io::stdout().is_terminal() {
        let options = ui::Options {
            claude_dir: claude,
            project: cli.project,
            session: cli.session,
            mouse: !cli.no_mouse,
            theme: Theme::default(),
        };
        return ui::run(Ctx { now: Timestamp::now() }, &options);
    }

    let projects = project::discover(&claude)?;

    println!("rewind {}", env!("CARGO_PKG_VERSION"));
    println!("claude  {}", claude.display());
    println!();

    let width = projects.iter().map(|project| project.path.display().to_string().chars().count()).max().unwrap_or(0);
    for project in &projects {
        println!("{}", line(project, width));
    }

    Ok(())
}

fn line(project: &Project, width: usize) -> String {
    let path = project.path.display().to_string();
    let padding = " ".repeat(width.saturating_sub(path.chars().count()));
    let plural = if project.sessions == 1 { " " } else { "s" };
    format!("{path}{padding}  {:>9}  {:>4} session{plural}  {}", state(project), project.sessions, day(project.last_activity))
}

const fn state(project: &Project) -> &'static str {
    match project.resolution {
        Resolution::Unresolved => "unknown",
        _ if !project.present => "gone",
        _ => "",
    }
}

fn day(at: SystemTime) -> String {
    Timestamp::try_from(at).map_or_else(|_| "-".to_owned(), |stamp| stamp.strftime("%Y-%m-%d").to_string())
}
