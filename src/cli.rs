//! The command line, as clap sees it.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};

#[derive(Debug, Parser)]
#[allow(clippy::struct_excessive_bools)]
#[command(name = "rewind", version, about, long_about = None)]
pub struct Cli {
    #[arg(value_name = "PROJECT", help = "Open this project first")]
    pub project: Option<String>,

    #[arg(long, value_name = "ID", help = "Open this session directly")]
    pub session: Option<String>,

    #[arg(long, value_name = "NAME", help = "Theme by name")]
    pub theme: Option<String>,

    #[arg(long, value_name = "FILE", help = "Explicit theme file")]
    pub config: Option<PathBuf>,

    #[arg(long, help = "List built-in and user themes, then exit")]
    pub list_themes: bool,

    #[arg(long, value_name = "DIR", help = "Override the Claude Code directory")]
    pub claude_dir: Option<PathBuf>,

    #[arg(long, help = "Disable mouse capture")]
    pub no_mouse: bool,

    #[arg(long, help = "Ignore and do not write the cache")]
    pub no_cache: bool,

    #[arg(long, help = "Discard the cache and rebuild, then exit")]
    pub rebuild_cache: bool,

    #[arg(long, value_name = "WHEN", value_enum, default_value_t = ColorChoice::Auto, help = "ANSI colors")]
    pub color: ColorChoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorChoice {
    #[value(help = "Color when the output is a terminal")]
    Auto,
    #[value(help = "Always color")]
    Always,
    #[value(help = "Never color")]
    Never,
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_command_definition_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn defaults_are_inert() {
        let cli = Cli::parse_from(["rewind"]);
        assert!(cli.project.is_none());
        assert!(cli.session.is_none());
        assert!(!cli.no_mouse);
        assert!(!cli.no_cache);
        assert_eq!(cli.color, ColorChoice::Auto);
    }

    #[test]
    fn every_flag_parses() {
        let cli = Cli::parse_from([
            "rewind",
            "vademecum",
            "--session",
            "abc",
            "--theme",
            "nord",
            "--config",
            "/tmp/t.toml",
            "--list-themes",
            "--claude-dir",
            "/tmp/claude",
            "--no-mouse",
            "--no-cache",
            "--rebuild-cache",
            "--color",
            "never",
        ]);
        assert_eq!(cli.project.as_deref(), Some("vademecum"));
        assert_eq!(cli.session.as_deref(), Some("abc"));
        assert_eq!(cli.theme.as_deref(), Some("nord"));
        assert!(cli.list_themes);
        assert!(cli.rebuild_cache);
        assert_eq!(cli.color, ColorChoice::Never);
    }
}
