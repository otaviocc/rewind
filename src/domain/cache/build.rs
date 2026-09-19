//! Building or refreshing one shard from a list of files: the three-way per-file decision
//! (unchanged / append / full rebuild) that makes the append fast path work.
//!
//! Scoped to the shard alone — `meta.json`'s session summaries are not append-aware (see
//! `domain::cache::meta`); they are cheap enough to recompute in full every build, the same
//! tier and cost as `domain::session::discover` already pays.

use std::fs::{self, File};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::domain::cache::fnv;
use crate::domain::cache::shard::{Builder, Kind, Shard};
use crate::domain::cancel::Cancel;
use crate::domain::lines::Lines;
use crate::domain::text::Extracted;

const HEAD_SIZE: usize = 4 * 1024;

pub struct Control<'a> {
    pub cancel: Cancel,
    tick: Option<&'a mut dyn FnMut()>,
}

impl Control<'static> {
    pub fn inert() -> Self {
        Self { cancel: Cancel::never(), tick: None }
    }
}

impl<'a> Control<'a> {
    pub fn new(cancel: Cancel, tick: &'a mut dyn FnMut()) -> Self {
        Self { cancel, tick: Some(tick) }
    }

    fn tick(&mut self) {
        if let Some(tick) = &mut self.tick {
            tick();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileAction {
    Fresh,
    Appended,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReport {
    pub name: String,
    pub action: FileAction,
}

#[derive(Debug, Clone)]
pub struct Output {
    pub shard_bytes: Vec<u8>,
    pub reports: Vec<FileReport>,
    pub cancelled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    pub path: PathBuf,
    pub name: String,
    pub kind: Kind,
}

pub fn build(files: &[Input], previous: Option<&Shard<'_>>, extract: fn(&[u8]) -> Vec<Extracted>, built_at_ms: i64) -> Output {
    build_with(files, previous, extract, built_at_ms, &mut Control::inert())
}

pub fn build_with(
    files: &[Input],
    previous: Option<&Shard<'_>>,
    extract: fn(&[u8]) -> Vec<Extracted>,
    built_at_ms: i64,
    control: &mut Control<'_>,
) -> Output {
    let mut builder = Builder::new();
    let mut reports = Vec::with_capacity(files.len());
    let mut cancelled = false;

    for input in files {
        if control.cancel.cancelled() {
            cancelled = true;
            break;
        }

        let Input { path, name, kind } = input;
        let Ok(metadata) = fs::metadata(path) else { continue };
        let len = metadata.len();
        let mtime_ms = mtime_ms_of(&metadata);

        let old = previous.and_then(|shard| {
            let index = shard.files().iter().position(|stamp| &stamp.path == name)?;
            shard.files().get(index).map(|stamp| (index, stamp.clone()))
        });

        let action = process_file(&mut builder, path, name, len, mtime_ms, previous, old.as_ref(), *kind, extract);
        reports.push(FileReport { name: name.clone(), action });
        control.tick();
    }

    Output { shard_bytes: builder.finish(built_at_ms), reports, cancelled }
}

#[allow(clippy::too_many_arguments)]
fn process_file(
    builder: &mut Builder,
    path: &Path,
    name: &str,
    len: u64,
    mtime_ms: i64,
    previous: Option<&Shard<'_>>,
    old: Option<&(usize, crate::domain::cache::shard::FileStamp)>,
    kind: Kind,
    extract: fn(&[u8]) -> Vec<Extracted>,
) -> FileAction {
    if let (Some(previous), Some((old_index, stamp))) = (previous, old) {
        if stamp.len == len && stamp.mtime_ms == mtime_ms {
            let file_idx = builder.push_file(name, len, mtime_ms, stamp.head_fnv, stamp.line_count);
            copy_forward(builder, previous, *old_index, file_idx);
            return FileAction::Unchanged;
        }

        if len > stamp.len
            && head_hash(path) == Some(stamp.head_fnv)
            && ends_in_newline(path, stamp.len) == Some(true)
            && let Some(tail) = read_tail(path, stamp.len, stamp.line_count, extract)
        {
            let file_idx = builder.push_file(name, len, mtime_ms, stamp.head_fnv, tail.line_count);
            copy_forward(builder, previous, *old_index, file_idx);
            push_pending(builder, file_idx, kind, tail.pending, next_seq_after(previous, *old_index));
            return FileAction::Appended;
        }
    }

    rebuild_fresh(builder, path, name, len, mtime_ms, kind, extract);
    FileAction::Fresh
}

fn copy_forward(builder: &mut Builder, previous: &Shard<'_>, old_file_idx: usize, new_file_idx: u32) {
    for record in previous.records() {
        if usize::try_from(record.file_idx).ok() != Some(old_file_idx) {
            continue;
        }
        let Some(text) = previous.text(record).and_then(|bytes| core::str::from_utf8(bytes).ok()) else { continue };
        builder.push_record(
            new_file_idx,
            record.line_no,
            record.byte_off,
            record.ts_ms,
            record.kind,
            record.field,
            record.flags,
            record.seq,
            text,
        );
    }
}

fn next_seq_after(previous: &Shard<'_>, old_file_idx: usize) -> u32 {
    previous
        .records()
        .iter()
        .filter(|record| usize::try_from(record.file_idx).ok() == Some(old_file_idx))
        .map(|record| record.seq)
        .max()
        .map_or(0, |max| max.saturating_add(1))
}

struct Tail {
    line_count: u32,
    pending: Vec<(u32, u64, Extracted)>,
}

fn read_tail(path: &Path, from: u64, old_line_count: u32, extract: fn(&[u8]) -> Vec<Extracted>) -> Option<Tail> {
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut lines = Lines::new(BufReader::new(file));

    let mut line_no = old_line_count;
    let mut start = from.checked_add(lines.complete_offset())?;
    let mut pending: Vec<(u32, u64, Extracted)> = Vec::new();
    while let Some(line) = lines.next_line().ok()? {
        line_no = line_no.saturating_add(1);
        for extracted in extract(line) {
            pending.push((line_no, start, extracted));
        }
        start = from.checked_add(lines.complete_offset())?;
    }

    Some(Tail { line_count: line_no, pending })
}

fn push_pending(builder: &mut Builder, file_idx: u32, kind: Kind, pending: Vec<(u32, u64, Extracted)>, first_seq: u32) {
    let mut seq = first_seq;
    for (line_no, byte_off, extracted) in pending {
        builder.push_record(
            file_idx,
            line_no,
            byte_off,
            extracted.ts_ms,
            kind,
            extracted.field,
            extracted.flags,
            seq,
            &extracted.text,
        );
        seq = seq.saturating_add(1);
    }
}

fn rebuild_fresh(
    builder: &mut Builder,
    path: &Path,
    name: &str,
    len: u64,
    mtime_ms: i64,
    kind: Kind,
    extract: fn(&[u8]) -> Vec<Extracted>,
) {
    let head_fnv = head_hash(path).unwrap_or(0);
    let Ok(mut lines) = Lines::open(path) else {
        builder.push_file(name, len, mtime_ms, head_fnv, 0);
        return;
    };

    let mut line_no: u32 = 0;
    let mut start = lines.complete_offset();
    let mut pending: Vec<(u32, u64, Extracted)> = Vec::new();
    while let Ok(Some(line)) = lines.next_line() {
        line_no = line_no.saturating_add(1);
        for extracted in extract(line) {
            pending.push((line_no, start, extracted));
        }
        start = lines.complete_offset();
    }

    let file_idx = builder.push_file(name, len, mtime_ms, head_fnv, line_no);
    push_pending(builder, file_idx, kind, pending, 0);
}

fn head_hash(path: &Path) -> Option<u64> {
    let mut file = File::open(path).ok()?;
    let mut buffer = vec![0_u8; HEAD_SIZE];
    let mut total = 0_usize;
    loop {
        let chunk = buffer.get_mut(total..)?;
        if chunk.is_empty() {
            break;
        }
        let read = file.read(chunk).ok()?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read)?;
    }
    buffer.truncate(total);
    Some(fnv::hash(&buffer))
}

fn ends_in_newline(path: &Path, at: u64) -> Option<bool> {
    if at == 0 {
        return Some(true);
    }
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(at.checked_sub(1)?)).ok()?;
    let mut byte = [0_u8; 1];
    file.read_exact(&mut byte).ok()?;
    Some(byte[0] == b'\n')
}

pub fn mtime_ms_of(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write;

    use tempfile::TempDir;

    use crate::domain::cache::shard::Shard;
    use crate::domain::text;

    fn write_session(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        let path = dir.join(name);
        let mut content = lines.join("\n");
        content.push('\n');
        fs::write(&path, content).expect("a written fixture session");
        path
    }

    fn transcript_input(path: &Path) -> Input {
        let name = path.file_name().and_then(|name| name.to_str()).expect("a utf-8 file name").to_owned();
        Input { path: path.to_path_buf(), name, kind: Kind::Transcript }
    }

    const HUMAN: &str = r#"{"type":"user","message":{"role":"user","content":"first line"},"origin":{"kind":"human"}}"#;
    const HUMAN_2: &str = r#"{"type":"user","message":{"role":"user","content":"second line"},"origin":{"kind":"human"}}"#;

    fn big_line(tag: &str) -> String {
        let filler: String = std::iter::repeat_n('z', 8_000).collect();
        format!(r#"{{"type":"user","message":{{"role":"user","content":"{tag} {filler}"}},"origin":{{"kind":"human"}}}}"#)
    }

    #[test]
    fn a_cold_build_with_no_previous_shard_marks_every_file_fresh() {
        let dir = TempDir::new().expect("a temporary directory");
        let path = write_session(dir.path(), "s1.jsonl", &[HUMAN]);

        let output = build(&[transcript_input(&path)], None, text::extract, 0);

        assert_eq!(output.reports, [FileReport { name: "s1.jsonl".to_owned(), action: FileAction::Fresh }]);
        let shard = Shard::parse(&output.shard_bytes).expect("a well-formed shard");
        assert_eq!(shard.records().len(), 1);
    }

    #[test]
    fn rebuilding_with_nothing_changed_marks_the_file_unchanged_and_keeps_its_records() {
        let dir = TempDir::new().expect("a temporary directory");
        let path = write_session(dir.path(), "s1.jsonl", &[HUMAN]);

        let first = build(&[transcript_input(&path)], None, text::extract, 0);
        let previous = Shard::parse(&first.shard_bytes).expect("a well-formed shard");

        let second = build(&[transcript_input(&path)], Some(&previous), text::extract, 1);

        assert_eq!(second.reports, [FileReport { name: "s1.jsonl".to_owned(), action: FileAction::Unchanged }]);
        let shard = Shard::parse(&second.shard_bytes).expect("a well-formed shard");
        assert_eq!(shard.records().len(), 1);
        let record = shard.records().first().expect("one record");
        assert_eq!(shard.text(record), Some(b"first line".as_slice()));
    }

    #[test]
    fn appending_a_line_is_marked_appended_and_keeps_both_the_old_and_new_records() {
        let dir = TempDir::new().expect("a temporary directory");
        let big = big_line("original");
        let path = write_session(dir.path(), "s1.jsonl", &[&big]);

        let first = build(&[transcript_input(&path)], None, text::extract, 0);
        let previous = Shard::parse(&first.shard_bytes).expect("a well-formed shard");

        let mut file = std::fs::OpenOptions::new().append(true).open(&path).expect("an appendable file");
        writeln!(file, "{HUMAN_2}").expect("an appended line");
        drop(file);
        filetime::set_file_mtime(&path, filetime::FileTime::from_unix_time(1, 0)).expect("a settable mtime");

        let second = build(&[transcript_input(&path)], Some(&previous), text::extract, 1);

        assert_eq!(second.reports, [FileReport { name: "s1.jsonl".to_owned(), action: FileAction::Appended }]);
        let shard = Shard::parse(&second.shard_bytes).expect("a well-formed shard");
        assert_eq!(shard.records().len(), 2);
        let texts: Vec<&[u8]> = shard.records().iter().filter_map(|record| shard.text(record)).collect();
        assert!(texts.iter().any(|text| memchr::memmem::find(text, b"original").is_some()));
        assert!(texts.contains(&b"second line".as_slice()));
    }

    #[test]
    fn appending_continues_line_numbering_from_the_stored_line_count() {
        let dir = TempDir::new().expect("a temporary directory");
        let big = big_line("original");
        let path = write_session(dir.path(), "s1.jsonl", &[&big]);

        let first = build(&[transcript_input(&path)], None, text::extract, 0);
        let previous = Shard::parse(&first.shard_bytes).expect("a well-formed shard");

        let mut file = std::fs::OpenOptions::new().append(true).open(&path).expect("an appendable file");
        writeln!(file, "{HUMAN_2}").expect("an appended line");
        drop(file);
        filetime::set_file_mtime(&path, filetime::FileTime::from_unix_time(1, 0)).expect("a settable mtime");

        let second = build(&[transcript_input(&path)], Some(&previous), text::extract, 1);
        assert_eq!(second.reports, [FileReport { name: "s1.jsonl".to_owned(), action: FileAction::Appended }]);
        let shard = Shard::parse(&second.shard_bytes).expect("a well-formed shard");

        let mut line_numbers: Vec<u32> = shard.records().iter().map(|record| record.line_no).collect();
        line_numbers.sort_unstable();
        assert_eq!(line_numbers, [1, 2]);
    }

    #[test]
    fn an_edit_inside_the_head_forces_a_full_rebuild_rather_than_an_append() {
        let dir = TempDir::new().expect("a temporary directory");
        let original = big_line("original");
        let path = write_session(dir.path(), "s1.jsonl", &[&original]);

        let first = build(&[transcript_input(&path)], None, text::extract, 0);
        let previous = Shard::parse(&first.shard_bytes).expect("a well-formed shard");

        let edited = big_line("edited");
        fs::write(&path, format!("{edited}\n{HUMAN_2}\n")).expect("an edited file that happens to be longer");
        filetime::set_file_mtime(&path, filetime::FileTime::from_unix_time(1, 0)).expect("a settable mtime");

        let second = build(&[transcript_input(&path)], Some(&previous), text::extract, 1);

        assert_eq!(second.reports, [FileReport { name: "s1.jsonl".to_owned(), action: FileAction::Fresh }]);
        let shard = Shard::parse(&second.shard_bytes).expect("a well-formed shard");
        let texts: Vec<&[u8]> = shard.records().iter().filter_map(|record| shard.text(record)).collect();
        assert!(texts.iter().any(|text| memchr::memmem::find(text, b"edited").is_some()));
        assert!(
            !texts.iter().any(|text| memchr::memmem::find(text, b"original").is_some()),
            "the edited head must not resurface as the old text"
        );
    }

    #[test]
    fn a_shrunk_file_forces_a_full_rebuild() {
        let dir = TempDir::new().expect("a temporary directory");
        let path = write_session(dir.path(), "s1.jsonl", &[HUMAN, HUMAN_2]);

        let first = build(&[transcript_input(&path)], None, text::extract, 0);
        let previous = Shard::parse(&first.shard_bytes).expect("a well-formed shard");

        fs::write(&path, format!("{HUMAN}\n")).expect("a truncated file");
        filetime::set_file_mtime(&path, filetime::FileTime::from_unix_time(1, 0)).expect("a settable mtime");

        let second = build(&[transcript_input(&path)], Some(&previous), text::extract, 1);

        assert_eq!(second.reports, [FileReport { name: "s1.jsonl".to_owned(), action: FileAction::Fresh }]);
    }

    #[test]
    fn a_missing_previous_stamp_for_a_new_file_is_fresh_even_alongside_an_unrelated_previous_shard() {
        let dir = TempDir::new().expect("a temporary directory");
        let existing = write_session(dir.path(), "s1.jsonl", &[HUMAN]);
        let first = build(&[transcript_input(&existing)], None, text::extract, 0);
        let previous = Shard::parse(&first.shard_bytes).expect("a well-formed shard");

        let new_file = write_session(dir.path(), "s2.jsonl", &[HUMAN_2]);
        let second = build(&[transcript_input(&existing), transcript_input(&new_file)], Some(&previous), text::extract, 1);

        let actions: Vec<FileAction> = second.reports.iter().map(|report| report.action).collect();
        assert_eq!(actions, [FileAction::Unchanged, FileAction::Fresh]);
    }

    #[test]
    fn the_appended_tail_is_copied_forward_rather_than_reparsed_from_the_start() {
        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("big.jsonl");
        let filler: String = std::iter::repeat_n('z', 20_000).collect();
        let sentinel =
            format!(r#"{{"type":"user","message":{{"role":"user","content":"sentinel {filler}"}},"origin":{{"kind":"human"}}}}"#);
        fs::write(&path, format!("{sentinel}\n")).expect("a large fixture file");

        let first = build(&[transcript_input(&path)], None, text::extract, 0);
        let previous = Shard::parse(&first.shard_bytes).expect("a well-formed shard");
        let old_len = fs::metadata(&path).expect("fixture metadata").len();

        let mut file = std::fs::OpenOptions::new().append(true).open(&path).expect("an appendable file");
        writeln!(file, "{HUMAN_2}").expect("a small appended line");
        drop(file);
        filetime::set_file_mtime(&path, filetime::FileTime::from_unix_time(1, 0)).expect("a settable mtime");

        let second = build(&[transcript_input(&path)], Some(&previous), text::extract, 1);
        assert_eq!(second.reports, [FileReport { name: "big.jsonl".to_owned(), action: FileAction::Appended }]);

        let shard = Shard::parse(&second.shard_bytes).expect("a well-formed shard");
        let sentinel_hits = shard
            .records()
            .iter()
            .filter(|record| shard.text(record).is_some_and(|text| memchr::memmem::find(text, b"sentinel").is_some()))
            .count();
        assert_eq!(sentinel_hits, 1, "the old prefix's sentinel text must be copied forward, not re-extracted a second time");

        let new_record = shard
            .records()
            .iter()
            .find(|record| shard.text(record) == Some(b"second line".as_slice()))
            .expect("the appended line's record");
        assert!(
            new_record.byte_off >= old_len,
            "the appended record's byte offset must land inside the new tail, not the old prefix"
        );
    }

    #[test]
    fn a_cancelled_build_stops_before_the_next_file_and_says_so() {
        let dir = TempDir::new().expect("a temporary directory");
        let first_path = write_session(dir.path(), "s1.jsonl", &[HUMAN]);
        let second_path = write_session(dir.path(), "s2.jsonl", &[HUMAN_2]);

        let gate = crate::domain::cancel::Gate::default();
        let token = gate.token();
        gate.bump();
        let mut noop = || {};
        let mut control = Control::new(token, &mut noop);

        let output =
            build_with(&[transcript_input(&first_path), transcript_input(&second_path)], None, text::extract, 0, &mut control);

        assert!(output.cancelled);
        assert_eq!(output.reports, []);
    }

    #[test]
    fn ticking_counts_one_file_at_a_time() {
        let dir = TempDir::new().expect("a temporary directory");
        let first_path = write_session(dir.path(), "s1.jsonl", &[HUMAN]);
        let second_path = write_session(dir.path(), "s2.jsonl", &[HUMAN_2]);

        let gate = crate::domain::cancel::Gate::default();
        let mut ticks: u32 = 0;
        let mut tick = || ticks = ticks.saturating_add(1);
        let mut control = Control::new(gate.token(), &mut tick);

        let output =
            build_with(&[transcript_input(&first_path), transcript_input(&second_path)], None, text::extract, 0, &mut control);

        assert!(!output.cancelled);
        assert_eq!(ticks, 2);
    }
}
