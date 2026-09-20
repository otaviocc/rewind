//! Writing an export to disk, and copying the raw transcript verbatim for the JSONL format.
//!
//! The only place besides `ui::clipboard` allowed to touch a path outside `~/.claude` on the
//! user's behalf — `App` only ever hands a destination and a payload to `App::take_export`.

use std::fs;
use std::path::{Path, PathBuf};

pub enum Attempt {
    Written(PathBuf),
    NeedsConfirmation,
    Failed(String),
}

pub fn attempt_text(dest: &Path, text: &str, force: bool) -> Attempt {
    if !force && dest.exists() {
        return Attempt::NeedsConfirmation;
    }
    match write_text(dest, text) {
        Ok(()) => Attempt::Written(dest.to_path_buf()),
        Err(error) => Attempt::Failed(error.to_string()),
    }
}

pub fn attempt_copy(source: &Path, dest: &Path, force: bool) -> Attempt {
    if !force && dest.exists() {
        return Attempt::NeedsConfirmation;
    }
    match copy_file(source, dest) {
        Ok(()) => Attempt::Written(dest.to_path_buf()),
        Err(error) => Attempt::Failed(error.to_string()),
    }
}

fn write_text(dest: &Path, text: &str) -> std::io::Result<()> {
    ensure_parent(dest)?;
    fs::write(dest, text)
}

fn copy_file(source: &Path, dest: &Path) -> std::io::Result<()> {
    ensure_parent(dest)?;
    fs::copy(source, dest).map(|_| ())
}

fn ensure_parent(dest: &Path) -> std::io::Result<()> {
    dest.parent().filter(|parent| !parent.as_os_str().is_empty()).map_or(Ok(()), fs::create_dir_all)
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    #[test]
    fn writing_text_to_a_new_path_succeeds_without_asking() {
        let dir = TempDir::new().expect("a temp dir");
        let dest = dir.path().join("out.md");
        assert!(matches!(attempt_text(&dest, "hello", false), Attempt::Written(_)));
        assert_eq!(fs::read_to_string(&dest).expect("the written file"), "hello");
    }

    #[test]
    fn writing_over_an_existing_path_without_force_asks_first() {
        let dir = TempDir::new().expect("a temp dir");
        let dest = dir.path().join("out.md");
        fs::write(&dest, "old").expect("a pre-existing file");
        assert!(matches!(attempt_text(&dest, "new", false), Attempt::NeedsConfirmation));
        assert_eq!(fs::read_to_string(&dest).expect("the untouched file"), "old", "must not overwrite without asking");
    }

    #[test]
    fn forcing_the_write_overwrites_an_existing_path() {
        let dir = TempDir::new().expect("a temp dir");
        let dest = dir.path().join("out.md");
        fs::write(&dest, "old").expect("a pre-existing file");
        assert!(matches!(attempt_text(&dest, "new", true), Attempt::Written(_)));
        assert_eq!(fs::read_to_string(&dest).expect("the overwritten file"), "new");
    }

    #[test]
    fn a_nested_destination_directory_is_created() {
        let dir = TempDir::new().expect("a temp dir");
        let dest = dir.path().join("nested").join("deeper").join("out.md");
        assert!(matches!(attempt_text(&dest, "hello", false), Attempt::Written(_)));
        assert!(dest.is_file());
    }

    #[test]
    fn copying_the_raw_transcript_carries_its_bytes_verbatim() {
        let dir = TempDir::new().expect("a temp dir");
        let source = dir.path().join("session.jsonl");
        fs::write(&source, "{\"type\":\"user\"}\n").expect("a source transcript");
        let dest = dir.path().join("out.jsonl");
        assert!(matches!(attempt_copy(&source, &dest, false), Attempt::Written(_)));
        assert_eq!(fs::read_to_string(&dest).expect("the copied file"), "{\"type\":\"user\"}\n");
    }

    #[test]
    fn a_missing_source_fails_rather_than_writing_nothing_silently() {
        let dir = TempDir::new().expect("a temp dir");
        let source = dir.path().join("does-not-exist.jsonl");
        let dest = dir.path().join("out.jsonl");
        assert!(matches!(attempt_copy(&source, &dest, false), Attempt::Failed(_)));
    }
}
