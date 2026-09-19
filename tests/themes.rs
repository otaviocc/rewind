//! The theme flags at the binary's edge: what a reader's config directory does to a run.

#![allow(clippy::expect_used)]

mod common;

use std::fs;
use std::path::Path;
use std::process::Output;

use common::{fixtures, rewind};
use ratatui::style::{Color, Modifier, Style};
use rewind::theme::loader::{self, BUILT_INS, DEFAULT, IMPLICIT_BASE};
use rewind::theme::{Element, Theme};
use tempfile::TempDir;

fn config(themes: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().expect("a temporary config directory");
    fs::create_dir_all(dir.path().join("rewind").join("themes")).expect("a themes directory");
    for (name, text) in themes {
        let path = if *name == "theme" {
            dir.path().join("rewind").join("theme.toml")
        } else {
            dir.path().join("rewind").join("themes").join(format!("{name}.toml"))
        };
        fs::write(path, text).expect("a written theme");
    }
    dir
}

fn run(config_home: Option<&Path>, args: &[&str]) -> Output {
    let mut command = rewind();
    if let Some(dir) = config_home {
        command.env("XDG_CONFIG_HOME", dir);
    }
    command.args(args).output().expect("rewind runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn listing_themes_prints_the_built_ins_and_needs_no_claude_directory() {
    let output = run(None, &["--list-themes"]);
    assert!(output.status.success(), "rewind failed: {}", stderr(&output));
    let printed = stdout(&output);
    assert!(printed.contains("built-in"), "{printed}");
    assert!(printed.contains("ansi"), "{printed}");
}

#[test]
fn listing_themes_shows_the_readers_own_themes_under_their_config_directory() {
    let dir = config(&[("holodeck", ""), ("ansi", "")]);
    let printed = stdout(&run(Some(dir.path()), &["--list-themes"]));
    assert!(printed.contains("holodeck"), "{printed}");
    assert!(printed.contains("shadows the built-in"), "{printed}");
}

#[test]
fn a_theme_in_the_config_directory_is_found_without_a_flag() {
    let dir = config(&[("theme", "[palette]\naccent = \"#89b4fa\"\n")]);
    let output = run(Some(dir.path()), &["--claude-dir", &fixtures().display().to_string()]);
    assert!(output.status.success(), "rewind failed: {}", stderr(&output));
    assert!(stderr(&output).is_empty(), "a good theme warned: {}", stderr(&output));
}

#[test]
fn an_unknown_theme_name_fails_and_names_the_theme_that_was_asked_for() {
    let output = run(None, &["--theme", "nosuch", "--claude-dir", &fixtures().display().to_string()]);
    assert!(!output.status.success(), "an unknown theme was accepted");
    assert!(stderr(&output).contains("nosuch"), "{}", stderr(&output));
}

#[test]
fn a_bad_color_fails_with_a_message_that_names_the_key_that_carried_it() {
    let dir = config(&[("theme", "[palette]\naccent = \"blurple\"\n")]);
    let output = run(Some(dir.path()), &["--claude-dir", &fixtures().display().to_string()]);
    assert!(!output.status.success(), "a bad colour was accepted");
    let complaint = stderr(&output);
    assert!(complaint.contains("palette.accent"), "{complaint}");
    assert!(complaint.contains("blurple"), "{complaint}");
}

#[test]
fn an_unknown_key_warns_on_stderr_and_the_run_carries_on() {
    let dir = config(&[("theme", "[palette]\nforground = \"red\"\n")]);
    let output = run(Some(dir.path()), &["--claude-dir", &fixtures().display().to_string()]);
    assert!(output.status.success(), "a typo aborted the run: {}", stderr(&output));
    let complaint = stderr(&output);
    assert!(complaint.contains("palette.forground"), "{complaint}");
    assert!(stdout(&output).contains("holodeck"), "the run did not carry on to the listing");
}

#[test]
fn a_theme_file_is_read_and_never_written() {
    let dir = config(&[("theme", "[palette]\naccent = \"red\"\n")]);
    let path = dir.path().join("rewind").join("theme.toml");
    let before = fs::read(&path).expect("the theme file");
    let modified = fs::metadata(&path).expect("the theme file").modified().expect("a modification time");

    let output = run(Some(dir.path()), &["--claude-dir", &fixtures().display().to_string()]);
    assert!(output.status.success(), "rewind failed: {}", stderr(&output));

    assert_eq!(fs::read(&path).expect("the theme file"), before, "the theme file was rewritten");
    assert_eq!(fs::metadata(&path).expect("the theme file").modified().expect("a modification time"), modified);
}

fn swatch(color: Color) -> String {
    match color {
        Color::Reset => String::from("reset"),
        Color::Rgb(red, green, blue) => format!("#{red:02x}{green:02x}{blue:02x}"),
        Color::Indexed(index) => format!("indexed {index}"),
        named => format!("{named:?}").to_lowercase(),
    }
}

fn modifiers(style: Style) -> String {
    const NAMED: [(Modifier, &str); 6] = [
        (Modifier::BOLD, "bold"),
        (Modifier::DIM, "dim"),
        (Modifier::ITALIC, "italic"),
        (Modifier::UNDERLINED, "underline"),
        (Modifier::REVERSED, "reversed"),
        (Modifier::CROSSED_OUT, "crossed_out"),
    ];
    NAMED
        .iter()
        .filter(|(modifier, _)| style.add_modifier.contains(*modifier))
        .map(|(_, name)| *name)
        .collect::<Vec<_>>()
        .join(" ")
}

fn table(theme: &Theme) -> String {
    let mut rows = vec![format!("syntax_theme  {}", theme.syntax_theme.as_deref().unwrap_or("-")), String::new()];
    for element in Element::ALL {
        let style = theme.style(element);
        let fg = style.fg.map_or_else(|| String::from("-"), swatch);
        let bg = style.bg.map_or_else(|| String::from("-"), swatch);
        rows.push(format!("{:<20}  fg {fg:<12}  bg {bg:<12}  {}", element.key(), modifiers(style)).trim_end().to_owned());
    }
    rows.join("\n")
}

fn built_in(name: &str) -> Theme {
    let loaded = loader::load(None, Some(name), None).expect("a built-in theme loads with no config directory");
    assert!(loaded.warnings.is_empty(), "{name} warned: {:?}", loaded.warnings);
    loaded.theme
}

#[test]
fn the_built_ins_are_the_default_then_ansi_then_the_rest_alphabetically() {
    let names: Vec<&str> = BUILT_INS.iter().map(|(name, _)| *name).collect();
    assert_eq!(names.first(), Some(&DEFAULT), "{names:?}");
    assert_eq!(names.get(1), Some(&IMPLICIT_BASE), "{names:?}");

    let rest = names.get(2..).expect("more than two built-in themes");
    let mut sorted = rest.to_vec();
    sorted.sort_unstable();
    assert_eq!(rest, sorted.as_slice(), "the built-ins after ansi are not alphabetical");
}

#[test]
fn every_built_in_resolves_to_a_distinct_palette() {
    let palettes: Vec<String> = BUILT_INS.iter().map(|(name, _)| format!("{:?}", built_in(name).palette)).collect();
    let distinct: std::collections::BTreeSet<&String> = palettes.iter().collect();
    assert_eq!(distinct.len(), palettes.len(), "two built-in themes resolve to the same palette");
}

macro_rules! theme_snapshot {
    ($test:ident, $name:literal) => {
        #[test]
        fn $test() {
            insta::assert_snapshot!($name, table(&built_in($name)));
        }
    };
}

theme_snapshot!(the_spool_theme_resolves_as_it_reads, "spool");
theme_snapshot!(the_ansi_theme_resolves_as_it_reads, "ansi");
theme_snapshot!(the_catppuccin_latte_theme_resolves_as_it_reads, "catppuccin-latte");
theme_snapshot!(the_catppuccin_mocha_theme_resolves_as_it_reads, "catppuccin-mocha");
theme_snapshot!(the_default_plus_theme_resolves_as_it_reads, "default-plus");
theme_snapshot!(the_gruvbox_dark_theme_resolves_as_it_reads, "gruvbox-dark");
theme_snapshot!(the_gruvbox_light_theme_resolves_as_it_reads, "gruvbox-light");
theme_snapshot!(the_kanagawa_dragon_theme_resolves_as_it_reads, "kanagawa-dragon");
theme_snapshot!(the_nord_theme_resolves_as_it_reads, "nord");
theme_snapshot!(the_solarized_dark_theme_resolves_as_it_reads, "solarized-dark");
theme_snapshot!(the_solarized_light_theme_resolves_as_it_reads, "solarized-light");
theme_snapshot!(the_tokyo_night_theme_resolves_as_it_reads, "tokyo-night");
theme_snapshot!(the_tokyo_night_day_theme_resolves_as_it_reads, "tokyo-night-day");
theme_snapshot!(the_vesper_theme_resolves_as_it_reads, "vesper");
