//! Reading a transcript line by line: one reused buffer, a cap on the pathological, and
//! base64 gone before anything deserializes.

use std::fs::File;
use std::io::{self, BufRead, BufReader, ErrorKind};
use std::path::Path;

pub const MAX_LINE: usize = 8 * 1024 * 1024;

const REDACT_LINE_FLOOR: usize = 256 * 1024;
const REDACT_SPAN_FLOOR: usize = 1024;
const SAMPLE: usize = 64;
const BASE64_MARKER: &[u8] = b"\"type\":\"base64\"";
const DATA_KEY: &[u8] = b"\"data\":\"";
const REDACTED_KEY: &[u8] = b",\"redactedBytes\":";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineNote {
    Oversized { offset: u64, bytes: u64 },
    Truncated { offset: u64 },
}

pub struct Lines<R> {
    reader: R,
    line: Vec<u8>,
    scratch: Vec<u8>,
    notes: Vec<LineNote>,
    offset: u64,
    complete_offset: u64,
    finished: bool,
}

impl Lines<BufReader<File>> {
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self::new(BufReader::new(File::open(path)?)))
    }
}

impl<R: BufRead> Lines<R> {
    pub const fn new(reader: R) -> Self {
        Self { reader, line: Vec::new(), scratch: Vec::new(), notes: Vec::new(), offset: 0, complete_offset: 0, finished: false }
    }

    pub fn notes(&self) -> &[LineNote] {
        &self.notes
    }

    pub const fn complete_offset(&self) -> u64 {
        self.complete_offset
    }

    pub fn next_line(&mut self) -> io::Result<Option<&[u8]>> {
        while self.fill()? {
            if self.line.is_empty() {
                continue;
            }
            let line = &self.line;
            let scratch = &mut self.scratch;
            return Ok(Some(redact_base64(line, scratch)));
        }
        Ok(None)
    }

    fn fill(&mut self) -> io::Result<bool> {
        while !self.finished {
            self.line.clear();
            let start = self.offset;
            let mut overflow: u64 = 0;

            let terminated = loop {
                let step = {
                    let available = match self.reader.fill_buf() {
                        Ok(available) => available,
                        Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                        Err(error) => return Err(error),
                    };
                    if available.is_empty() {
                        break false;
                    }
                    if let Some(index) = memchr::memchr(b'\n', available) {
                        let (head, _) = available.split_at_checked(index).unwrap_or((available, &[]));
                        append_capped(&mut self.line, head, &mut overflow);
                        (index.checked_add(1).unwrap_or(index), true)
                    } else {
                        append_capped(&mut self.line, available, &mut overflow);
                        (available.len(), false)
                    }
                };
                let (consumed, done) = step;
                self.reader.consume(consumed);
                self.offset = self.offset.saturating_add(u64::try_from(consumed).unwrap_or(u64::MAX));
                if done {
                    break true;
                }
            };

            if !terminated {
                self.finished = true;
                if !self.line.is_empty() || overflow > 0 {
                    self.notes.push(LineNote::Truncated { offset: self.complete_offset });
                }
                return Ok(false);
            }

            self.complete_offset = self.offset;
            if overflow > 0 {
                let bytes = u64::try_from(self.line.len()).unwrap_or(u64::MAX).saturating_add(overflow);
                self.notes.push(LineNote::Oversized { offset: start, bytes });
                continue;
            }

            if self.line.last() == Some(&b'\r') {
                self.line.pop();
            }
            return Ok(true);
        }
        Ok(false)
    }
}

fn append_capped(line: &mut Vec<u8>, chunk: &[u8], overflow: &mut u64) {
    let room = MAX_LINE.saturating_sub(line.len());
    match chunk.split_at_checked(room) {
        Some((head, tail)) => {
            line.extend_from_slice(head);
            *overflow = overflow.saturating_add(u64::try_from(tail.len()).unwrap_or(u64::MAX));
        }
        None => line.extend_from_slice(chunk),
    }
}

