//! The corpus shard: a hand-rolled binary format.
//!
//! The corpus loads as one contiguous `Vec<u8>` from a single `read_to_end` and is searched
//! in place with zero per-message allocation. `bincode`/`postcard` would inflate 90 MB into
//! 60 000 `String`s.
//!
//! ```text
//! | header 64 B | stamps (stamps_len) | records (40 x record_count) | blob (blob_len) |
//! ```
//!
//! Every failure mode — bad magic, wrong version, a truncated file, a record pointing
//! outside the blob, header lengths that disagree with the file length — rejects the whole
//! shard. #20 treats that uniformly as cold: rebuild.

use thiserror::Error;

use crate::domain::cache::cursor::Cursor;

pub const CACHE_VERSION: u16 = 1;

const MAGIC: &[u8; 8] = b"RWNDCORP";
const HEADER_SIZE: usize = 64;
const RESERVED_HEADER_BYTES: usize = 24;
const RECORD_SIZE: usize = 40;
const RESERVED_RECORD_BYTES: usize = 1;

pub const TRUNCATED: u8 = 0b0000_0001;
pub const SIDECHAIN: u8 = 0b0000_0010;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ShardError {
    #[error("shard is truncated")]
    Truncated,
    #[error("bad magic")]
    BadMagic,
    #[error("unsupported shard version {0}")]
    WrongVersion(u16),
    #[error("a header length overflows this platform's usize")]
    LengthOverflow,
    #[error("shard length does not match its header")]
    LengthMismatch,
    #[error("a file stamp is malformed")]
    MalformedStamp,
    #[error("a record's text span falls outside the blob")]
    RecordOutOfBounds,
    #[error("a record names a file index outside the stamp table")]
    UnknownFileIndex,
    #[error("unknown record kind {0}")]
    UnknownKind(u8),
    #[error("unknown record field {0}")]
    UnknownField(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Transcript,
    Subagent,
    History,
}

impl Kind {
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Transcript => 0,
            Self::Subagent => 1,
            Self::History => 2,
        }
    }
}

impl TryFrom<u8> for Kind {
    type Error = ShardError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Transcript),
            1 => Ok(Self::Subagent),
            2 => Ok(Self::History),
            other => Err(ShardError::UnknownKind(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    UserPrompt,
    AssistantText,
    Thinking,
    ToolInput,
    ToolResult,
}

impl Field {
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::UserPrompt => 0,
            Self::AssistantText => 1,
            Self::Thinking => 2,
            Self::ToolInput => 3,
            Self::ToolResult => 4,
        }
    }
}

