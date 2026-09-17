//! Wiring: parse the command line, resolve the directories, hand over to the browser.

mod cli;
mod paths;

use anyhow::Result;
use clap::Parser;

use crate::cli::Cli;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let claude = paths::claude_dir(cli.claude_dir)?;

    println!("rewind {}", env!("CARGO_PKG_VERSION"));
    println!("claude  {}", claude.display());

    Ok(())
}