pub fn redact_base64<'a>(line: &'a [u8], scratch: &'a mut Vec<u8>) -> &'a [u8] {
    if line.len() <= REDACT_LINE_FLOOR || memchr::memmem::find(line, BASE64_MARKER).is_none() {
        return line;
    }

    scratch.clear();
    let mut rest = line;
    let mut redacted = false;

    while let Some(position) = memchr::memmem::find(rest, DATA_KEY) {
        let Some((head, at_key)) = rest.split_at_checked(position) else { break };
        let Some((key, blob_start)) = at_key.split_at_checked(DATA_KEY.len()) else { break };
        let Some(end) = memchr::memchr(b'"', blob_start) else { break };
        let Some((blob, tail)) = blob_start.split_at_checked(end) else { break };

        scratch.extend_from_slice(head);
        scratch.extend_from_slice(key);
        if blob.len() > REDACT_SPAN_FLOOR && looks_like_base64(blob) {
            redacted = true;
            let Some((quote, after)) = tail.split_at_checked(1) else { break };
            scratch.extend_from_slice(quote);
            scratch.extend_from_slice(REDACTED_KEY);
            scratch.extend_from_slice(blob.len().to_string().as_bytes());
            rest = after;
        } else {
            scratch.extend_from_slice(blob);
            rest = tail;
        }
    }

    if !redacted {
        return line;
    }
    scratch.extend_from_slice(rest);
    scratch.as_slice()
}

