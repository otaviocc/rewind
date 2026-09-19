//! Resolving a `Hit` back to something openable, and rendering its snippet.
//!
//! The snippet is re-read from the original `.jsonl` — never from the blob, so a stale
//! cache can never lie about what's actually there. All of this touches the filesystem and
//! belongs on a worker thread.

use std::fs::File;
use std::io::{BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::domain::cache::shard::Kind;
use crate::domain::lines::Lines;
use crate::domain::scan;
use crate::domain::search::engine::Hit;
use crate::domain::text;

const HISTORY_FILE_NAME: &str = "history.jsonl";
const PROJECTS_DIR_NAME: &str = "projects";
const SNIPPET_RADIUS: usize = 60;
const ELLIPSIS: &str = "…";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub project_directory: String,
    pub session_id: String,
    pub uuid: Option<String>,
}

pub fn file_path(claude_dir: &Path, hit: &Hit) -> PathBuf {
    hit.directory.as_ref().map_or_else(
        || claude_dir.join(HISTORY_FILE_NAME),
        |directory| claude_dir.join(PROJECTS_DIR_NAME).join(directory).join(&hit.file_name),
    )
}

fn read_line_at(path: &Path, byte_off: u64) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(byte_off)).ok()?;
    let mut lines = Lines::new(BufReader::new(file));
    lines.next_line().ok()?.map(<[u8]>::to_vec)
}

pub fn snippet(claude_dir: &Path, hit: &Hit, terms: &[String]) -> Option<String> {
    let path = file_path(claude_dir, hit);
    let line = read_line_at(&path, hit.byte_off)?;
    let extracted = match hit.kind {
        Kind::History => text::extract_history(&line),
        Kind::Transcript | Kind::Subagent => text::extract(&line),
    };
    let field_text = extracted.into_iter().find(|item| item.field == hit.field)?.text;
    Some(window(&field_text, terms))
}

fn window(text: &str, terms: &[String]) -> String {
    let lower = text.to_lowercase();
    let found = terms.iter().find_map(|term| memchr::memmem::find(lower.as_bytes(), term.as_bytes()));
    let Some(at) = found else {
        return truncate_chars(text, SNIPPET_RADIUS.saturating_mul(2));
    };

    let start = char_floor(text, at.saturating_sub(SNIPPET_RADIUS));
    let end = char_ceil(text, at.saturating_add(SNIPPET_RADIUS));
    let mut out = String::new();
    if start > 0 {
        out.push_str(ELLIPSIS);
    }
    out.push_str(text.get(start..end).unwrap_or(text));
    if end < text.len() {
        out.push_str(ELLIPSIS);
    }
    out
}

const fn char_floor(text: &str, mut at: usize) -> usize {
    while at > 0 && !text.is_char_boundary(at) {
        at = at.saturating_sub(1);
    }
    at
}

fn char_ceil(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while at < text.len() && !text.is_char_boundary(at) {
        at = at.saturating_add(1);
    }
    at
}

fn truncate_chars(text: &str, max_bytes: usize) -> String {
    let end = char_ceil(text, max_bytes);
    text.get(..end).unwrap_or(text).to_owned()
}

pub fn resolve_transcript(claude_dir: &Path, hit: &Hit) -> Option<Opened> {
    let directory = hit.directory.clone()?;
    let path = file_path(claude_dir, hit);
    let session_id = path.file_stem()?.to_str()?.to_owned();
    let line = read_line_at(&path, hit.byte_off)?;
    let uuid = scan::top_level_str(&line, "uuid").map(str::to_owned);
    Some(Opened { project_directory: directory, session_id, uuid })
}

