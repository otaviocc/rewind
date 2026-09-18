//! The theme flags at the binary's edge: what a reader's config directory does to a run.

#![allow(clippy::expect_used)]

mod common;

use std::fs;
use std::path::Path;
use std::process::Output;

use common::{fixtures, rewind};
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