fn looks_like_base64(blob: &[u8]) -> bool {
    blob.first_chunk::<SAMPLE>()
        .is_some_and(|sample| sample.iter().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=')))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(input: &[u8]) -> (Vec<Vec<u8>>, Vec<LineNote>, u64) {
        let mut lines = Lines::new(io::BufReader::new(input));
        let mut read = Vec::new();
        while let Some(line) = lines.next_line().expect("an in-memory reader never fails") {
            read.push(line.to_vec());
        }
        (read, lines.notes().to_vec(), lines.complete_offset())
    }

    #[test]
    fn an_empty_reader_yields_nothing_and_notes_nothing() {
        let (read, notes, offset) = collect(b"");
        assert!(read.is_empty());
        assert!(notes.is_empty());
        assert_eq!(offset, 0);
    }

    #[test]
    fn newline_terminated_lines_are_read_whole() {
        let (read, notes, offset) = collect(b"{\"a\":1}\n{\"b\":2}\n");
        assert_eq!(read, vec![b"{\"a\":1}".to_vec(), b"{\"b\":2}".to_vec()]);
        assert!(notes.is_empty());
        assert_eq!(offset, 16);
    }

    #[test]
    fn blank_lines_are_skipped_without_a_note() {
        let (read, notes, _) = collect(b"{\"a\":1}\n\n\n{\"b\":2}\n");
        assert_eq!(read.len(), 2);
        assert!(notes.is_empty());
    }

    #[test]
    fn carriage_returns_are_stripped() {
        let (read, notes, _) = collect(b"{\"a\":1}\r\n{\"b\":2}\r\n");
        assert_eq!(read, vec![b"{\"a\":1}".to_vec(), b"{\"b\":2}".to_vec()]);
        assert!(notes.is_empty());
    }

    #[test]
    fn a_final_line_without_a_newline_is_truncated_and_not_yielded() {
        let (read, notes, offset) = collect(b"{\"a\":1}\n{\"b\":2,\"cut\"");
        assert_eq!(read, vec![b"{\"a\":1}".to_vec()]);
        assert_eq!(notes, vec![LineNote::Truncated { offset: 8 }]);
        assert_eq!(offset, 8);
    }

    #[test]
    fn a_line_at_exactly_the_cap_is_read() {
        let mut input = vec![b'x'; MAX_LINE];
        input.push(b'\n');
        let (read, notes, _) = collect(&input);
        assert_eq!(read.len(), 1);
        assert_eq!(read.first().map(Vec::len), Some(MAX_LINE));
        assert!(notes.is_empty());
    }

    #[test]
    fn a_line_over_the_cap_is_skipped_noted_and_does_not_disturb_the_next_line() {
        let mut input = Vec::new();
        input.extend_from_slice(b"{\"a\":1}\n");
        input.extend(std::iter::repeat_n(b'x', MAX_LINE.saturating_add(10)));
        input.extend_from_slice(b"\n{\"b\":2}\n");
        let (read, notes, _) = collect(&input);
        assert_eq!(read, vec![b"{\"a\":1}".to_vec(), b"{\"b\":2}".to_vec()]);
        assert_eq!(notes, vec![LineNote::Oversized { offset: 8, bytes: u64::try_from(MAX_LINE).unwrap_or(0) + 10 }]);
    }

    fn image_line(blob_bytes: usize) -> Vec<u8> {
        let blob: String = std::iter::repeat_n('A', blob_bytes).collect();
        let filler: String = std::iter::repeat_n('z', REDACT_LINE_FLOOR).collect();
        format!(
            r#"{{"type":"user","note":"{filler}","message":{{"content":[{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{blob}"}}}}]}}}}"#
        )
        .into_bytes()
    }

    #[test]
    fn a_two_megabyte_image_line_redacts_to_a_handful_of_kilobytes_and_still_parses() {
        let line = image_line(1_960_000);
        let mut scratch = Vec::new();
        let redacted = redact_base64(&line, &mut scratch).to_vec();
        assert!(line.len() > 1_960_000);
        assert!(redacted.len() < REDACT_LINE_FLOOR.saturating_add(4096), "redacted to {} bytes", redacted.len());
        let value: serde_json::Value = serde_json::from_slice(&redacted).expect("the redacted line still parses");
        assert_eq!(value.pointer("/message/content/0/source/data").and_then(serde_json::Value::as_str), Some(""));
    }

    #[test]
    fn redaction_records_the_length_it_elided() {
        let line = image_line(1_960_000);
        let mut scratch = Vec::new();
        let redacted = redact_base64(&line, &mut scratch).to_vec();
        let value: serde_json::Value = serde_json::from_slice(&redacted).expect("the redacted line still parses");
        assert_eq!(
            value.pointer("/message/content/0/source/redactedBytes").and_then(serde_json::Value::as_u64),
            Some(1_960_000),
            "the elided length is the one fact the renderer needs back"
        );
    }

    #[test]
    fn a_short_data_field_on_a_large_line_is_left_alone() {
        let line = image_line(64);
        let mut scratch = Vec::new();
        assert_eq!(redact_base64(&line, &mut scratch), line.as_slice());
    }

    #[test]
    fn a_large_data_field_that_is_not_base64_is_left_alone() {
        let prose: String = std::iter::repeat_n("the grid scanner reads back, ", 200).collect();
        let filler: String = std::iter::repeat_n('z', REDACT_LINE_FLOOR).collect();
        let line = format!(r#"{{"type":"user","note":"{filler}","source":{{"type":"base64","data":"{prose}"}}}}"#).into_bytes();
        let mut scratch = Vec::new();
        assert_eq!(redact_base64(&line, &mut scratch), line.as_slice());
    }

    #[test]
    fn a_small_line_is_never_redacted_however_much_base64_it_carries() {
        let blob: String = std::iter::repeat_n('A', 5000).collect();
        let line = format!(r#"{{"source":{{"type":"base64","data":"{blob}"}}}}"#).into_bytes();
        let mut scratch = Vec::new();
        assert_eq!(redact_base64(&line, &mut scratch), line.as_slice());
    }

    #[test]
    fn a_large_line_with_no_base64_marker_is_never_walked() {
        let filler: String = std::iter::repeat_n('z', REDACT_LINE_FLOOR).collect();
        let line = format!(r#"{{"type":"user","note":"{filler}"}}"#).into_bytes();
        let mut scratch = Vec::new();
        assert_eq!(redact_base64(&line, &mut scratch), line.as_slice());
        assert!(scratch.is_empty());
    }

    #[test]
    fn the_reader_redacts_without_being_asked() {
        let mut input = image_line(1_960_000);
        input.push(b'\n');
        let (read, _, _) = collect(&input);
        assert_eq!(read.len(), 1);
        assert!(read.first().map_or(usize::MAX, Vec::len) < REDACT_LINE_FLOOR.saturating_add(4096));
    }
}
