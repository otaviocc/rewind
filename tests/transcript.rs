//! Rendering a real fixture session to plain text: the exit criteria in #9.

#![allow(clippy::expect_used)]

mod common;

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use common::fixtures;
use rewind::domain::thread;
use rewind::render::line::RenderedLine;
use rewind::render::message::transcript;
use tempfile::TempDir;

const HOLODECK: &str = "-Users-fixture-Developer-holodeck";
const BASELINE: &str = "11111111-1111-4111-8111-111111111111";
const WIDE: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const IMAGES: &str = "33333333-3333-4333-8333-333333333333";

fn session_path(session: &str) -> PathBuf {
    fixtures().join("projects").join(HOLODECK).join(format!("{session}.jsonl"))
}

fn rendered(session: &str, width: usize) -> String {
    render_file(&session_path(session), width)
}

fn render_file(path: &Path, width: usize) -> String {
    let conversation = thread::build(path).expect("a built conversation");
    lines_of(&transcript(&conversation, width))
}

fn lines_of(lines: &[RenderedLine]) -> String {
    lines.iter().map(RenderedLine::text).collect::<Vec<_>>().join("\n")
}

fn widths(path: &Path, width: usize) -> Vec<usize> {
    let conversation = thread::build(path).expect("a built conversation");
    transcript(&conversation, width).iter().map(RenderedLine::width).collect()
}

#[test]
fn the_baseline_session_renders_as_plain_text() {
    insta::assert_snapshot!("baseline-80", rendered(BASELINE, 80));
}

#[test]
fn the_baseline_session_rewraps_for_a_narrow_column() {
    insta::assert_snapshot!("baseline-32", rendered(BASELINE, 32));
}

#[test]
fn a_session_of_wide_glyphs_renders_and_wraps_by_display_width() {
    insta::assert_snapshot!("wide-glyphs-40", rendered(WIDE, 40));
}

#[test]
fn every_line_of_a_wide_glyph_session_fits_the_column_it_was_wrapped_for() {
    let path = session_path(WIDE);
    for width in [16, 24, 40, 80] {
        for line in widths(&path, width) {
            assert!(line <= width, "a line of {line} columns was wrapped for {width}");
        }
    }
}

#[test]
fn an_inline_image_is_named_and_never_decoded() {
    let text = rendered(IMAGES, 80);
    assert!(text.contains("[image · png · "), "no image line in:\n{text}");
    assert!(!text.contains("iVBOR"), "a base64 payload reached the transcript");
}

fn huge_session() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("a temporary directory");
    let path = dir.path().join("huge.jsonl");
    let blob = "A".repeat(4 * 1024 * 1024);
    let prose = "the deflector array reads back one plate at a time ".repeat(40);
    let mut text = String::new();
    let _ = write!(
        text,
        r#"{{"parentUuid":null,"isSidechain":false,"type":"user","uuid":"u0","timestamp":"2026-01-01T00:00:00Z","sessionId":"h","origin":{{"kind":"human"}},"message":{{"role":"user","content":[{{"type":"text","text":"look"}},{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{blob}"}}}}]}}}}"#
    );
    text.push('\n');
    for index in 0..8_000u32 {
        let parent = if index == 0 { "u0".to_owned() } else { format!("a{}", index.saturating_sub(1)) };
        let _ = write!(
            text,
            r#"{{"parentUuid":"{parent}","isSidechain":false,"type":"assistant","uuid":"a{index}","timestamp":"2026-01-01T00:00:01Z","sessionId":"h","requestId":"r{index}","message":{{"model":"opus-5","id":"msg_{index}","role":"assistant","content":[{{"type":"text","text":"{prose}"}}]}}}}"#
        );
        text.push('\n');
    }
    fs::write(&path, text).expect("a written transcript");
    (dir, path)
}

#[test]
fn a_ten_megabyte_session_renders_without_decoding_a_byte_of_base64() {
    let (_dir, path) = huge_session();
    assert!(fs::metadata(&path).expect("the transcript exists").len() > 10 * 1024 * 1024);

    let conversation = thread::build(&path).expect("a built conversation");
    let lines = transcript(&conversation, 80);

    assert!(lines.len() > 8_000, "only {} lines", lines.len());
    assert!(lines.iter().all(|line| line.width() <= 80));
    assert!(!lines.iter().any(|line| line.text().contains("AAAAAAAAAAAAAAAA")), "a base64 payload reached the transcript");
    assert!(lines.iter().any(|line| line.text().starts_with("[image · png · ")), "the image was not named");
}