impl TryFrom<u8> for Field {
    type Error = ShardError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::UserPrompt),
            1 => Ok(Self::AssistantText),
            2 => Ok(Self::Thinking),
            3 => Ok(Self::ToolInput),
            4 => Ok(Self::ToolResult),
            other => Err(ShardError::UnknownField(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp {
    pub path: String,
    pub len: u64,
    pub mtime_ms: i64,
    pub head_fnv: u64,
    pub line_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    pub blob_off: u32,
    pub blob_len: u32,
    pub file_idx: u32,
    pub line_no: u32,
    pub byte_off: u64,
    pub ts_ms: i64,
    pub kind: Kind,
    pub field: Field,
    pub flags: u8,
    pub seq: u32,
}

#[derive(Debug, Default)]
pub struct Builder {
    files: Vec<FileStamp>,
    records: Vec<Record>,
    blob: Vec<u8>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_file(&mut self, path: &str, len: u64, mtime_ms: i64, head_fnv: u64, line_count: u32) -> u32 {
        let idx = u32::try_from(self.files.len()).unwrap_or(u32::MAX);
        self.files.push(FileStamp { path: path.to_owned(), len, mtime_ms, head_fnv, line_count });
        idx
    }

    #[allow(clippy::too_many_arguments)]
    pub fn push_record(
        &mut self,
        file_idx: u32,
        line_no: u32,
        byte_off: u64,
        ts_ms: i64,
        kind: Kind,
        field: Field,
        flags: u8,
        seq: u32,
        text: &str,
    ) {
        let blob_off = u32::try_from(self.blob.len()).unwrap_or(u32::MAX);
        let blob_len = u32::try_from(text.len()).unwrap_or(u32::MAX);
        self.blob.extend(text.bytes().map(|byte| byte.to_ascii_lowercase()));
        self.records.push(Record { blob_off, blob_len, file_idx, line_no, byte_off, ts_ms, kind, field, flags, seq });
    }

    pub fn finish(self, built_at_ms: i64) -> Vec<u8> {
        let mut stamps = Vec::new();
        for file in &self.files {
            let path_len = u16::try_from(file.path.len()).unwrap_or(u16::MAX);
            let path_bytes = file.path.as_bytes();
            let (kept, _) = path_bytes.split_at_checked(usize::from(path_len)).unwrap_or((path_bytes, &[]));
            stamps.extend_from_slice(&u16::try_from(kept.len()).unwrap_or(u16::MAX).to_le_bytes());
            stamps.extend_from_slice(kept);
            stamps.extend_from_slice(&file.len.to_le_bytes());
            stamps.extend_from_slice(&file.mtime_ms.to_le_bytes());
            stamps.extend_from_slice(&file.head_fnv.to_le_bytes());
            stamps.extend_from_slice(&file.line_count.to_le_bytes());
        }

        let records_len = self.records.len().saturating_mul(RECORD_SIZE);
        let capacity = HEADER_SIZE.saturating_add(stamps.len()).saturating_add(records_len).saturating_add(self.blob.len());
        let mut out = Vec::with_capacity(capacity);

        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&CACHE_VERSION.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&u32::try_from(self.records.len()).unwrap_or(u32::MAX).to_le_bytes());
        out.extend_from_slice(&u32::try_from(self.files.len()).unwrap_or(u32::MAX).to_le_bytes());
        out.extend_from_slice(&u32::try_from(stamps.len()).unwrap_or(u32::MAX).to_le_bytes());
        out.extend_from_slice(&u64::try_from(self.blob.len()).unwrap_or(u64::MAX).to_le_bytes());
        out.extend_from_slice(&built_at_ms.to_le_bytes());
        out.extend_from_slice(&[0_u8; RESERVED_HEADER_BYTES]);

        out.extend_from_slice(&stamps);

        for record in &self.records {
            out.extend_from_slice(&record.blob_off.to_le_bytes());
            out.extend_from_slice(&record.blob_len.to_le_bytes());
            out.extend_from_slice(&record.file_idx.to_le_bytes());
            out.extend_from_slice(&record.line_no.to_le_bytes());
            out.extend_from_slice(&record.byte_off.to_le_bytes());
            out.extend_from_slice(&record.ts_ms.to_le_bytes());
            out.push(record.kind.as_u8());
            out.push(record.field.as_u8());
            out.push(record.flags);
            out.extend_from_slice(&[0_u8; RESERVED_RECORD_BYTES]);
            out.extend_from_slice(&record.seq.to_le_bytes());
        }

        out.extend_from_slice(&self.blob);
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shard<'a> {
    files: Vec<FileStamp>,
    records: Vec<Record>,
    blob: &'a [u8],
    built_at_ms: i64,
}

impl<'a> Shard<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ShardError> {
        let mut cursor = Cursor::new(bytes);

        let magic = cursor.take(MAGIC.len()).ok_or(ShardError::Truncated)?;
        if magic != MAGIC {
            return Err(ShardError::BadMagic);
        }
        let version = cursor.u16().ok_or(ShardError::Truncated)?;
        if version != CACHE_VERSION {
            return Err(ShardError::WrongVersion(version));
        }
        let _flags = cursor.u16().ok_or(ShardError::Truncated)?;
        let record_count = cursor.u32().ok_or(ShardError::Truncated)?;
        let file_count = cursor.u32().ok_or(ShardError::Truncated)?;
        let stamps_len = cursor.u32().ok_or(ShardError::Truncated)?;
        let blob_len = cursor.u64().ok_or(ShardError::Truncated)?;
        let built_at_ms = cursor.i64().ok_or(ShardError::Truncated)?;
        let _reserved = cursor.take(RESERVED_HEADER_BYTES).ok_or(ShardError::Truncated)?;

        let stamps_len = usize::try_from(stamps_len).map_err(|_| ShardError::LengthOverflow)?;
        let record_count_usize = usize::try_from(record_count).map_err(|_| ShardError::LengthOverflow)?;
        let blob_len = usize::try_from(blob_len).map_err(|_| ShardError::LengthOverflow)?;

        let records_len = record_count_usize.checked_mul(RECORD_SIZE).ok_or(ShardError::LengthOverflow)?;
        let body_len =
            stamps_len.checked_add(records_len).and_then(|sum| sum.checked_add(blob_len)).ok_or(ShardError::LengthOverflow)?;
        let total = HEADER_SIZE.checked_add(body_len).ok_or(ShardError::LengthOverflow)?;
        if total != bytes.len() {
            return Err(ShardError::LengthMismatch);
        }

        let stamps_bytes = cursor.take(stamps_len).ok_or(ShardError::Truncated)?;
        let files = parse_stamps(stamps_bytes, file_count)?;

        let records_bytes = cursor.take(records_len).ok_or(ShardError::Truncated)?;
        let blob = cursor.take(blob_len).ok_or(ShardError::Truncated)?;
        if !cursor.is_empty() {
            return Err(ShardError::LengthMismatch);
        }

        let records = parse_records(records_bytes, record_count_usize, blob.len(), files.len())?;

        Ok(Self { files, records, blob, built_at_ms })
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    pub fn files(&self) -> &[FileStamp] {
        &self.files
    }

    pub const fn built_at_ms(&self) -> i64 {
        self.built_at_ms
    }

    pub const fn blob(&self) -> &'a [u8] {
        self.blob
    }

    pub fn text(&self, record: &Record) -> Option<&'a [u8]> {
        let start = usize::try_from(record.blob_off).ok()?;
        let len = usize::try_from(record.blob_len).ok()?;
        let end = start.checked_add(len)?;
        self.blob.get(start..end)
    }
}

