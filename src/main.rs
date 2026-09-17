//! Wiring: parse the command line, resolve the directories, hand over to the browser.

use anyhow::Result;
use clap::Parser;
use rewind::{cli::Cli, paths};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let claude = paths::claude_dir(cli.claude_dir)?;

    println!("rewind {}", env!("CARGO_PKG_VERSION"));
    println!("claude  {}", claude.display());

    Ok(())
}