pub fn resolve_history(claude_dir: &Path, hit: &Hit) -> Option<(String, String)> {
    let path = file_path(claude_dir, hit);
    let line = read_line_at(&path, hit.byte_off)?;
    let session_id = scan::top_level_str(&line, "sessionId")?.to_owned();
    let project_cwd = scan::top_level_str(&line, "project")?.to_owned();
    Some((session_id, project_cwd))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::domain::cache::shard::Field;

    fn hit(directory: Option<&str>, file_name: &str, byte_off: u64, kind: Kind, field: Field) -> Hit {
        Hit {
            score: 0,
            directory: directory.map(str::to_owned),
            file_name: file_name.to_owned(),
            line_no: 1,
            byte_off,
            ts_ms: 0,
            kind,
            field,
        }
    }

    #[test]
    fn a_transcript_hit_resolves_the_project_directory_and_the_line_s_uuid() {
        let claude = TempDir::new().expect("a temporary directory");
        let project_dir = claude.path().join("projects").join("-a-project");
        fs::create_dir_all(&project_dir).expect("a project directory");
        let line =
            br#"{"type":"user","uuid":"u1","message":{"role":"user","content":"read the grid"},"origin":{"kind":"human"}}"#;
        fs::write(project_dir.join("s1.jsonl"), [line.as_slice(), b"\n"].concat()).expect("a written session line");

        let hit = hit(Some("-a-project"), "s1.jsonl", 0, Kind::Transcript, Field::UserPrompt);
        let opened = resolve_transcript(claude.path(), &hit).expect("a resolved hit");
        assert_eq!(opened.project_directory, "-a-project");
        assert_eq!(opened.session_id, "s1");
        assert_eq!(opened.uuid.as_deref(), Some("u1"));
    }

    #[test]
    fn a_history_hit_resolves_the_session_id_and_project_cwd() {
        let claude = TempDir::new().expect("a temporary directory");
        let line = br#"{"display":"read the grid","sessionId":"s9","project":"/Users/fixture/holodeck"}"#;
        fs::write(claude.path().join("history.jsonl"), [line.as_slice(), b"\n"].concat()).expect("a written history line");

        let hit = hit(None, "history.jsonl", 0, Kind::History, Field::UserPrompt);
        let (session_id, project) = resolve_history(claude.path(), &hit).expect("a resolved history hit");
        assert_eq!(session_id, "s9");
        assert_eq!(project, "/Users/fixture/holodeck");
    }

    #[test]
    fn the_snippet_is_read_from_the_original_file_not_the_lowercased_blob() {
        let claude = TempDir::new().expect("a temporary directory");
        let project_dir = claude.path().join("projects").join("-a-project");
        fs::create_dir_all(&project_dir).expect("a project directory");
        let line = br#"{"type":"user","uuid":"u1","message":{"role":"user","content":"Read The Grid Scanner"},"origin":{"kind":"human"}}"#;
        fs::write(project_dir.join("s1.jsonl"), [line.as_slice(), b"\n"].concat()).expect("a written session line");

        let hit = hit(Some("-a-project"), "s1.jsonl", 0, Kind::Transcript, Field::UserPrompt);
        let snippet = snippet(claude.path(), &hit, &["grid".to_owned()]).expect("a snippet");
        assert!(snippet.contains("Grid"), "the snippet must keep the original casing: {snippet}");
    }

    #[test]
    fn a_long_field_is_windowed_around_the_match_with_ellipses() {
        let claude = TempDir::new().expect("a temporary directory");
        let project_dir = claude.path().join("projects").join("-a-project");
        fs::create_dir_all(&project_dir).expect("a project directory");
        let filler_before: String = std::iter::repeat_n('a', 200).collect();
        let filler_after: String = std::iter::repeat_n('b', 200).collect();
        let content = format!("{filler_before} needle {filler_after}");
        let line = format!(
            r#"{{"type":"user","uuid":"u1","message":{{"role":"user","content":"{content}"}},"origin":{{"kind":"human"}}}}"#
        );
        fs::write(project_dir.join("s1.jsonl"), format!("{line}\n")).expect("a written session line");

        let hit = hit(Some("-a-project"), "s1.jsonl", 0, Kind::Transcript, Field::UserPrompt);
        let snippet = snippet(claude.path(), &hit, &["needle".to_owned()]).expect("a snippet");
        assert!(snippet.contains("needle"));
        assert!(snippet.starts_with(ELLIPSIS));
        assert!(snippet.ends_with(ELLIPSIS));
        assert!(snippet.len() < content.len());
    }
}