fn parse_stamps(bytes: &[u8], file_count: u32) -> Result<Vec<FileStamp>, ShardError> {
    let count = usize::try_from(file_count).map_err(|_| ShardError::LengthOverflow)?;
    let mut cursor = Cursor::new(bytes);
    let mut files = Vec::with_capacity(count.min(4096));

    for _ in 0..count {
        let path_len = cursor.u16().ok_or(ShardError::Truncated)?;
        let path_bytes = cursor.take(usize::from(path_len)).ok_or(ShardError::Truncated)?;
        let path = core::str::from_utf8(path_bytes).map_err(|_| ShardError::MalformedStamp)?.to_owned();
        let len = cursor.u64().ok_or(ShardError::Truncated)?;
        let mtime_ms = cursor.i64().ok_or(ShardError::Truncated)?;
        let head_fnv = cursor.u64().ok_or(ShardError::Truncated)?;
        let line_count = cursor.u32().ok_or(ShardError::Truncated)?;
        files.push(FileStamp { path, len, mtime_ms, head_fnv, line_count });
    }

    if !cursor.is_empty() {
        return Err(ShardError::LengthMismatch);
    }
    Ok(files)
}

fn parse_records(bytes: &[u8], count: usize, blob_len: usize, file_count: usize) -> Result<Vec<Record>, ShardError> {
    let mut cursor = Cursor::new(bytes);
    let mut records = Vec::with_capacity(count.min(4096));

    for _ in 0..count {
        let blob_off = cursor.u32().ok_or(ShardError::Truncated)?;
        let blob_len_field = cursor.u32().ok_or(ShardError::Truncated)?;
        let file_idx = cursor.u32().ok_or(ShardError::Truncated)?;
        let line_no = cursor.u32().ok_or(ShardError::Truncated)?;
        let byte_off = cursor.u64().ok_or(ShardError::Truncated)?;
        let ts_ms = cursor.i64().ok_or(ShardError::Truncated)?;
        let kind = Kind::try_from(cursor.u8().ok_or(ShardError::Truncated)?)?;
        let field = Field::try_from(cursor.u8().ok_or(ShardError::Truncated)?)?;
        let flags = cursor.u8().ok_or(ShardError::Truncated)?;
        let _reserved = cursor.take(RESERVED_RECORD_BYTES).ok_or(ShardError::Truncated)?;
        let seq = cursor.u32().ok_or(ShardError::Truncated)?;

        let start = usize::try_from(blob_off).map_err(|_| ShardError::RecordOutOfBounds)?;
        let span = usize::try_from(blob_len_field).map_err(|_| ShardError::RecordOutOfBounds)?;
        let end = start.checked_add(span).ok_or(ShardError::RecordOutOfBounds)?;
        if end > blob_len {
            return Err(ShardError::RecordOutOfBounds);
        }

        let file_idx_usize = usize::try_from(file_idx).map_err(|_| ShardError::UnknownFileIndex)?;
        if file_idx_usize >= file_count {
            return Err(ShardError::UnknownFileIndex);
        }

        records.push(Record { blob_off, blob_len: blob_len_field, file_idx, line_no, byte_off, ts_ms, kind, field, flags, seq });
    }

    if !cursor.is_empty() {
        return Err(ShardError::LengthMismatch);
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAMP_LEN_A_JSONL: usize = 2 + 7 + 8 + 8 + 8 + 4;

    #[test]
    fn the_header_is_exactly_sixty_four_bytes_and_the_record_is_exactly_forty() {
        let bytes = Builder::new().finish(0);
        assert_eq!(bytes.len(), HEADER_SIZE);

        let mut builder = Builder::new();
        let file = builder.push_file("a.jsonl", 10, 0, 0, 0);
        builder.push_record(file, 1, 0, 0, Kind::Transcript, Field::UserPrompt, 0, 0, "hi");
        let with_one_record = builder.finish(0);
        let stamp_len = 2 + 7 + 8 + 8 + 8 + 4;
        assert_eq!(
            with_one_record.len(),
            HEADER_SIZE + stamp_len + RECORD_SIZE + 2,
            "stamp = 2 (path_len) + 7 (path) + 8 (len) + 8 (mtime) + 8 (head_fnv) + 4 (line_count)"
        );
    }

    #[test]
    fn an_empty_shard_round_trips() {
        let bytes = Builder::new().finish(42);
        let shard = Shard::parse(&bytes).expect("an empty shard is valid");
        assert!(shard.records().is_empty());
        assert!(shard.files().is_empty());
        assert_eq!(shard.built_at_ms(), 42);
        assert!(shard.blob().is_empty());
    }

    #[test]
    fn a_single_record_shard_round_trips_its_text() {
        let mut builder = Builder::new();
        let file = builder.push_file("session.jsonl", 100, 5, 99, 0);
        builder.push_record(file, 3, 40, 1000, Kind::Transcript, Field::UserPrompt, 0, 0, "Read The Grid Scanner");
        let bytes = builder.finish(7);

        let shard = Shard::parse(&bytes).expect("a well-formed single-record shard");
        assert_eq!(
            shard.files(),
            [FileStamp { path: "session.jsonl".to_owned(), len: 100, mtime_ms: 5, head_fnv: 99, line_count: 0 }]
        );
        let record = shard.records().first().expect("one record");
        assert_eq!(record.file_idx, 0);
        assert_eq!(record.line_no, 3);
        assert_eq!(record.byte_off, 40);
        assert_eq!(record.ts_ms, 1000);
        assert_eq!(record.kind, Kind::Transcript);
        assert_eq!(record.field, Field::UserPrompt);
        assert_eq!(shard.text(record), Some(b"read the grid scanner".as_slice()), "the blob is lowercased");
    }

    #[test]
    fn many_records_across_several_files_all_read_back() {
        let mut builder = Builder::new();
        let a = builder.push_file("a.jsonl", 1, 0, 1, 0);
        let b = builder.push_file("b.jsonl", 2, 0, 2, 0);
        for index in 0..50 {
            let file = if index % 2 == 0 { a } else { b };
            builder.push_record(file, index, u64::from(index), 0, Kind::Subagent, Field::ToolResult, 0, index, "payload");
        }
        let bytes = builder.finish(0);

        let shard = Shard::parse(&bytes).expect("many records across two files");
        assert_eq!(shard.records().len(), 50);
        assert_eq!(shard.files().len(), 2);
        for record in shard.records() {
            assert_eq!(shard.text(record), Some(b"payload".as_slice()));
        }
    }

    #[test]
    fn a_bad_magic_is_rejected() {
        let mut bytes = Builder::new().finish(0);
        if let Some(first) = bytes.first_mut() {
            *first = b'X';
        }
        assert_eq!(Shard::parse(&bytes), Err(ShardError::BadMagic));
    }

    #[test]
    fn a_wrong_version_is_rejected() {
        let mut bytes = Builder::new().finish(0);
        if let Some(slot) = bytes.get_mut(8..10) {
            slot.copy_from_slice(&99_u16.to_le_bytes());
        }
        assert_eq!(Shard::parse(&bytes), Err(ShardError::WrongVersion(99)));
    }

    #[test]
    fn a_file_truncated_inside_the_header_is_rejected() {
        let bytes = Builder::new().finish(0);
        let cut = bytes.get(..10).expect("header is longer than 10 bytes");
        assert_eq!(Shard::parse(cut), Err(ShardError::Truncated));
    }

    #[test]
    fn a_file_truncated_inside_the_stamp_table_is_rejected() {
        let mut builder = Builder::new();
        builder.push_file("a-long-enough-path.jsonl", 1, 0, 0, 0);
        let bytes = builder.finish(0);
        let cut = bytes.get(..bytes.len().saturating_sub(5)).expect("bytes to cut");
        assert_eq!(Shard::parse(cut), Err(ShardError::LengthMismatch));
    }

    #[test]
    fn a_file_truncated_inside_the_record_section_is_rejected() {
        let mut builder = Builder::new();
        let file = builder.push_file("a.jsonl", 1, 0, 0, 0);
        builder.push_record(file, 0, 0, 0, Kind::Transcript, Field::UserPrompt, 0, 0, "x");
        let bytes = builder.finish(0);
        let cut = bytes.get(..bytes.len().saturating_sub(1)).expect("bytes to cut");
        assert_eq!(Shard::parse(cut), Err(ShardError::LengthMismatch));
    }

    #[test]
    fn a_file_truncated_inside_the_blob_is_rejected() {
        let mut builder = Builder::new();
        let file = builder.push_file("a.jsonl", 1, 0, 0, 0);
        builder.push_record(file, 0, 0, 0, Kind::Transcript, Field::UserPrompt, 0, 0, "hello");
        let bytes = builder.finish(0);
        let cut = bytes.get(..bytes.len().saturating_sub(2)).expect("bytes to cut");
        assert_eq!(Shard::parse(cut), Err(ShardError::LengthMismatch));
    }

    #[test]
    fn a_record_pointing_past_the_blob_is_rejected() {
        let mut builder = Builder::new();
        let file = builder.push_file("a.jsonl", 1, 0, 0, 0);
        builder.push_record(file, 0, 0, 0, Kind::Transcript, Field::UserPrompt, 0, 0, "hi");
        let mut bytes = builder.finish(0);

        let record_start = HEADER_SIZE + STAMP_LEN_A_JSONL;
        if let Some(slot) = bytes.get_mut(record_start.saturating_add(4)..record_start.saturating_add(8)) {
            slot.copy_from_slice(&99_u32.to_le_bytes());
        }
        assert_eq!(Shard::parse(&bytes), Err(ShardError::RecordOutOfBounds));
    }

    #[test]
    fn header_lengths_that_disagree_with_the_file_length_are_rejected() {
        let mut builder = Builder::new();
        let file = builder.push_file("a.jsonl", 1, 0, 0, 0);
        builder.push_record(file, 0, 0, 0, Kind::Transcript, Field::UserPrompt, 0, 0, "hi");
        let mut bytes = builder.finish(0);
        bytes.push(0);
        assert_eq!(Shard::parse(&bytes), Err(ShardError::LengthMismatch));
    }

    #[test]
    fn an_unknown_record_kind_byte_is_rejected() {
        let mut builder = Builder::new();
        let file = builder.push_file("a.jsonl", 1, 0, 0, 0);
        builder.push_record(file, 0, 0, 0, Kind::Transcript, Field::UserPrompt, 0, 0, "hi");
        let mut bytes = builder.finish(0);

        let kind_offset = HEADER_SIZE + STAMP_LEN_A_JSONL + 32;
        if let Some(slot) = bytes.get_mut(kind_offset) {
            *slot = 200;
        }
        assert_eq!(Shard::parse(&bytes), Err(ShardError::UnknownKind(200)));
    }

    #[test]
    fn an_unknown_record_field_byte_is_rejected() {
        let mut builder = Builder::new();
        let file = builder.push_file("a.jsonl", 1, 0, 0, 0);
        builder.push_record(file, 0, 0, 0, Kind::Transcript, Field::UserPrompt, 0, 0, "hi");
        let mut bytes = builder.finish(0);

        let field_offset = HEADER_SIZE + STAMP_LEN_A_JSONL + 33;
        if let Some(slot) = bytes.get_mut(field_offset) {
            *slot = 200;
        }
        assert_eq!(Shard::parse(&bytes), Err(ShardError::UnknownField(200)));
    }

    #[test]
    fn a_record_naming_a_file_index_outside_the_stamp_table_is_rejected() {
        let mut builder = Builder::new();
        let file = builder.push_file("a.jsonl", 1, 0, 0, 0);
        builder.push_record(file, 0, 0, 0, Kind::Transcript, Field::UserPrompt, 0, 0, "hi");
        let mut bytes = builder.finish(0);

        let file_idx_offset = HEADER_SIZE + STAMP_LEN_A_JSONL + 8;
        if let Some(slot) = bytes.get_mut(file_idx_offset..file_idx_offset.saturating_add(4)) {
            slot.copy_from_slice(&7_u32.to_le_bytes());
        }
        assert_eq!(Shard::parse(&bytes), Err(ShardError::UnknownFileIndex));
    }

    #[test]
    fn a_record_count_that_would_overflow_the_section_arithmetic_is_rejected_not_panicked() {
        let mut bytes = Builder::new().finish(0);
        if let Some(slot) = bytes.get_mut(12..16) {
            slot.copy_from_slice(&u32::MAX.to_le_bytes());
        }
        assert_eq!(Shard::parse(&bytes), Err(ShardError::LengthMismatch));
    }

    #[test]
    fn non_ascii_text_passes_through_the_blob_unchanged() {
        let mut builder = Builder::new();
        let file = builder.push_file("a.jsonl", 1, 0, 0, 0);
        builder.push_record(file, 0, 0, 0, Kind::Transcript, Field::AssistantText, 0, 0, "café ☕ HÉLLO");
        let bytes = builder.finish(0);

        let shard = Shard::parse(&bytes).expect("valid shard");
        let record = shard.records().first().expect("one record");
        let text = shard.text(record).expect("text within the blob");
        assert_eq!(text, "café ☕ hÉllo".as_bytes(), "only ASCII bytes are lowercased");
    }

    #[test]
    fn flag_bits_round_trip() {
        let mut builder = Builder::new();
        let file = builder.push_file("a.jsonl", 1, 0, 0, 0);
        builder.push_record(file, 0, 0, 0, Kind::Transcript, Field::ToolResult, TRUNCATED | SIDECHAIN, 0, "x");
        let bytes = builder.finish(0);

        let shard = Shard::parse(&bytes).expect("valid shard");
        let record = shard.records().first().expect("one record");
        assert_eq!(record.flags, TRUNCATED | SIDECHAIN);
    }

    #[test]
    fn a_file_stamps_line_count_round_trips_so_an_append_can_resume_numbering_without_a_rescan() {
        let mut builder = Builder::new();
        builder.push_file("a.jsonl", 500, 0, 0, 42);
        let bytes = builder.finish(0);

        let shard = Shard::parse(&bytes).expect("valid shard");
        let file = shard.files().first().expect("one file stamp");
        assert_eq!(file.line_count, 42);
    }
}
